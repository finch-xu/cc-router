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
//! - `reasoning.effort` 透传, 优先级链: 订阅槽位级强制值 / 客户端 extra_body / 客户端
//!   output_config.effort / 客户端 thinking.effort / thinking.budget_tokens 自动映射 / yaml 默认
//!   (见 [`resolve_reasoning_effort`])

use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::AppResult;

use super::responses_common::{
    self, encode_reasoning_signature, ResponsesTransformConfig,
};

/// OpenAI Responses `reasoning.effort` 枚举. 接受 6 个输入字符串, 内部 6 个 variant
/// 一一对应, 全部原样透传上游。
///
/// 取值矩阵:
/// - `minimal`: OpenAI GPT-5+ 专用 (低 reasoning tokens), Anthropic 不接受
/// - `low/medium/high`: 两边都接受
/// - `xhigh`: OpenAI GPT-5.2+ / Anthropic Opus 4.7 共同支持
/// - `max`: 两边最高档 (OpenAI gpt-5.6 系起官方支持, 此前会 400 → 曾饱和到 Xhigh,
///   现原样透传; 老模型/不认 max 的中转由上游报错, 属调用方责任)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ReasoningEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }

    /// Anthropic deprecated `thinking.budget_tokens` (integer) → OpenAI effort. 阈值与 gpt-5
    /// 系列 reasoning_tokens 经验值对齐。
    ///
    /// 注: 不映射到 Xhigh / Max — 最高两档仅通过显式 effort string 触发, budget_tokens
    /// 这条路径在 Opus 4.7 已被弃用, 维持现有 4 档够用。
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
            "xhigh" => Ok(Self::Xhigh),
            "max" => Ok(Self::Max),
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

/// 读 Claude Code 2.1+ 的原生 effort 字段 `body.output_config.effort`.
///
/// 协议事实: CC 把 effort 放进 `output_config.effort` (取值 low|medium|high|xhigh|max,
/// 配 beta header `effort-2025-11-24`), 这是 Anthropic 官方的 effort 字段位置 —— 既不是顶层
/// `effort` 也不是 `thinking.effort`。cc-router 此前完全没读这个字段, 导致所有翻译类
/// provider (codex/openai/gemini) 拿不到客户端 effort, 全部落到 yaml 默认值。
///
/// 三个 resolver — 本文件 [`resolve_reasoning_effort`]、[`super::gemini::resolve_thinking_budget`]、
/// [`super::gemini_interactions::resolve_thinking_level`] — 共用本 accessor, 字段名只写一处。
/// 只取值不校验, 各 resolver 自己按目标协议映射, 非法值继续往链下落。
pub fn output_config_effort(body: &Value) -> Option<&str> {
    body.get("output_config")
        .and_then(|x| x.get("effort"))
        .and_then(|x| x.as_str())
}

