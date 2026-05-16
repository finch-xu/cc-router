//! Anthropic 协议透传分支的请求体 sanitize 工具.
//!
//! 多 provider 轮询场景下, 客户端 (Claude Code) 多轮回灌的 `messages[].content` 里可能
//! 携带上一轮某个 cc-router 翻译层 (openai_responses / gemini) 包装过的 thinking block。
//! 这些 block 的 `signature` 是 cc-router 内部格式, 真正的 Anthropic 协议 provider
//! (xiaomi/deepseek/zhipu/anthropic/minimax/moonshot/alibaba 等) 无法识别, 透传会触发上游 400
//! ("thinking/reasoning_content must be passed back to the API")。
//!
//! 本模块负责在 [`pipeline`](super::pipeline) Anthropic 透传分支序列化 body 之前剥离这些
//! foreign thinking blocks, 保留空 signature 或真 Anthropic 原生 signature 的 block。

use serde_json::Value;

use crate::proxy::transform::responses_common::looks_like_cc_router_signature;

/// 剥离 cc-router 自家翻译层包装过的 thinking blocks. 本函数原地修改 body.
///
/// 判定: 任何 `type == "thinking"` 且 signature 能被 [`looks_like_cc_router_signature`] 识别
/// (即 openai_responses 或 gemini 包装) 的 block → drop。空 signature / Anthropic 原生 UUID
/// signature / 其他无法识别 → 保留 (上游各自校验)。
///
/// 返回值: drop 的 block 数, 0 表示没动 body, 调用方可用于日志。
pub fn strip_foreign_thinking_blocks(body: &mut Value) -> usize {
    let Some(msgs) = body.get_mut("messages").and_then(|m| m.as_array_mut()) else {
        return 0;
    };
    let mut dropped = 0usize;
    for msg in msgs.iter_mut() {
        let Some(content) = msg.get_mut("content").and_then(|c| c.as_array_mut()) else {
            continue;
        };
        content.retain(|blk| {
            if blk.get("type").and_then(|t| t.as_str()) != Some("thinking") {
                return true;
            }
            let sig = blk.get("signature").and_then(|v| v.as_str()).unwrap_or("");
            if looks_like_cc_router_signature(sig).is_some() {
                dropped += 1;
                return false;
            }
            true
        });
    }
    dropped
}

