//! OpenAI Responses (ChatGPT 反代) ↔ Anthropic Messages 协议翻译, codex 专用入口.
//!
//! 历史: 本模块 v1.8 起独立实现 Anthropic ↔ Responses 翻译, 服务 `openai_codex` provider
//! (chatgpt.com/backend-api/codex 反代). v2.2 把通用映射逻辑搬到了 [`responses_common`],
//! 本模块只剩 codex chatgpt 反代专用入口 + 现有测试集 (保持回归基线)。
//!
//! ## codex 后端的四大强约束 (Phase 0 实测确认)
//! - 强制 `stream: true`, 非流式直接 400
//! - 禁 `max_output_tokens`, 即便用户传也得 strip (一并 drop Anthropic `max_tokens`)
//! - 强制 `store: false` 与 `include: ["reasoning.encrypted_content"]`
//! - 强制 `instructions` 字段 present (即使空字符串), 否则 400 "Instructions are required"
//!
//! 全部封装在 [`ResponsesTransformConfig::codex_chatgpt`] 里, 本模块只调用 common::build_responses_body。

use serde_json::Value;

use crate::error::AppResult;

use super::responses_common::{self, ResponsesTransformConfig};

// 重新导出供 oauth_dispatch.rs 等调用方使用 — 与 v2.1 之前的 API 完全一致, 调用点不需要改。
pub use super::responses_common::{
    parse_sse_frame, AnthropicEvent, NonStreamingCollector, ResponsesSseConverter,
};

/// 把 Anthropic Messages 请求体转成 OpenAI Responses 请求体 (codex 路径, chatgpt 反代专用).
///
/// model 已经被 pipeline 改写为 slot 真实模型名 (例如 `gpt-5.5`).
/// 不能用 `gpt-5` 等通用名 — chatgpt 反代会 400。
pub fn anthropic_to_responses(body: &Value) -> AppResult<Value> {
    responses_common::build_responses_body(body, &ResponsesTransformConfig::codex_chatgpt())
}