/// reasoning effort 优先级链解析. 返回应填入 OpenAI request body 的 effort 值 (None 表示不传)。
///
/// 优先级 (高 → 低):
/// 0. `forced_effort` — **订阅槽位级强制值** (用户在 UI 给「该订阅的该槽位」固定了档位,
///    例: 只接受 max 的模型)。一旦有值就**丢弃客户端 body 里的所有 effort 信号**。
/// 1. `body.extra_body.reasoning_effort` (string) — cc-router 自家 escape hatch,
///    显式高于 CC 原生字段, 让高级用户能按请求覆盖。
/// 2. `body.output_config.effort` (string) — Claude Code 2.1+ 原生字段, 见 [`output_config_effort`]
/// 3. `body.thinking.effort` (string) — 老客户端 / 其他客户端写法
/// 4. `body.thinking.budget_tokens` (integer, 自动映射, 阈值见 [`ReasoningEffort::from_budget_tokens`])
/// 5. `yaml_default_effort` (provider yaml `default_reasoning_effort`)
///
/// 任意一档字符串非法都视为缺失, 继续往下找 (含 `forced_effort` — 脏 DB 数据不该让请求失效)。
///
/// 注意第 2 层必须排在第 4 层**之前**: 第 4 层是无条件 `return Some(...)`, 任何排在它后面的
/// 来源都会被永久遮蔽 (CC 开思考时历来同时发 `thinking.budget_tokens`)。
pub fn resolve_reasoning_effort(
    body: &Value,
    forced_effort: Option<&str>,
    yaml_default_effort: Option<&str>,
) -> Option<ReasoningEffort> {
    if let Some(s) = forced_effort.filter(|s| !s.is_empty()) {
        if let Ok(v) = ReasoningEffort::from_str(s) {
            return Some(v);
        }
    }
    if let Some(s) = body
        .get("extra_body")
        .and_then(|x| x.get("reasoning_effort"))
        .and_then(|x| x.as_str())
    {
        if let Ok(v) = ReasoningEffort::from_str(s) {
            return Some(v);
        }
    }
    if let Some(s) = output_config_effort(body) {
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

    /// body 内部各来源的相对优先级 (forced 一律传 None, 强制语义由下面几个测试单独覆盖)。
    #[test]
    fn resolve_reasoning_effort_priority_chain() {
        // 1) extra_body.reasoning_effort 最高优先级 (高于 CC 原生 output_config)
        let body1 = json!({
            "extra_body": {"reasoning_effort": "high"},
            "output_config": {"effort": "low"},
            "thinking": {"effort": "low", "budget_tokens": 100},
        });
        assert_eq!(
            resolve_reasoning_effort(&body1, None, Some("minimal")),
            Some(ReasoningEffort::High)
        );

        // 2) output_config.effort 次之 (CC 2.1+ 原生字段)
        let body2 = json!({
            "output_config": {"effort": "xhigh"},
            "thinking": {"effort": "low", "budget_tokens": 100},
        });
        assert_eq!(
            resolve_reasoning_effort(&body2, None, Some("minimal")),
            Some(ReasoningEffort::Xhigh)
        );

        // 3) thinking.effort 再次之
        let body3 = json!({"thinking": {"effort": "low", "budget_tokens": 100}});
        assert_eq!(
            resolve_reasoning_effort(&body3, None, Some("minimal")),
            Some(ReasoningEffort::Low)
        );

        // 4) thinking.budget_tokens 自动映射
        let body4 = json!({"thinking": {"budget_tokens": 20000}});
        assert_eq!(
            resolve_reasoning_effort(&body4, None, Some("minimal")),
            Some(ReasoningEffort::Medium)
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

    #[test]
    fn resolve_reasoning_effort_reads_output_config() {
        // 本次补漏的核心: CC 2.1+ 把 effort 放在 output_config.effort, 此前完全没读。
        let body = json!({"output_config": {"effort": "xhigh"}});
        assert_eq!(
            resolve_reasoning_effort(&body, None, Some("medium")),
            Some(ReasoningEffort::Xhigh)
        );
    }

    /// 回归哨兵: `thinking.budget_tokens` 那一层是无条件 `return Some(...)`,
    /// 谁把 output_config 挪到它后面, 这条就会红。
    #[test]
    fn output_config_effort_not_shadowed_by_budget_tokens() {
        let body = json!({
            "output_config": {"effort": "max"},
            "thinking": {"type": "enabled", "budget_tokens": 1024},
        });
        assert_eq!(
            resolve_reasoning_effort(&body, None, None),
            Some(ReasoningEffort::Max)
        );
    }

    /// 需求「用户设定后丢弃传入值」的直接断言。
    #[test]
    fn forced_effort_overrides_all_body_sources() {
        let body = json!({
            "extra_body": {"reasoning_effort": "low"},
            "output_config": {"effort": "low"},
            "thinking": {"effort": "low", "budget_tokens": 1024},
        });
        assert_eq!(
            resolve_reasoning_effort(&body, Some("max"), Some("minimal")),
            Some(ReasoningEffort::Max)
        );
    }

    #[test]
    fn forced_effort_empty_string_is_ignored() {
        // 前端 auto 的 sentinel 是 "", 不能被当成「强制成空」。
        let body = json!({"output_config": {"effort": "high"}});
        assert_eq!(
            resolve_reasoning_effort(&body, Some(""), None),
            Some(ReasoningEffort::High)
        );
    }

    #[test]
    fn forced_effort_invalid_falls_through_to_body() {
        // 脏 DB 数据不该让请求失效。
        let body = json!({"output_config": {"effort": "high"}});
        assert_eq!(
            resolve_reasoning_effort(&body, Some("bogus"), None),
            Some(ReasoningEffort::High)
        );
    }

    #[test]
    fn invalid_output_config_effort_falls_through() {
        let body = json!({
            "output_config": {"effort": "bogus"},
            "thinking": {"effort": "high"},
        });
        assert_eq!(
            resolve_reasoning_effort(&body, None, None),
            Some(ReasoningEffort::High)
        );
    }

    #[test]
    fn output_config_non_object_is_ignored() {
        // accessor 不能 panic, 也不能吞掉后面的来源。
        let body = json!({"output_config": 42, "thinking": {"effort": "low"}});
        assert_eq!(
            resolve_reasoning_effort(&body, None, None),
            Some(ReasoningEffort::Low)
        );
    }

    // ============================================================
    // xhigh / max 档位测试 (xhigh: OpenAI 5.2+; max: gpt-5.6 系起官方支持, 原样透传)
    // ============================================================

    #[test]
    fn reasoning_effort_xhigh_parses_to_xhigh_variant() {
        assert_eq!(
            ReasoningEffort::from_str("xhigh"),
            Ok(ReasoningEffort::Xhigh)
        );
    }

    #[test]
    fn reasoning_effort_max_parses_to_max_variant() {
        // OpenAI 官方已支持 max (gpt-5.6 系起), 不再饱和到 Xhigh, 原样透传
        assert_eq!(ReasoningEffort::from_str("max"), Ok(ReasoningEffort::Max));
    }

    #[test]
    fn reasoning_effort_top_tiers_serialize_as_str() {
        assert_eq!(ReasoningEffort::Xhigh.as_str(), "xhigh");
        assert_eq!(ReasoningEffort::Max.as_str(), "max");
    }

    #[test]
    fn resolve_reasoning_effort_xhigh_no_longer_dropped() {
        // 修复前: 客户端传 xhigh → silent drop → 落回 yaml medium (反而降级)
        // 修复后: 客户端传 xhigh → Xhigh variant, 原样透传给 OpenAI 上游
        let body = json!({"extra_body": {"reasoning_effort": "xhigh"}});
        assert_eq!(
            resolve_reasoning_effort(&body, None, Some("medium")),
            Some(ReasoningEffort::Xhigh)
        );
    }

    #[test]
    fn resolve_reasoning_effort_max_passes_through() {
        // 客户端传 max → Max variant 原样透传, 不落回 yaml 也不降档
        let body = json!({"thinking": {"effort": "max"}});
        assert_eq!(
            resolve_reasoning_effort(&body, None, Some("medium")),
            Some(ReasoningEffort::Max)
        );
    }
}
