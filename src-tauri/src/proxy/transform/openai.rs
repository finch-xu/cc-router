//! OpenAI 官方 / 兼容 `/v1/responses` 通用入口.
//!
//! 与 [`super::openai_responses`] (codex 反代专用) 共享底层 [`super::responses_common`]
//! 翻译 helper, 但走 [`ResponsesTransformConfig::openai_official`] 配置 — 不带 chatgpt 反代的
//! 四大约束 (force stream / strip max_tokens / 强制 instructions / 强制 store=false+include reasoning)。
//!
//! 主要差异 (相对 codex 入口):
//! - 跟随客户端 `stream` 值, 不强制改写
//! - 把 Anthropic `max_tokens` 映射为 OpenAI `max_output_tokens` (而非 drop)
//! - `instructions` 仅在 system 存在时注入
//! - 默认不注入 `include` 字段 (expose_reasoning=true 时启用)
//! - reasoning 双向: 上游 reasoning item → Anthropic thinking content_block (signature 携带 encrypted_content);
//!   客户端回传 thinking 块 → 上游 input items 里的 reasoning item (多轮回灌)
//! - `reasoning.effort` 透传, 优先级链 yaml 默认 / 订阅级 / 客户端 thinking.budget_tokens 自动映射 (见 [`resolve_reasoning_effort`])

use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::AppResult;

use super::responses_common::{
    self, encode_reasoning_signature, ResponsesTransformConfig,
};

/// OpenAI Responses `reasoning.effort` 枚举值. Anthropic `thinking.budget_tokens` 自动映射也走这里。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
}

impl ReasoningEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// Anthropic `thinking.budget_tokens` → OpenAI effort. 阈值与 gpt-5 系列 reasoning_tokens 经验值对齐。
    pub fn from_budget_tokens(budget_tokens: u64) -> Self {
        if budget_tokens < 4096 {
            Self::Minimal
        } else if budget_tokens < 16384 {
            Self::Low
        } else if budget_tokens < 65536 {
            Self::Medium
        } else {
            Self::High
        }
    }
}

impl FromStr for ReasoningEffort {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            other => Err(format!("无效 reasoning_effort: {other}")),
        }
    }
}

/// `anthropic_to_openai_responses` 的可选项, 由 dispatch 层从 yaml + 订阅 + 客户端 body 推导后传入。
#[derive(Debug, Clone, Default)]
pub struct OpenAiResponsesExtras {
    /// None 表示不传, 让 OpenAI 走默认。
    pub reasoning_effort: Option<ReasoningEffort>,
    /// 是否在响应翻译时把 reasoning 内容暴露成 Anthropic thinking content_block
    /// (同时影响请求侧: include 注入 + 多轮回灌)
    pub expose_reasoning: bool,
}

/// 把 Anthropic Messages 请求体转成 OpenAI `/v1/responses` 请求体 (官方/兼容路径).
pub fn anthropic_to_openai_responses(
    body: &Value,
    extras: &OpenAiResponsesExtras,
) -> AppResult<Value> {
    let config = ResponsesTransformConfig::openai_official(extras.expose_reasoning);
    let mut out = responses_common::build_responses_body(body, &config)?;
    if let Some(effort) = extras.reasoning_effort {
        out["reasoning"] = json!({ "effort": effort.as_str() });
    }
    Ok(out)
}

/// reasoning effort 优先级链解析. 返回应填入 OpenAI request body 的 effort 值 (None 表示不传)。
///
/// 优先级 (高 → 低):
/// 1. `body.extra_body.reasoning_effort` (string)
/// 2. `body.thinking.effort` (string)
/// 3. `body.thinking.budget_tokens` (integer, 自动映射, 阈值见 [`ReasoningEffort::from_budget_tokens`])
/// 4. `subscription_effort` (订阅级配置, 后续 migration)
/// 5. `yaml_default_effort` (provider yaml `default_reasoning_effort`)
///
/// 任意一档字符串非法都视为缺失, 继续往下找。
pub fn resolve_reasoning_effort(
    body: &Value,
    subscription_effort: Option<&str>,
    yaml_default_effort: Option<&str>,
) -> Option<ReasoningEffort> {
    if let Some(s) = body
        .get("extra_body")
        .and_then(|x| x.get("reasoning_effort"))
        .and_then(|x| x.as_str())
    {
        if let Ok(v) = ReasoningEffort::from_str(s) {
            return Some(v);
        }
    }
    if let Some(thinking) = body.get("thinking") {
        if let Some(s) = thinking.get("effort").and_then(|x| x.as_str()) {
            if let Ok(v) = ReasoningEffort::from_str(s) {
                return Some(v);
            }
        }
        if let Some(bt) = thinking.get("budget_tokens").and_then(|x| x.as_u64()) {
            return Some(ReasoningEffort::from_budget_tokens(bt));
        }
    }
    if let Some(s) = subscription_effort.filter(|s| !s.is_empty()) {
        if let Ok(v) = ReasoningEffort::from_str(s) {
            return Some(v);
        }
    }
    if let Some(s) = yaml_default_effort.filter(|s| !s.is_empty()) {
        if let Ok(v) = ReasoningEffort::from_str(s) {
            return Some(v);
        }
    }
    None
}