/// 给 messages 数组里每个 `role: assistant` 消息检查: 若 content 数组没有任何 thinking
/// content_block, 则在 content[0] 位置插入 placeholder `{type:"thinking", thinking:"", signature:""}`.
///
/// 用于 provider yaml `inject_missing_thinking_placeholder == true` 的兼容子集 provider
/// (当前只有 DeepSeek): DeepSeek 协议要求每个含 tool_use 的 assistant 消息必须有 thinking
/// block 开头, 否则触发 400 "thinking must be passed back to the API"。多 provider 轮询时
/// 由 GLM/anthropic 等不发 thinking 的 provider 生成的 assistant 消息回灌到 DeepSeek 时会
/// 触发该错误, 本函数补 placeholder 后实测 DeepSeek 200 接受。
///
/// 保守策略: 对所有缺 thinking 的 assistant 消息都补 (不区分是否含 tool_use), 因为空 thinking
/// placeholder 对纯 text assistant 也无副作用 (DeepSeek 实测忽略空 thinking)。
///
/// 返回值: 补充 placeholder 的消息数。
pub fn inject_missing_thinking_placeholders(body: &mut serde_json::Value) -> usize {
    use serde_json::json;

    let Some(msgs) = body.get_mut("messages").and_then(|m| m.as_array_mut()) else {
        return 0;
    };
    let mut injected = 0usize;
    for msg in msgs.iter_mut() {
        if msg.get("role").and_then(|r| r.as_str()) != Some("assistant") {
            continue;
        }
        let Some(content) = msg.get_mut("content").and_then(|c| c.as_array_mut()) else {
            // assistant 消息可能 content 是 string (纯文本), 这种情况 DeepSeek 内部接受, 不补
            continue;
        };
        let has_thinking = content
            .iter()
            .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("thinking"));
        if !has_thinking {
            content.insert(
                0,
                json!({ "type": "thinking", "thinking": "", "signature": "" }),
            );
            injected += 1;
        }
    }
    injected
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::transform::gemini::encode_gemini_thought_signature;
    use crate::proxy::transform::responses_common::encode_reasoning_signature;
    use serde_json::json;

    fn body_with_thinking(signature: &str, thinking_text: &str) -> Value {
        json!({
            "model": "claude-3-5-haiku-20241022",
            "messages": [
                {
                    "role": "user",
                    "content": [{ "type": "text", "text": "hi" }]
                },
                {
                    "role": "assistant",
                    "content": [
                        { "type": "thinking", "thinking": thinking_text, "signature": signature },
                        { "type": "text", "text": "response" }
                    ]
                }
            ]
        })
    }

    #[test]
    fn drops_openai_responses_wrapped() {
        let sig = encode_reasoning_signature("rs_abc", "encrypted_payload");
        let mut body = body_with_thinking(&sig, "let me think");
        let dropped = strip_foreign_thinking_blocks(&mut body);
        assert_eq!(dropped, 1);
        let assistant_content = body["messages"][1]["content"].as_array().unwrap();
        assert_eq!(assistant_content.len(), 1);
        assert_eq!(assistant_content[0]["type"], "text");
    }

    #[test]
    fn drops_gemini_wrapped() {
        let sig = encode_gemini_thought_signature("some_gemini_thought_sig_base64");
        let mut body = body_with_thinking(&sig, "");
        let dropped = strip_foreign_thinking_blocks(&mut body);
        assert_eq!(dropped, 1);
        assert_eq!(body["messages"][1]["content"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn keeps_empty_signature() {
        let mut body = body_with_thinking("", "some thinking");
        let dropped = strip_foreign_thinking_blocks(&mut body);
        assert_eq!(dropped, 0);
        assert_eq!(body["messages"][1]["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn keeps_anthropic_native_uuid_signature() {
        let mut body = body_with_thinking("03ea0953-5ece-4386-afea-31404f331c5f", "thought");
        let dropped = strip_foreign_thinking_blocks(&mut body);
        assert_eq!(dropped, 0);
        assert_eq!(body["messages"][1]["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn keeps_random_base64_that_is_not_cc_router_format() {
        // base64url 解码出非 JSON 内容 → 不被识别为 cc-router signature
        let mut body = body_with_thinking("YWJjZGVmZ2hpams", "x");
        let dropped = strip_foreign_thinking_blocks(&mut body);
        assert_eq!(dropped, 0);
    }

    #[test]
    fn handles_missing_messages_field() {
        let mut body = json!({ "model": "foo" });
        let dropped = strip_foreign_thinking_blocks(&mut body);
        assert_eq!(dropped, 0);
    }

    #[test]
    fn handles_string_content_messages() {
        // 历史消息 content 可能是 string (Anthropic 协议允许), 不应 panic
        let mut body = json!({
            "model": "foo",
            "messages": [
                { "role": "user", "content": "plain text user message" }
            ]
        });
        let dropped = strip_foreign_thinking_blocks(&mut body);
        assert_eq!(dropped, 0);
    }

    #[test]
    fn drops_multiple_across_multiple_messages() {
        let sig = encode_reasoning_signature("rs_1", "ec_1");
        let mut body = json!({
            "model": "foo",
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "q1" }] },
                {
                    "role": "assistant",
                    "content": [
                        { "type": "thinking", "thinking": "", "signature": sig.clone() },
                        { "type": "text", "text": "a1" }
                    ]
                },
                { "role": "user", "content": [{ "type": "text", "text": "q2" }] },
                {
                    "role": "assistant",
                    "content": [
                        { "type": "thinking", "thinking": "", "signature": sig.clone() },
                        { "type": "thinking", "thinking": "", "signature": "" },
                        { "type": "text", "text": "a2" }
                    ]
                }
            ]
        });
        let dropped = strip_foreign_thinking_blocks(&mut body);
        assert_eq!(dropped, 2);
        // 第二条 assistant 应该还剩 thinking(空 sig) + text 两块
        assert_eq!(body["messages"][3]["content"].as_array().unwrap().len(), 2);
    }

    // ============================================================
    // inject_missing_thinking_placeholders 测试 (修 DeepSeek 400)
    // ============================================================

    #[test]
    fn inject_adds_placeholder_to_assistant_missing_thinking() {
        let mut body = json!({
            "model": "deepseek-v4-flash",
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "q" }] },
                {
                    "role": "assistant",
                    "content": [
                        { "type": "text", "text": "calling tool" },
                        { "type": "tool_use", "id": "t1", "name": "echo", "input": {} }
                    ]
                },
                { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": "ok" }] }
            ]
        });
        let injected = inject_missing_thinking_placeholders(&mut body);
        assert_eq!(injected, 1);
        let assistant = body["messages"][1]["content"].as_array().unwrap();
        assert_eq!(assistant.len(), 3);
        assert_eq!(assistant[0]["type"], "thinking");
        assert_eq!(assistant[0]["thinking"], "");
        assert_eq!(assistant[0]["signature"], "");
        assert_eq!(assistant[1]["type"], "text");
        assert_eq!(assistant[2]["type"], "tool_use");
    }

    #[test]
    fn inject_skips_assistant_with_existing_thinking() {
        let mut body = json!({
            "model": "deepseek-v4-flash",
            "messages": [
                {
                    "role": "assistant",
                    "content": [
                        { "type": "thinking", "thinking": "I'm thinking", "signature": "abc" },
                        { "type": "text", "text": "hi" }
                    ]
                }
            ]
        });
        let injected = inject_missing_thinking_placeholders(&mut body);
        assert_eq!(injected, 0);
        assert_eq!(body["messages"][0]["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn inject_ignores_user_messages() {
        let mut body = json!({
            "model": "deepseek-v4-flash",
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "q" }] }
            ]
        });
        let injected = inject_missing_thinking_placeholders(&mut body);
        assert_eq!(injected, 0);
        // user 消息不应被加 thinking
        assert_eq!(body["messages"][0]["content"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn inject_skips_assistant_with_string_content() {
        // assistant content 可以是 string (Anthropic 协议允许), DeepSeek 内部接受, 不补
        let mut body = json!({
            "model": "deepseek-v4-flash",
            "messages": [
                { "role": "assistant", "content": "plain text reply" }
            ]
        });
        let injected = inject_missing_thinking_placeholders(&mut body);
        assert_eq!(injected, 0);
    }

    #[test]
    fn inject_handles_multiple_assistant_messages() {
        let mut body = json!({
            "model": "deepseek-v4-flash",
            "messages": [
                {
                    "role": "assistant",
                    "content": [{ "type": "text", "text": "no thinking 1" }]
                },
                { "role": "user", "content": [{ "type": "text", "text": "q" }] },
                {
                    "role": "assistant",
                    "content": [
                        { "type": "thinking", "thinking": "yes", "signature": "" },
                        { "type": "text", "text": "has thinking" }
                    ]
                },
                {
                    "role": "assistant",
                    "content": [{ "type": "tool_use", "id": "t1", "name": "x", "input": {} }]
                }
            ]
        });
        let injected = inject_missing_thinking_placeholders(&mut body);
        assert_eq!(injected, 2); // msg[0] 和 msg[3] 缺 thinking, msg[2] 不动
        assert_eq!(body["messages"][0]["content"][0]["type"], "thinking");
        assert_eq!(body["messages"][2]["content"][0]["type"], "thinking");
        assert_eq!(body["messages"][2]["content"][0]["thinking"], "yes"); // 已存在的不被覆盖
        assert_eq!(body["messages"][3]["content"][0]["type"], "thinking");
    }

    #[test]
    fn inject_handles_missing_messages_field() {
        let mut body = json!({ "model": "deepseek-v4-flash" });
        let injected = inject_missing_thinking_placeholders(&mut body);
        assert_eq!(injected, 0);
    }
}