// ============================================================
// 测试 (覆盖 codex 路径行为, 保证 P1 重构零回归)
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_drops_max_tokens_and_forces_stream() {
        let body = json!({
            "model": "gpt-5.5",
            "max_tokens": 1024,
            "stream": false,
            "messages": [{"role": "user", "content": "hi"}],
        });
        let out = anthropic_to_responses(&body).unwrap();
        assert_eq!(out["stream"], json!(true));
        assert_eq!(out["store"], json!(false));
        assert_eq!(out["include"], json!(["reasoning.encrypted_content"]));
        assert!(out.get("max_output_tokens").is_none());
        assert!(out.get("max_tokens").is_none());
    }

    #[test]
    fn request_system_string_to_instructions() {
        let body = json!({
            "model": "gpt-5.5",
            "system": "你是一个助手",
            "messages": [{"role": "user", "content": "hi"}],
        });
        let out = anthropic_to_responses(&body).unwrap();
        assert_eq!(out["instructions"], json!("你是一个助手"));
    }

    #[test]
    fn request_system_array_to_instructions() {
        let body = json!({
            "model": "gpt-5.5",
            "system": [
                {"type": "text", "text": "段落 A"},
                {"type": "text", "text": "段落 B"},
            ],
            "messages": [{"role": "user", "content": "hi"}],
        });
        let out = anthropic_to_responses(&body).unwrap();
        assert_eq!(out["instructions"], json!("段落 A\n\n段落 B"));
    }

    // Codex 后端要求 instructions 必须 present, 即使空字符串。下面三个 case 守住边界。
    #[test]
    fn request_without_system_still_includes_empty_instructions() {
        let body = json!({
            "model": "gpt-5.5",
            "messages": [{"role": "user", "content": "hi"}],
        });
        let out = anthropic_to_responses(&body).unwrap();
        assert_eq!(out["instructions"], json!(""));
    }

    #[test]
    fn request_empty_system_string_still_includes_empty_instructions() {
        let body = json!({
            "model": "gpt-5.5",
            "system": "",
            "messages": [{"role": "user", "content": "hi"}],
        });
        let out = anthropic_to_responses(&body).unwrap();
        assert_eq!(out["instructions"], json!(""));
    }

    #[test]
    fn request_empty_system_array_still_includes_empty_instructions() {
        let body = json!({
            "model": "gpt-5.5",
            "system": [],
            "messages": [{"role": "user", "content": "hi"}],
        });
        let out = anthropic_to_responses(&body).unwrap();
        assert_eq!(out["instructions"], json!(""));
    }

    #[test]
    fn request_messages_text_content() {
        let body = json!({
            "model": "gpt-5.5",
            "messages": [
                {"role": "user", "content": "你好"},
                {"role": "assistant", "content": "我是助手"},
            ],
        });
        let out = anthropic_to_responses(&body).unwrap();
        let input = out["input"].as_array().unwrap();
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"][0]["type"], "input_text");
        assert_eq!(input[1]["role"], "assistant");
        assert_eq!(input[1]["content"][0]["type"], "output_text");
    }

    #[test]
    fn request_tool_use_promoted_to_top_level() {
        let body = json!({
            "model": "gpt-5.5",
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "我去查一下"},
                    {"type": "tool_use", "id": "call_1", "name": "search",
                     "input": {"query": "rust"}},
                ]
            }, {
                "role": "user",
                "content": [
                    {"type": "tool_result", "tool_use_id": "call_1", "content": "结果 X"},
                ]
            }],
        });
        let out = anthropic_to_responses(&body).unwrap();
        let input = out["input"].as_array().unwrap();
        assert_eq!(input.len(), 3);
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "call_1");
        assert_eq!(input[1]["name"], "search");
        // arguments 必须是字符串化的 JSON
        assert_eq!(input[1]["arguments"], json!("{\"query\":\"rust\"}"));
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[2]["call_id"], "call_1");
        assert_eq!(input[2]["output"], "结果 X");
    }

    #[test]
    fn request_tools_converted_with_parameters_field() {
        let body = json!({
            "model": "gpt-5.5",
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [{
                "name": "get_weather",
                "description": "查询天气",
                "input_schema": {
                    "type": "object",
                    "properties": {"city": {"type": "string"}},
                    "required": ["city"]
                }
            }],
        });
        let out = anthropic_to_responses(&body).unwrap();
        let tools = out["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["name"], "get_weather");
        assert!(tools[0].get("parameters").is_some());
        assert!(tools[0].get("input_schema").is_none());
    }

    #[test]
    fn converter_exposes_response_model_after_created() {
        let mut conv = ResponsesSseConverter::new();
        assert_eq!(conv.response_model(), "");
        conv.feed(
            "response.created",
            &json!({"response":{"id":"resp_1","model":"gpt-5.5"}}),
        );
        assert_eq!(conv.response_model(), "gpt-5.5");
    }

    #[test]
    fn sse_text_flow_basic() {
        let mut conv = ResponsesSseConverter::new();

        // response.created → message_start
        let started = conv.feed(
            "response.created",
            &json!({"type":"response.created","response":{"id":"resp_1","model":"gpt-5.5"}}),
        );
        assert_eq!(started.len(), 1);
        assert_eq!(started[0].event_name(), "message_start");

        // 跳过 reasoning item (output_index=0)
        let r = conv.feed(
            "response.output_item.added",
            &json!({"output_index":0,"item":{"type":"reasoning","encrypted_content":"X"}}),
        );
        assert!(r.is_empty(), "reasoning 不暴露");

        let r = conv.feed(
            "response.output_item.done",
            &json!({"output_index":0,"item":{"type":"reasoning"}}),
        );
        assert!(r.is_empty());

        // message item (output_index=1)
        let r = conv.feed(
            "response.output_item.added",
            &json!({"output_index":1,"item":{"type":"message"}}),
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].event_name(), "content_block_start");

        // text deltas
        let r = conv.feed(
            "response.output_text.delta",
            &json!({"output_index":1,"delta":"Hello "}),
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].event_name(), "content_block_delta");
        let r = conv.feed(
            "response.output_text.delta",
            &json!({"output_index":1,"delta":"world"}),
        );
        assert_eq!(r.len(), 1);

        // output_item.done → content_block_stop
        let r = conv.feed(
            "response.output_item.done",
            &json!({"output_index":1,"item":{"type":"message","status":"completed"}}),
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].event_name(), "content_block_stop");

        // response.completed → message_delta + message_stop
        let r = conv.feed(
            "response.completed",
            &json!({"type":"response.completed","response":{
                "status":"completed",
                "usage":{"input_tokens":10, "output_tokens":5}
            }}),
        );
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].event_name(), "message_delta");
        assert_eq!(r[1].event_name(), "message_stop");
    }

    #[test]
    fn sse_tool_call_flow() {
        let mut conv = ResponsesSseConverter::new();
        conv.feed("response.created", &json!({"response":{"id":"r","model":"gpt-5.5"}}));
        // function_call output_item.added
        let r = conv.feed(
            "response.output_item.added",
            &json!({"output_index":0,"item":{
                "type":"function_call",
                "call_id":"call_x",
                "name":"get_weather"
            }}),
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].event_name(), "content_block_start");
        if let AnthropicEvent::ContentBlockStart { content_block, .. } = &r[0] {
            assert_eq!(content_block["type"], "tool_use");
            assert_eq!(content_block["id"], "call_x");
            assert_eq!(content_block["name"], "get_weather");
        } else {
            panic!("expected ContentBlockStart");
        }

        // arguments deltas
        let r = conv.feed(
            "response.function_call_arguments.delta",
            &json!({"output_index":0,"delta":"{\"city\""}),
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].event_name(), "content_block_delta");
        if let AnthropicEvent::ContentBlockDelta { delta, .. } = &r[0] {
            assert_eq!(delta["type"], "input_json_delta");
            assert_eq!(delta["partial_json"], "{\"city\"");
        }

        // done
        let r = conv.feed(
            "response.output_item.done",
            &json!({"output_index":0,"item":{"type":"function_call"}}),
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].event_name(), "content_block_stop");
    }

    #[test]
    fn nonstreaming_collector_assembles_message() {
        let mut col = NonStreamingCollector::new();
        col.feed(
            "response.created",
            &json!({"response":{"id":"resp_1","model":"gpt-5.5"}}),
        );
        col.feed(
            "response.output_item.added",
            &json!({"output_index":0,"item":{"type":"reasoning"}}),
        );
        col.feed(
            "response.output_item.done",
            &json!({"output_index":0,"item":{"type":"reasoning"}}),
        );
        col.feed(
            "response.output_item.added",
            &json!({"output_index":1,"item":{"type":"message"}}),
        );
        col.feed(
            "response.output_text.delta",
            &json!({"output_index":1,"delta":"你好 "}),
        );
        col.feed(
            "response.output_text.delta",
            &json!({"output_index":1,"delta":"世界"}),
        );
        col.feed(
            "response.output_item.done",
            &json!({"output_index":1,"item":{"type":"message"}}),
        );
        col.feed(
            "response.completed",
            &json!({"response":{"status":"completed","usage":{"input_tokens":12,"output_tokens":3}}}),
        );

        let msg = col.finalize();
        assert_eq!(msg["type"], "message");
        assert_eq!(msg["role"], "assistant");
        assert_eq!(msg["model"], "gpt-5.5");
        let content = msg["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "你好 世界");
        assert_eq!(msg["usage"]["input_tokens"], 12);
        assert_eq!(msg["usage"]["output_tokens"], 3);
    }

    #[test]
    fn nonstreaming_collector_assembles_tool_use() {
        let mut col = NonStreamingCollector::new();
        col.feed("response.created", &json!({"response":{"id":"r","model":"gpt-5.5"}}));
        col.feed(
            "response.output_item.added",
            &json!({"output_index":0,"item":{
                "type":"function_call","call_id":"call_x","name":"get_weather"
            }}),
        );
        col.feed(
            "response.function_call_arguments.delta",
            &json!({"output_index":0,"delta":"{\"city\":"}),
        );
        col.feed(
            "response.function_call_arguments.delta",
            &json!({"output_index":0,"delta":"\"Beijing\"}"}),
        );
        col.feed(
            "response.output_item.done",
            &json!({"output_index":0,"item":{"type":"function_call"}}),
        );
        col.feed(
            "response.completed",
            &json!({"response":{"status":"completed","usage":{"input_tokens":1,"output_tokens":1}}}),
        );
        let msg = col.finalize();
        let content = msg["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "tool_use");
        assert_eq!(content[0]["id"], "call_x");
        assert_eq!(content[0]["name"], "get_weather");
        assert_eq!(content[0]["input"], json!({"city": "Beijing"}));
    }

    #[test]
    fn parse_sse_frame_handles_basic() {
        let raw = "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{}}";
        let (name, data) = parse_sse_frame(raw).unwrap();
        assert_eq!(name, "response.created");
        assert_eq!(data["type"], "response.created");
    }

    #[test]
    fn parse_sse_frame_returns_none_on_garbage() {
        assert!(parse_sse_frame("").is_none());
        assert!(parse_sse_frame("data: not-json").is_none());
    }

    #[test]
    fn anthropic_event_serializes_with_correct_event_name() {
        let evt = AnthropicEvent::MessageStart {
            message: json!({"id": "x"}),
        };
        let frame = evt.to_sse_frame();
        assert!(frame.starts_with("event: message_start\n"));
        assert!(frame.contains("\"id\":\"x\""));
        assert!(frame.ends_with("\n\n"));
    }
}