/// 非流式 (client stream=false 上游也 stream=false) 的 JSON-to-JSON 翻译.
///
/// OpenAI Responses stream=false 返回:
/// ```json
/// {
///   "id": "resp_xxx", "object": "response", "status": "completed", "model": "...",
///   "output": [
///     {"id": "rs_xxx", "type": "reasoning", "encrypted_content": "...", "summary": [...]},
///     {"id": "msg_xxx", "type": "message", "content": [{"type": "output_text", "text": "..."}]}
///   ],
///   "usage": {"input_tokens": N, "output_tokens": N, "output_tokens_details": {"reasoning_tokens": N}}
/// }
/// ```
///
/// 翻译成 Anthropic Messages JSON. config.emit_reasoning=true 时 reasoning item 会变成
/// Anthropic thinking content_block, 否则 skip。
pub fn responses_json_to_anthropic(
    upstream_body: &Value,
    config: &ResponsesTransformConfig,
) -> AppResult<Value> {
    let id = upstream_body
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("msg_unknown")
        .to_string();
    let model = upstream_body
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let status = upstream_body
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("completed");

    let mut content: Vec<Value> = Vec::new();
    if let Some(output) = upstream_body.get("output").and_then(|v| v.as_array()) {
        for item in output {
            if let Some(block) = output_item_to_content_block(item, config) {
                content.push(block);
            }
        }
    }

    let usage = upstream_body
        .get("usage")
        .cloned()
        .unwrap_or_else(|| json!({"input_tokens": 0, "output_tokens": 0}));
    let stop_reason = responses_common::map_status_to_anthropic_stop_reason(status);

    Ok(json!({
        "id": id,
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": usage,
    }))
}

/// 把 OpenAI Responses `output[]` 里的单个 item 转成 Anthropic content_block。
fn output_item_to_content_block(item: &Value, config: &ResponsesTransformConfig) -> Option<Value> {
    let item_type = item.get("type").and_then(|v| v.as_str())?;
    match item_type {
        "message" => {
            let content_arr = item.get("content").and_then(|v| v.as_array())?;
            let text: String = content_arr
                .iter()
                .filter_map(|c| {
                    let t = c.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if t == "output_text" {
                        c.get("text").and_then(|v| v.as_str()).map(str::to_string)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("");
            Some(json!({"type": "text", "text": text}))
        }
        "function_call" => {
            let call_id = item.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args_str = item.get("arguments").and_then(|v| v.as_str()).unwrap_or("{}");
            let input: Value = serde_json::from_str(args_str).unwrap_or(json!({}));
            Some(json!({
                "type": "tool_use",
                "id": call_id,
                "name": name,
                "input": input,
            }))
        }
        "reasoning" if config.emit_reasoning => {
            let summary_text = item
                .get("summary")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|s| s.get("text").and_then(|t| t.as_str()).map(str::to_string))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let ec = item
                .get("encrypted_content")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let signature = if ec.is_empty() {
                String::new()
            } else {
                encode_reasoning_signature(id, ec)
            };
            Some(json!({
                "type": "thinking",
                "thinking": summary_text,
                "signature": signature,
            }))
        }
        _ => None,
    }
}

// ============================================================
// 单测
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_entry_uses_official_config() {
        let body = json!({
            "model": "gpt-5",
            "max_tokens": 100,
            "stream": false,
            "messages": [{"role": "user", "content": "hi"}],
        });
        let out = anthropic_to_openai_responses(&body, &OpenAiResponsesExtras::default()).unwrap();
        // openai 路径与 codex 关键差异
        assert_eq!(out["stream"], json!(false), "跟随客户端");
        assert_eq!(out["max_output_tokens"], json!(100), "映射而非 drop");
        assert!(out.get("max_tokens").is_none());
        assert!(out.get("instructions").is_none(), "无 system 不注入");
        assert!(out.get("include").is_none(), "默认不开 reasoning include");
    }

    #[test]
    fn openai_entry_injects_reasoning_effort() {
        let body = json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "hi"}],
        });
        let extras = OpenAiResponsesExtras {
            reasoning_effort: Some(ReasoningEffort::High),
            expose_reasoning: false,
        };
        let out = anthropic_to_openai_responses(&body, &extras).unwrap();
        assert_eq!(out["reasoning"]["effort"], json!("high"));
    }

    #[test]
    fn openai_entry_with_expose_reasoning_injects_include() {
        let body = json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "hi"}],
        });
        let extras = OpenAiResponsesExtras {
            reasoning_effort: None,
            expose_reasoning: true,
        };
        let out = anthropic_to_openai_responses(&body, &extras).unwrap();
        assert_eq!(out["include"], json!(["reasoning.encrypted_content"]));
    }

    #[test]
    fn expose_reasoning_roundtrip_input_items() {
        // 用 expose_reasoning=true 让 config.roundtrip_reasoning=true,
        // 然后传一个含 thinking content_block 的 messages, 验证 input 里有 reasoning item
        use super::super::responses_common::{
            anthropic_messages_to_input, encode_reasoning_signature, ResponsesTransformConfig,
        };
        let mut config = ResponsesTransformConfig::openai_official(false);
        config.roundtrip_reasoning = true;

        let sig = encode_reasoning_signature("rs_abc", "ENC_BYTES");
        let msgs = vec![json!({
            "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "summary text", "signature": sig},
                {"type": "text", "text": "final answer"}
            ]
        })];
        let input = anthropic_messages_to_input(&msgs, &config).unwrap();
        // 期望: reasoning item + message item
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["type"], "reasoning");
        assert_eq!(input[0]["id"], "rs_abc");
        assert_eq!(input[0]["encrypted_content"], "ENC_BYTES");
        assert_eq!(input[0]["summary"][0]["text"], "summary text");
        assert_eq!(input[1]["type"], "message");
        assert_eq!(input[1]["content"][0]["text"], "final answer");
    }

    #[test]
    fn roundtrip_disabled_drops_thinking_block() {
        use super::super::responses_common::{anthropic_messages_to_input, ResponsesTransformConfig};
        let config = ResponsesTransformConfig::openai_official(false); // roundtrip_reasoning=false
        let msgs = vec![json!({
            "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "should drop", "signature": "anything"},
                {"type": "text", "text": "answer"}
            ]
        })];
        let input = anthropic_messages_to_input(&msgs, &config).unwrap();
        // thinking 被 drop, 只剩 message
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["type"], "message");
    }

    #[test]
    fn json_to_anthropic_text_message() {
        let upstream = json!({
            "id": "resp_1",
            "model": "gpt-5",
            "status": "completed",
            "output": [
                {"type": "message", "content": [
                    {"type": "output_text", "text": "Hello"},
                    {"type": "output_text", "text": " world"}
                ]}
            ],
            "usage": {"input_tokens": 5, "output_tokens": 2}
        });
        let cfg = ResponsesTransformConfig::openai_official(false);
        let out = responses_json_to_anthropic(&upstream, &cfg).unwrap();
        assert_eq!(out["id"], json!("resp_1"));
        assert_eq!(out["model"], json!("gpt-5"));
        assert_eq!(out["stop_reason"], json!("end_turn"));
        let content = out["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "Hello world");
        assert_eq!(out["usage"]["input_tokens"], 5);
    }

    #[test]
    fn json_to_anthropic_tool_use() {
        let upstream = json!({
            "id": "resp_2",
            "model": "gpt-5",
            "status": "completed",
            "output": [
                {"type": "function_call", "call_id": "call_a", "name": "get_weather",
                 "arguments": "{\"city\":\"Tokyo\"}"}
            ],
            "usage": {}
        });
        let cfg = ResponsesTransformConfig::openai_official(false);
        let out = responses_json_to_anthropic(&upstream, &cfg).unwrap();
        let content = out["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "tool_use");
        assert_eq!(content[0]["id"], "call_a");
        assert_eq!(content[0]["name"], "get_weather");
        assert_eq!(content[0]["input"], json!({"city": "Tokyo"}));
    }

    #[test]
    fn json_to_anthropic_skips_reasoning_by_default() {
        let upstream = json!({
            "id": "r", "model": "gpt-5", "status": "completed",
            "output": [
                {"type": "reasoning", "id": "rs_1", "encrypted_content": "ENC", "summary": []},
                {"type": "message", "content": [{"type": "output_text", "text": "answer"}]}
            ],
            "usage": {}
        });
        let cfg = ResponsesTransformConfig::openai_official(false); // emit_reasoning=false
        let out = responses_json_to_anthropic(&upstream, &cfg).unwrap();
        let content = out["content"].as_array().unwrap();
        assert_eq!(content.len(), 1, "reasoning skip, 只剩 text");
        assert_eq!(content[0]["type"], "text");
    }

    #[test]
    fn json_to_anthropic_emits_thinking_when_enabled() {
        let upstream = json!({
            "id": "r", "model": "gpt-5", "status": "completed",
            "output": [
                {"type": "reasoning", "id": "rs_1", "encrypted_content": "ENC", "summary": [{"type":"summary_text","text":"thinking..."}]},
                {"type": "message", "content": [{"type": "output_text", "text": "answer"}]}
            ],
            "usage": {}
        });
        let mut cfg = ResponsesTransformConfig::openai_official(false);
        cfg.emit_reasoning = true;
        let out = responses_json_to_anthropic(&upstream, &cfg).unwrap();
        let content = out["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["thinking"], "thinking...");
        assert!(!content[0]["signature"].as_str().unwrap().is_empty());
        assert_eq!(content[1]["type"], "text");
    }

    #[test]
    fn budget_tokens_to_effort_boundaries() {
        assert_eq!(ReasoningEffort::from_budget_tokens(0), ReasoningEffort::Minimal);
        assert_eq!(ReasoningEffort::from_budget_tokens(4095), ReasoningEffort::Minimal);
        assert_eq!(ReasoningEffort::from_budget_tokens(4096), ReasoningEffort::Low);
        assert_eq!(ReasoningEffort::from_budget_tokens(16383), ReasoningEffort::Low);
        assert_eq!(ReasoningEffort::from_budget_tokens(16384), ReasoningEffort::Medium);
        assert_eq!(ReasoningEffort::from_budget_tokens(65535), ReasoningEffort::Medium);
        assert_eq!(ReasoningEffort::from_budget_tokens(65536), ReasoningEffort::High);
        assert_eq!(ReasoningEffort::from_budget_tokens(200_000), ReasoningEffort::High);
    }

    #[test]
    fn resolve_reasoning_effort_priority_chain() {
        // 1) extra_body.reasoning_effort 最高优先级
        let body1 = json!({
            "extra_body": {"reasoning_effort": "high"},
            "thinking": {"effort": "low", "budget_tokens": 100},
        });
        assert_eq!(
            resolve_reasoning_effort(&body1, Some("medium"), Some("minimal")),
            Some(ReasoningEffort::High)
        );

        // 2) thinking.effort 次之
        let body2 = json!({"thinking": {"effort": "low", "budget_tokens": 100}});
        assert_eq!(
            resolve_reasoning_effort(&body2, Some("medium"), Some("minimal")),
            Some(ReasoningEffort::Low)
        );

        // 3) thinking.budget_tokens 自动映射
        let body3 = json!({"thinking": {"budget_tokens": 20000}});
        assert_eq!(
            resolve_reasoning_effort(&body3, Some("low"), Some("minimal")),
            Some(ReasoningEffort::Medium)
        );

        // 4) 订阅级
        let body4 = json!({});
        assert_eq!(
            resolve_reasoning_effort(&body4, Some("low"), Some("minimal")),
            Some(ReasoningEffort::Low)
        );

        // 5) yaml 默认
        let body5 = json!({});
        assert_eq!(
            resolve_reasoning_effort(&body5, None, Some("minimal")),
            Some(ReasoningEffort::Minimal)
        );

        // 6) 非法字符串视为缺失, 继续往下找
        let body6 = json!({"thinking": {"effort": "bogus"}});
        assert_eq!(
            resolve_reasoning_effort(&body6, None, Some("medium")),
            Some(ReasoningEffort::Medium)
        );

        // 7) 全 None
        let body7 = json!({});
        assert_eq!(resolve_reasoning_effort(&body7, None, None), None);
    }
}
