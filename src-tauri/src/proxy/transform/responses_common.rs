//! OpenAI Responses 协议翻译共享 helper.
//!
//! 这一层封装「Anthropic Messages ↔ OpenAI Responses」的纯协议映射, 不携带任何
//! provider 特定的 quirk (例如 chatgpt.com 反代强制 stream=true / strip max_tokens).
//! quirks 通过 [`ResponsesTransformConfig`] 集中表达, 由各个入口选择不同 preset:
//!
//! - `codex_chatgpt(expose_reasoning)` — ChatGPT 反代 (openai_codex provider, oauth_dispatch.rs 走它)
//! - `openai_official(expose_reasoning)` — 标准 OpenAI Responses (custom_openai / openai.yaml 走它)
//!
//! `expose_reasoning` 由 dispatch 层从 yaml `expose_reasoning` 字段读出, 控制是否暴露 reasoning
//! 块到客户端 + 是否回灌客户端 thinking 块到上游 input。
//!
//! ## 边界
//!
//! 本模块的函数都是纯函数 + 同步状态机, 不发起任何网络请求, 不读 DB / oauth metadata.
//! 各 provider 的 dispatch 层 (`oauth_dispatch.rs`, `openai_responses_dispatch.rs`) 负责
//! 调度、鉴权、headers 注入, 翻译层只关心协议字节。

use std::collections::HashMap;

use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

// ============================================================
// 配置
// ============================================================

/// 翻译层的所有 provider 特定 quirks 集中点.
///
/// 同一份代码服务 codex (chatgpt 反代) 和 openai (官方/中转) 两条路径, 行为差异通过本结构体配置.
#[derive(Debug, Clone)]
pub struct ResponsesTransformConfig {
    /// 是否强制把上游请求改成 stream=true (即使客户端要 stream=false).
    /// codex: true (chatgpt 反代不支持 stream=false). openai: false (跟随客户端).
    pub force_upstream_streaming: bool,

    /// 是否注入 `store: false` 到请求体. cc-router 不持久化历史, 一般是 true.
    pub inject_store_false: bool,

    /// 注入到请求体的 `include` 字段值. 空 vec 表示不注入.
    /// codex: `["reasoning.encrypted_content"]`. openai: 视 expose_reasoning 配置.
    pub inject_default_include: Vec<String>,

    /// 是否强制让 `instructions` 字段 present (即使没 system, 也注入空字符串).
    /// codex: true (chatgpt 反代 schema 要求). openai: false (官方可选).
    pub force_instructions_present: bool,

    /// 是否 drop Anthropic `max_tokens` 字段而不映射.
    /// codex: true (chatgpt 反代会 400 拒绝 max_output_tokens). openai: false (映射为 max_output_tokens).
    pub drop_max_tokens: bool,

    /// 上游 SSE 里出现 reasoning item 时, 是否翻译成 Anthropic thinking content_block.
    /// codex 默认 false (向后兼容, yaml opt-in 后开). openai 默认 true.
    pub emit_reasoning: bool,

    /// 客户端 messages 里出现 thinking content_block 时, 是否翻译成上游 input reasoning item.
    /// 与 emit_reasoning 配对使用; 关闭时 anthropic_messages_to_input 直接 drop thinking 块。
    pub roundtrip_reasoning: bool,
}

impl ResponsesTransformConfig {
    /// codex (ChatGPT 反代) 的强约束配置.
    ///
    /// `expose_reasoning`: 是否把上游 reasoning item 暴露成 Anthropic thinking content_block,
    /// 同时启用客户端 thinking 块 → 上游 reasoning input 的回灌。yaml `expose_reasoning` 字段控制。
    /// 注意 `inject_default_include` 始终保留 `reasoning.encrypted_content` —— chatgpt 反代
    /// `store=false` 下不发 include 会丢失 encrypted_content, 多轮回灌不可能。
    pub fn codex_chatgpt(expose_reasoning: bool) -> Self {
        Self {
            force_upstream_streaming: true,
            inject_store_false: true,
            inject_default_include: vec!["reasoning.encrypted_content".into()],
            force_instructions_present: true,
            drop_max_tokens: true,
            emit_reasoning: expose_reasoning,
            roundtrip_reasoning: expose_reasoning,
        }
    }

    /// OpenAI 官方 `/v1/responses` 的宽松配置.
    ///
    /// `expose_reasoning`: 同上语义。区别于 codex: openai 官方端口 `include` 字段视 expose 决定,
    /// 不暴露时不发 include 减少上游开销。
    pub fn openai_official(expose_reasoning: bool) -> Self {
        Self {
            force_upstream_streaming: false,
            inject_store_false: true,
            inject_default_include: if expose_reasoning {
                vec!["reasoning.encrypted_content".into()]
            } else {
                vec![]
            },
            force_instructions_present: false,
            drop_max_tokens: false,
            emit_reasoning: expose_reasoning,
            roundtrip_reasoning: expose_reasoning,
        }
    }
}

// ============================================================
// 请求转换
// ============================================================

/// 把 Anthropic Messages 请求体转成 OpenAI Responses 请求体, quirks 由 config 决定。
///
/// 入口 (`anthropic_to_responses` / `anthropic_to_openai_responses`) 调用本函数即可,
/// 不需要重复实现字段映射逻辑。
pub fn build_responses_body(body: &Value, config: &ResponsesTransformConfig) -> AppResult<Value> {
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::BadRequest("请求 body 缺少 model".into()))?;

    let mut out = json!({ "model": model });

    if config.force_upstream_streaming {
        out["stream"] = json!(true);
    } else if let Some(s) = body.get("stream") {
        // 跟随客户端
        out["stream"] = s.clone();
    }

    if config.inject_store_false {
        out["store"] = json!(false);
    }

    if !config.inject_default_include.is_empty() {
        out["include"] = json!(config.inject_default_include);
    }

    // system → instructions (语义差异见 force_instructions_present 字段文档)
    let system = body.get("system");
    let has_system = system.is_some();
    if config.force_instructions_present {
        let text = system.map(anthropic_system_to_text).unwrap_or_default();
        out["instructions"] = json!(text);
    } else if has_system {
        let text = anthropic_system_to_text(system.unwrap());
        if !text.is_empty() {
            out["instructions"] = json!(text);
        }
    }

    // messages[] → input[]
    if let Some(msgs) = body.get("messages").and_then(|m| m.as_array()) {
        let input = anthropic_messages_to_input(msgs, config)?;
        out["input"] = json!(input);
    }

    // tools[] → tools[]
    if let Some(tools) = body.get("tools").and_then(|t| t.as_array()) {
        let converted: Vec<Value> = tools.iter().filter_map(convert_tool).collect();
        if !converted.is_empty() {
            out["tools"] = json!(converted);
        }
    }

    // tool_choice
    if let Some(tc) = body.get("tool_choice") {
        if let Some(mapped) = map_tool_choice(tc) {
            out["tool_choice"] = mapped;
        }
    }

    // 透传字段
    for key in ["temperature", "top_p"] {
        if let Some(v) = body.get(key) {
            out[key] = v.clone();
        }
    }

    // max_tokens — codex 路径 drop, openai 路径映射为 max_output_tokens
    if !config.drop_max_tokens {
        if let Some(mt) = body.get("max_tokens") {
            out["max_output_tokens"] = mt.clone();
        }
    }
    // 注意: stop_sequences 不映射 — codex 反代和 OpenAI Responses 都不接受此参数

    Ok(out)
}

pub fn anthropic_system_to_text(system: &Value) -> String {
    if let Some(s) = system.as_str() {
        return s.to_string();
    }
    if let Some(arr) = system.as_array() {
        return arr
            .iter()
            .filter_map(|item| {
                // Anthropic system 数组里每个元素是 {"type": "text", "text": "..."}
                item.get("text").and_then(|t| t.as_str()).map(str::to_string)
            })
            .collect::<Vec<_>>()
            .join("\n\n");
    }
    String::new()
}

/// Anthropic messages 的 content 既可能是 str, 也可能是 [{type:..., ...}, ...].
/// 而 OpenAI Responses 的 input 是扁平化的 item 数组, 其中:
/// - 文本/图片消息 → {"type":"message", "role":"user|assistant", "content":[{type:"input_text",text}]}
/// - tool_use (assistant 调工具) → {"type":"function_call","call_id","name","arguments"}
/// - tool_result (user 给工具结果) → {"type":"function_call_output","call_id","output"}
/// - thinking (config.roundtrip_reasoning=true 时) → {"type":"reasoning","id","encrypted_content","summary"}
///   其中 id + encrypted_content 从 Anthropic signature 字段 base64url 解码得到。
pub fn anthropic_messages_to_input(
    msgs: &[Value],
    config: &ResponsesTransformConfig,
) -> AppResult<Vec<Value>> {
    let mut out: Vec<Value> = Vec::new();
    for m in msgs {
        let role = m.get("role").and_then(|v| v.as_str()).unwrap_or("user");
        let content = m.get("content");
        match content {
            Some(Value::String(text)) => {
                out.push(make_message_item(role, text));
            }
            Some(Value::Array(blocks)) => {
                let mut text_parts: Vec<Value> = Vec::new();
                for blk in blocks {
                    let blk_type = blk.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    match blk_type {
                        "text" => {
                            if let Some(t) = blk.get("text").and_then(|v| v.as_str()) {
                                text_parts.push(json!({
                                    "type": text_block_type(role),
                                    "text": t,
                                }));
                            }
                        }
                        "tool_use" => {
                            flush_text_parts(&mut out, role, &mut text_parts);
                            let call_id = blk.get("id").and_then(|v| v.as_str()).unwrap_or("");
                            let name = blk.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            let arguments = blk
                                .get("input")
                                .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "{}".into()))
                                .unwrap_or_else(|| "{}".into());
                            out.push(json!({
                                "type": "function_call",
                                "call_id": call_id,
                                "name": name,
                                "arguments": arguments,
                            }));
                        }
                        "tool_result" => {
                            flush_text_parts(&mut out, role, &mut text_parts);
                            let call_id =
                                blk.get("tool_use_id").and_then(|v| v.as_str()).unwrap_or("");
                            // tool_result.content 可能是 str 或 Anthropic content blocks
                            let output = match blk.get("content") {
                                Some(Value::String(s)) => s.clone(),
                                Some(Value::Array(arr)) => arr
                                    .iter()
                                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                                _ => String::new(),
                            };
                            out.push(json!({
                                "type": "function_call_output",
                                "call_id": call_id,
                                "output": output,
                            }));
                        }
                        "thinking" if config.roundtrip_reasoning => {
                            // 注意: 多轮长对话里每轮都会重解码全部历史 thinking 块 (N 轮 × M 块).
                            // 当前规模 (~50 块以内) 量级 µs 级可忽略; 超出后考虑改 signature 编码避免 JSON parse.
                            flush_text_parts(&mut out, role, &mut text_parts);
                            let signature =
                                blk.get("signature").and_then(|v| v.as_str()).unwrap_or("");
                            let summary_text =
                                blk.get("thinking").and_then(|v| v.as_str()).unwrap_or("");
                            let decoded = decode_reasoning_signature(signature);
                            match decoded {
                                Some((id, ec)) if !id.is_empty() && !ec.is_empty() => {
                                    let summary_payload = if summary_text.is_empty() {
                                        json!([])
                                    } else {
                                        json!([{"type": "summary_text", "text": summary_text}])
                                    };
                                    out.push(json!({
                                        "type": "reasoning",
                                        "id": id,
                                        "encrypted_content": ec,
                                        "summary": summary_payload,
                                    }));
                                }
                                _ => {
                                    // signature 缺失/损坏 — drop, 不阻塞请求
                                    tracing::warn!(
                                        "drop thinking content_block: signature 缺失或损坏, 多轮 reasoning 上下文将丢失"
                                    );
                                }
                            }
                        }
                        // 其余 (image / document / 配置关闭时的 thinking) 暂不支持: 直接忽略
                        _ => {}
                    }
                }
                flush_text_parts(&mut out, role, &mut text_parts);
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Anthropic thinking content_block 的 `signature` 字段编码 schema 版本号。
///
/// 版本 1: `base64url(JSON{v:1, id, ec})` — 历史格式, 无 provider tag。新 cc-router 仍能解
/// (向后兼容老 build 包装过的会话历史), 但**新写入**一律用 v=2。
///
/// 版本 2: `base64url(JSON{v:2, p:"openai_responses", id, ec})` — 加 `p` 字段防止
/// 跨翻译层错喂 (例如 cc-router 的 openai signature 被 dispatch 透传给 xiaomi/deepseek
/// 后上游 400)。与 [`gemini.rs::GEMINI_SIG_PROVIDER`] (`"gemini"`) 物理隔离。
const REASONING_SIG_VERSION: u64 = 2;
const REASONING_SIG_PROVIDER: &str = "openai_responses";

/// Anthropic thinking content_block 的 `signature` 字段编码方案:
/// `base64url(JSON{v:2, p:"openai_responses", id: "rs_xxx", ec: "<encrypted_content>"})`.
///
/// 流式 (`output_item.done(reasoning)`) 和非流式 (`responses_json_to_anthropic`) 路径
/// 写入 signature; 客户端回传后 [`anthropic_messages_to_input`] 解码还原成 input items 里的 reasoning item。
pub fn encode_reasoning_signature(id: &str, encrypted_content: &str) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    let payload = json!({
        "v": REASONING_SIG_VERSION,
        "p": REASONING_SIG_PROVIDER,
        "id": id,
        "ec": encrypted_content,
    });
    let json_str = serde_json::to_string(&payload).unwrap_or_default();
    URL_SAFE_NO_PAD.encode(json_str.as_bytes())
}

/// 与 [`encode_reasoning_signature`] 配对的解码器, 返回 (id, encrypted_content). 失败 → None。
///
/// 兼容性: v=1 (老 build 包装的, 无 `p` 字段) 仍接受; v=2 必须 `p == "openai_responses"` 才解。
/// 其他 v 或 p 不匹配 → None (例如 gemini wrap 喂进来会被拒)。
pub fn decode_reasoning_signature(signature: &str) -> Option<(String, String)> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    if signature.is_empty() {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(signature.as_bytes()).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    let ver = v.get("v").and_then(|x| x.as_u64())?;
    match ver {
        1 => {
            // 老 cc-router (v=1) 包装的 openai_responses signature 无 `p` 字段。但 gemini wrap 也是
            // v=1 且有 `p:"gemini"`, 必须显式拒绝以避免误识别为 openai_responses。
            if let Some(p) = v.get("p").and_then(|x| x.as_str()) {
                if p != REASONING_SIG_PROVIDER {
                    return None;
                }
            }
        }
        2 => {
            if v.get("p").and_then(|x| x.as_str()) != Some(REASONING_SIG_PROVIDER) {
                return None;
            }
        }
        _ => return None,
    }
    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let ec = v.get("ec").and_then(|x| x.as_str()).unwrap_or("").to_string();
    Some((id, ec))
}

/// 跨 transform 层的 signature 来源识别. 用于 Anthropic 协议透传分支 (xiaomi/deepseek/zhipu/
/// anthropic/minimax/moonshot/alibaba 等) 在 dispatch 前剥离 cc-router 自家包装的 thinking
/// block —— 这些 block 的 signature 是给 cc-router 内部翻译层用的, 上游 Anthropic 协议
/// provider 不认识, 透传会触发 400。
///
/// 返回 `Some("openai_responses")` 或 `Some("gemini")` 表示 cc-router 包装; 空串、Anthropic
/// 原生 UUID signature 或解码失败一律 `None`。
pub fn looks_like_cc_router_signature(signature: &str) -> Option<&'static str> {
    if signature.is_empty() {
        return None;
    }
    if decode_reasoning_signature(signature).is_some() {
        return Some("openai_responses");
    }
    if crate::proxy::transform::gemini::decode_gemini_thought_signature(signature).is_some() {
        return Some("gemini");
    }
    None
}

fn flush_text_parts(out: &mut Vec<Value>, role: &str, text_parts: &mut Vec<Value>) {
    if !text_parts.is_empty() {
        out.push(json!({
            "type": "message",
            "role": role,
            "content": std::mem::take(text_parts),
        }));
    }
}

pub fn make_message_item(role: &str, text: &str) -> Value {
    json!({
        "type": "message",
        "role": role,
        "content": [{ "type": text_block_type(role), "text": text }],
    })
}

/// OpenAI Responses 里区分 input_text (用户/工具结果) 和 output_text (助手回复).
pub fn text_block_type(role: &str) -> &'static str {
    if role == "assistant" {
        "output_text"
    } else {
        "input_text"
    }
}

pub fn convert_tool(t: &Value) -> Option<Value> {
    let name = t.get("name").and_then(|v| v.as_str())?;
    let description = t.get("description").cloned().unwrap_or(Value::Null);
    let schema = t.get("input_schema").cloned().unwrap_or(json!({}));
    Some(json!({
        "type": "function",
        "name": name,
        "description": description,
        "parameters": schema,
    }))
}

pub fn map_tool_choice(tc: &Value) -> Option<Value> {
    if let Some(s) = tc.as_str() {
        match s {
            "auto" | "any" | "none" => return Some(json!(s)),
            _ => return None,
        }
    }
    if let Some(obj) = tc.as_object() {
        if obj.get("type").and_then(|v| v.as_str()) == Some("tool") {
            if let Some(name) = obj.get("name").and_then(|v| v.as_str()) {
                return Some(json!({"type": "function", "name": name}));
            }
        }
    }
    None
}

// ============================================================
// 响应转换 (SSE 状态机)
// ============================================================

/// 上游 SSE 事件的解码视图.
#[derive(Debug)]
pub enum UpstreamEvent {
    Created(Value),
    Completed(Value),
    OutputItemAdded { output_index: u32, item: Value },
    OutputItemDone { output_index: u32, item: Value },
    OutputTextDelta { output_index: u32, delta: String },
    FunctionCallArgsDelta { output_index: u32, delta: String },
    /// reasoning summary 增量 (gpt-5.5 不发, o1/o3 等模型可能发).
    ReasoningSummaryTextDelta { output_index: u32, delta: String },
    /// 不关心或暂时忽略的事件 (in_progress / content_part.added / output_text.done 等)
    Ignored,
    /// 解析失败但不 fatal
    Unknown,
}

pub fn parse_upstream_event(event_name: &str, data: &Value) -> UpstreamEvent {
    match event_name {
        "response.created" => UpstreamEvent::Created(data.clone()),
        "response.completed" => UpstreamEvent::Completed(data.clone()),
        "response.output_item.added" => {
            let output_index = data.get("output_index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let item = data.get("item").cloned().unwrap_or(Value::Null);
            UpstreamEvent::OutputItemAdded { output_index, item }
        }
        "response.output_item.done" => {
            let output_index = data.get("output_index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let item = data.get("item").cloned().unwrap_or(Value::Null);
            UpstreamEvent::OutputItemDone { output_index, item }
        }
        "response.output_text.delta" => {
            let output_index = data.get("output_index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let delta = data
                .get("delta")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            UpstreamEvent::OutputTextDelta { output_index, delta }
        }
        "response.function_call_arguments.delta" => {
            let output_index = data.get("output_index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let delta = data
                .get("delta")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            UpstreamEvent::FunctionCallArgsDelta { output_index, delta }
        }
        "response.reasoning_summary_text.delta" => {
            let output_index = data.get("output_index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let delta = data
                .get("delta")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            UpstreamEvent::ReasoningSummaryTextDelta { output_index, delta }
        }
        "response.in_progress"
        | "response.content_part.added"
        | "response.content_part.done"
        | "response.output_text.done"
        | "response.function_call_arguments.done"
        | "response.reasoning_summary_part.added"
        | "response.reasoning_summary_part.done"
        | "response.reasoning_summary_text.done" => UpstreamEvent::Ignored,
        _ => UpstreamEvent::Unknown,
    }
}

/// Anthropic SSE 事件 (输出端).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum AnthropicEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: Value },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: u32,
        content_block: Value,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: u32, delta: Value },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: u32 },
    #[serde(rename = "message_delta")]
    MessageDelta { delta: Value, usage: Value },
    #[serde(rename = "message_stop")]
    MessageStop,
}

impl AnthropicEvent {
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::MessageStart { .. } => "message_start",
            Self::ContentBlockStart { .. } => "content_block_start",
            Self::ContentBlockDelta { .. } => "content_block_delta",
            Self::ContentBlockStop { .. } => "content_block_stop",
            Self::MessageDelta { .. } => "message_delta",
            Self::MessageStop => "message_stop",
        }
    }

    pub fn to_sse_frame(&self) -> String {
        let json = serde_json::to_string(self).unwrap_or_else(|_| "{}".into());
        format!("event: {}\ndata: {}\n\n", self.event_name(), json)
    }
}

/// 单个内容块 (Anthropic 视角) 的状态.
#[derive(Debug, Clone)]
pub(crate) struct ContentBlock {
    pub kind: BlockKind,
    /// Anthropic 侧 content_blocks 索引 (顺序分配, 与 OpenAI output_index 不同 — 跳过的 reasoning items 不占索引).
    /// `BlockKind::Skipped` 时该字段无意义。
    pub anthropic_index: u32,
}

#[derive(Debug, Clone)]
pub(crate) enum BlockKind {
    Text,
    ToolUse,
    /// reasoning 暴露 (config.emit_reasoning=true 时使用), 翻译为 Anthropic thinking content_block。
    /// payload 是 output_item.added 时拿到的 reasoning item id, 在 output_item.done 时一起编码进 signature。
    Reasoning { id: String },
    /// reasoning / 其他不暴露的类型 (config.emit_reasoning=false 时 reasoning 走这里)
    Skipped,
}

/// SSE 状态机. 调用方按 `\n\n` 切帧, 解 event_name + data,
/// 然后调用 `feed` 拿一组 (可能为 0) 待写出的 Anthropic 帧.
///
/// 通过 [`ResponsesTransformConfig`] 控制 reasoning 暴露等行为. 默认 [`Self::new`] 使用
/// `codex_chatgpt(false)` 配置 (向后兼容 oauth_dispatch.rs); openai 路径用 [`Self::new_with_config`].
pub struct ResponsesSseConverter {
    config: ResponsesTransformConfig,
    started: bool,
    pub(crate) message_id: String,
    pub(crate) response_model: String,
    next_anthropic_index: u32,
    blocks: HashMap<u32, ContentBlock>,
    pub(crate) final_usage: Option<Value>,
    stopped: bool,
}

impl Default for ResponsesSseConverter {
    fn default() -> Self {
        Self::new()
    }
}

impl ResponsesSseConverter {
    /// 默认配置: codex chatgpt 反代 + expose_reasoning=false (维持历史行为).
    /// 真实 dispatch 路径都走 [`Self::new_with_config`] 显式传配置, 此构造仅为测试便利。
    pub fn new() -> Self {
        Self::new_with_config(ResponsesTransformConfig::codex_chatgpt(false))
    }

    pub fn new_with_config(config: ResponsesTransformConfig) -> Self {
        Self {
            config,
            started: false,
            message_id: String::new(),
            response_model: String::new(),
            next_anthropic_index: 0,
            blocks: HashMap::new(),
            final_usage: None,
            stopped: false,
        }
    }

    pub fn feed(&mut self, event_name: &str, data: &Value) -> Vec<AnthropicEvent> {
        let parsed = parse_upstream_event(event_name, data);
        match parsed {
            UpstreamEvent::Created(resp) => self.handle_created(&resp),
            UpstreamEvent::OutputItemAdded { output_index, item } => {
                self.handle_output_item_added(output_index, &item)
            }
            UpstreamEvent::OutputItemDone { output_index, item } => {
                self.handle_output_item_done(output_index, &item)
            }
            UpstreamEvent::OutputTextDelta { output_index, delta } => {
                self.handle_output_text_delta(output_index, &delta)
            }
            UpstreamEvent::FunctionCallArgsDelta { output_index, delta } => {
                self.handle_function_args_delta(output_index, &delta)
            }
            UpstreamEvent::ReasoningSummaryTextDelta { output_index, delta } => {
                self.handle_reasoning_summary_text_delta(output_index, &delta)
            }
            UpstreamEvent::Completed(resp) => self.handle_completed(&resp),
            UpstreamEvent::Ignored | UpstreamEvent::Unknown => Vec::new(),
        }
    }

    /// 流结束兜底: 没收到 response.completed 时, 至少 emit message_stop.
    pub fn finalize_if_needed(&mut self) -> Vec<AnthropicEvent> {
        if !self.stopped && self.started {
            self.stopped = true;
            return vec![AnthropicEvent::MessageStop];
        }
        Vec::new()
    }

    pub fn response_model(&self) -> &str {
        &self.response_model
    }

    // ---------- handlers ----------

    fn handle_created(&mut self, resp: &Value) -> Vec<AnthropicEvent> {
        if self.started {
            return Vec::new();
        }
        self.started = true;
        self.message_id = resp
            .get("response")
            .and_then(|r| r.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("msg_unknown")
            .to_string();
        let model = resp
            .get("response")
            .and_then(|r| r.get("model"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !model.is_empty() {
            self.response_model = model.to_string();
        }

        let message = json!({
            "id": self.message_id,
            "type": "message",
            "role": "assistant",
            "model": self.response_model,
            "content": [],
            "stop_reason": null,
            "stop_sequence": null,
            "usage": {
                "input_tokens": 0,
                "output_tokens": 0,
            }
        });
        vec![AnthropicEvent::MessageStart { message }]
    }

    fn handle_output_item_added(
        &mut self,
        output_index: u32,
        item: &Value,
    ) -> Vec<AnthropicEvent> {
        let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match item_type {
            "message" => {
                let anthropic_index = self.next_anthropic_index;
                self.next_anthropic_index += 1;
                self.blocks.insert(
                    output_index,
                    ContentBlock { kind: BlockKind::Text, anthropic_index },
                );
                vec![AnthropicEvent::ContentBlockStart {
                    index: anthropic_index,
                    content_block: json!({"type": "text", "text": ""}),
                }]
            }
            "function_call" => {
                let anthropic_index = self.next_anthropic_index;
                self.next_anthropic_index += 1;
                self.blocks.insert(
                    output_index,
                    ContentBlock { kind: BlockKind::ToolUse, anthropic_index },
                );
                let call_id = item
                    .get("call_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("call_unknown");
                let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                vec![AnthropicEvent::ContentBlockStart {
                    index: anthropic_index,
                    content_block: json!({
                        "type": "tool_use",
                        "id": call_id,
                        "name": name,
                        "input": {},
                    }),
                }]
            }
            "reasoning" if self.config.emit_reasoning => {
                let anthropic_index = self.next_anthropic_index;
                self.next_anthropic_index += 1;
                let id =
                    item.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                self.blocks.insert(
                    output_index,
                    ContentBlock {
                        kind: BlockKind::Reasoning { id },
                        anthropic_index,
                    },
                );
                vec![AnthropicEvent::ContentBlockStart {
                    index: anthropic_index,
                    content_block: json!({"type": "thinking", "thinking": ""}),
                }]
            }
            // reasoning (emit_reasoning=false) / 其他类型 → skip, 不占索引
            _ => {
                self.blocks.insert(
                    output_index,
                    ContentBlock {
                        kind: BlockKind::Skipped,
                        anthropic_index: 0,
                    },
                );
                Vec::new()
            }
        }
    }

    fn handle_reasoning_summary_text_delta(
        &mut self,
        output_index: u32,
        delta: &str,
    ) -> Vec<AnthropicEvent> {
        let Some(block) = self.blocks.get(&output_index) else {
            return Vec::new();
        };
        if !matches!(block.kind, BlockKind::Reasoning { .. }) {
            return Vec::new();
        }
        if delta.is_empty() {
            return Vec::new();
        }
        vec![AnthropicEvent::ContentBlockDelta {
            index: block.anthropic_index,
            delta: json!({"type": "thinking_delta", "thinking": delta}),
        }]
    }

    fn handle_output_text_delta(
        &mut self,
        output_index: u32,
        delta: &str,
    ) -> Vec<AnthropicEvent> {
        let Some(block) = self.blocks.get(&output_index) else {
            return Vec::new();
        };
        if !matches!(block.kind, BlockKind::Text) {
            return Vec::new();
        }
        vec![AnthropicEvent::ContentBlockDelta {
            index: block.anthropic_index,
            delta: json!({"type": "text_delta", "text": delta}),
        }]
    }

    fn handle_function_args_delta(
        &mut self,
        output_index: u32,
        delta: &str,
    ) -> Vec<AnthropicEvent> {
        let Some(block) = self.blocks.get(&output_index) else {
            return Vec::new();
        };
        if !matches!(block.kind, BlockKind::ToolUse) {
            return Vec::new();
        }
        vec![AnthropicEvent::ContentBlockDelta {
            index: block.anthropic_index,
            delta: json!({"type": "input_json_delta", "partial_json": delta}),
        }]
    }

    fn handle_output_item_done(
        &mut self,
        output_index: u32,
        item: &Value,
    ) -> Vec<AnthropicEvent> {
        let Some(block) = self.blocks.get(&output_index) else {
            return Vec::new();
        };
        if matches!(block.kind, BlockKind::Skipped) {
            return Vec::new();
        }
        if let BlockKind::Reasoning { id: cached_id } = &block.kind {
            // reasoning done: signature 携带 encrypted_content (来自完整版的 item, 不是 added 时的部分版)
            let anthropic_index = block.anthropic_index;
            let reasoning_id = item
                .get("id")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| cached_id.clone());
            let encrypted_content = item
                .get("encrypted_content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let mut events = Vec::new();
            if !encrypted_content.is_empty() {
                let signature = encode_reasoning_signature(&reasoning_id, &encrypted_content);
                events.push(AnthropicEvent::ContentBlockDelta {
                    index: anthropic_index,
                    delta: json!({"type": "signature_delta", "signature": signature}),
                });
            }
            // 兜底: summary 在 done 时才出现的模型 (与提前 reasoning_summary_text.delta 互斥, 实测无重叠案例)
            if let Some(summary) = item.get("summary").and_then(|v| v.as_array()) {
                let text: String = summary
                    .iter()
                    .filter_map(|s| s.get("text").and_then(|t| t.as_str()).map(str::to_string))
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    events.insert(
                        0,
                        AnthropicEvent::ContentBlockDelta {
                            index: anthropic_index,
                            delta: json!({"type": "thinking_delta", "thinking": text}),
                        },
                    );
                }
            }
            events.push(AnthropicEvent::ContentBlockStop {
                index: anthropic_index,
            });
            return events;
        }
        vec![AnthropicEvent::ContentBlockStop {
            index: block.anthropic_index,
        }]
    }

    fn handle_completed(&mut self, resp: &Value) -> Vec<AnthropicEvent> {
        if self.stopped {
            return Vec::new();
        }
        self.stopped = true;

        let response = resp.get("response");
        let stop_reason = response
            .and_then(|r| r.get("status"))
            .and_then(|v| v.as_str())
            .map(map_status_to_anthropic_stop_reason)
            .unwrap_or("end_turn");

        let usage = response
            .and_then(|r| r.get("usage"))
            .cloned()
            .unwrap_or_else(|| json!({"input_tokens": 0, "output_tokens": 0}));
        self.final_usage = Some(usage.clone());

        vec![
            AnthropicEvent::MessageDelta {
                delta: json!({"stop_reason": stop_reason, "stop_sequence": null}),
                usage,
            },
            AnthropicEvent::MessageStop,
        ]
    }
}

pub fn map_status_to_anthropic_stop_reason(status: &str) -> &'static str {
    match status {
        "completed" => "end_turn",
        "incomplete" => "max_tokens",
        "cancelled" => "stop_sequence",
        _ => "end_turn",
    }
}

// ============================================================
// 非流式: 把所有 SSE 帧吃完, 还原成 Anthropic Messages 的最终 JSON
// ============================================================

/// 给客户端要非流式但上游强制 SSE 的场景 (codex chatgpt 反代) 用. openai 路径
/// 自身走真 JSON 路径 (responses_json_to_anthropic), 不会用到本结构体。
pub struct NonStreamingCollector {
    converter: ResponsesSseConverter,
    text_acc: HashMap<u32, String>,
    tool_args_acc: HashMap<u32, String>,
    thinking_acc: HashMap<u32, String>,
    signature_acc: HashMap<u32, String>,
    block_meta: HashMap<u32, Value>,
    order: Vec<u32>,
}

impl Default for NonStreamingCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl NonStreamingCollector {
    /// 默认配置: codex chatgpt 反代 + expose_reasoning=false (维持历史行为).
    /// 真实 dispatch 路径都走 [`Self::new_with_config`] 显式传配置。
    pub fn new() -> Self {
        Self::new_with_config(ResponsesTransformConfig::codex_chatgpt(false))
    }

    pub fn new_with_config(config: ResponsesTransformConfig) -> Self {
        Self {
            converter: ResponsesSseConverter::new_with_config(config),
            text_acc: HashMap::new(),
            tool_args_acc: HashMap::new(),
            thinking_acc: HashMap::new(),
            signature_acc: HashMap::new(),
            block_meta: HashMap::new(),
            order: Vec::new(),
        }
    }

    pub fn feed(&mut self, event_name: &str, data: &Value) {
        let events = self.converter.feed(event_name, data);
        for evt in events {
            self.absorb(&evt);
        }
    }

    pub fn finalize(mut self) -> Value {
        let _ = self.converter.finalize_if_needed();
        let content: Vec<Value> = self
            .order
            .iter()
            .filter_map(|idx| {
                let meta = self.block_meta.get(idx)?.clone();
                let mtype = meta.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match mtype {
                    "text" => {
                        let text = self.text_acc.get(idx).cloned().unwrap_or_default();
                        Some(json!({"type": "text", "text": text}))
                    }
                    "tool_use" => {
                        let args_raw = self.tool_args_acc.get(idx).cloned().unwrap_or_default();
                        let input: Value = if args_raw.is_empty() {
                            json!({})
                        } else {
                            serde_json::from_str(&args_raw).unwrap_or(json!({}))
                        };
                        Some(json!({
                            "type": "tool_use",
                            "id": meta.get("id").cloned().unwrap_or(Value::Null),
                            "name": meta.get("name").cloned().unwrap_or(Value::Null),
                            "input": input,
                        }))
                    }
                    "thinking" => {
                        let thinking = self.thinking_acc.get(idx).cloned().unwrap_or_default();
                        let signature = self.signature_acc.get(idx).cloned().unwrap_or_default();
                        Some(json!({
                            "type": "thinking",
                            "thinking": thinking,
                            "signature": signature,
                        }))
                    }
                    _ => None,
                }
            })
            .collect();

        let usage = self
            .converter
            .final_usage
            .clone()
            .unwrap_or_else(|| json!({"input_tokens": 0, "output_tokens": 0}));
        let stop_reason = "end_turn";

        json!({
            "id": if self.converter.message_id.is_empty() { Uuid::new_v4().to_string() } else { self.converter.message_id.clone() },
            "type": "message",
            "role": "assistant",
            "model": self.converter.response_model,
            "content": content,
            "stop_reason": stop_reason,
            "stop_sequence": null,
            "usage": usage,
        })
    }

    fn absorb(&mut self, evt: &AnthropicEvent) {
        match evt {
            AnthropicEvent::ContentBlockStart { index, content_block } => {
                if !self.order.contains(index) {
                    self.order.push(*index);
                }
                self.block_meta.insert(*index, content_block.clone());
            }
            AnthropicEvent::ContentBlockDelta { index, delta } => {
                let dtype = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match dtype {
                    "text_delta" => {
                        if let Some(t) = delta.get("text").and_then(|v| v.as_str()) {
                            self.text_acc.entry(*index).or_default().push_str(t);
                        }
                    }
                    "input_json_delta" => {
                        if let Some(t) = delta.get("partial_json").and_then(|v| v.as_str()) {
                            self.tool_args_acc.entry(*index).or_default().push_str(t);
                        }
                    }
                    "thinking_delta" => {
                        if let Some(t) = delta.get("thinking").and_then(|v| v.as_str()) {
                            self.thinking_acc.entry(*index).or_default().push_str(t);
                        }
                    }
                    "signature_delta" => {
                        if let Some(s) = delta.get("signature").and_then(|v| v.as_str()) {
                            // signature 是完整字符串, 不是增量, 覆盖式赋值
                            self.signature_acc.insert(*index, s.to_string());
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// 解 SSE 帧成 (event_name, data_json). 调用方反复调.
pub fn parse_sse_frame(raw: &str) -> Option<(String, Value)> {
    let mut event_name = String::new();
    let mut data = String::new();
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("event: ") {
            event_name = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("event:") {
            event_name = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("data: ") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest);
        } else if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest);
        }
    }
    if event_name.is_empty() {
        return None;
    }
    let parsed: Value = serde_json::from_str(&data).ok()?;
    Some((event_name, parsed))
}

// ============================================================
// 单测 (覆盖 helper 通用路径; codex / openai 入口的测试在各自模块)
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_sse_flow_gpt55_pattern() {
        // 复现 probe4 实测的 gpt-5.5 reasoning SSE 序列:
        // output_item.added(reasoning, encrypted_content=部分) → output_item.done(reasoning, encrypted_content=完整)
        // 不发 reasoning_summary_text.delta, summary 始终空数组
        let mut cfg = ResponsesTransformConfig::openai_official(false);
        cfg.emit_reasoning = true;
        let mut conv = ResponsesSseConverter::new_with_config(cfg);

        let started = conv.feed(
            "response.created",
            &json!({"response":{"id":"resp_1","model":"gpt-5.5"}}),
        );
        assert_eq!(started.len(), 1);
        assert_eq!(started[0].event_name(), "message_start");

        // reasoning added: emit content_block_start { thinking, "" }
        let r = conv.feed(
            "response.output_item.added",
            &json!({"output_index":0,"item":{
                "id":"rs_xxx","type":"reasoning",
                "encrypted_content":"ENC_PARTIAL","summary":[]
            }}),
        );
        assert_eq!(r.len(), 1, "reasoning added emit content_block_start");
        if let AnthropicEvent::ContentBlockStart { content_block, index } = &r[0] {
            assert_eq!(content_block["type"], "thinking");
            assert_eq!(content_block["thinking"], "");
            assert_eq!(*index, 0);
        } else {
            panic!("expected ContentBlockStart for thinking");
        }

        // reasoning done: emit signature_delta + content_block_stop (没 thinking_delta, 因为 summary 空)
        let r = conv.feed(
            "response.output_item.done",
            &json!({"output_index":0,"item":{
                "id":"rs_xxx","type":"reasoning",
                "encrypted_content":"ENC_FULL","summary":[]
            }}),
        );
        assert_eq!(r.len(), 2);
        if let AnthropicEvent::ContentBlockDelta { delta, .. } = &r[0] {
            assert_eq!(delta["type"], "signature_delta");
            let sig = delta["signature"].as_str().unwrap();
            // signature 解码后应能恢复 id 和 encrypted_content
            let (id, ec) = decode_reasoning_signature(sig).unwrap();
            assert_eq!(id, "rs_xxx");
            assert_eq!(ec, "ENC_FULL", "应该是 done 时的完整版, 不是 added 时的部分版");
        } else {
            panic!("expected ContentBlockDelta(signature_delta)");
        }
        assert_eq!(r[1].event_name(), "content_block_stop");

        // 后续 message item: 下一个 anthropic_index = 1
        let r = conv.feed(
            "response.output_item.added",
            &json!({"output_index":1,"item":{"type":"message"}}),
        );
        assert_eq!(r.len(), 1);
        if let AnthropicEvent::ContentBlockStart { index, .. } = &r[0] {
            assert_eq!(*index, 1, "reasoning 占了 index=0, message 用 index=1");
        }
    }

    #[test]
    fn reasoning_sse_flow_with_summary_delta() {
        // 模拟 o1/o3 模型可能发的 reasoning_summary_text.delta 事件
        let mut cfg = ResponsesTransformConfig::openai_official(false);
        cfg.emit_reasoning = true;
        let mut conv = ResponsesSseConverter::new_with_config(cfg);

        conv.feed("response.created", &json!({"response":{"id":"r","model":"o1"}}));
        let _ = conv.feed(
            "response.output_item.added",
            &json!({"output_index":0,"item":{"id":"rs_1","type":"reasoning","encrypted_content":"","summary":[]}}),
        );
        // reasoning_summary_text.delta — gpt-5.5 不发, 但 o1 可能发
        let r = conv.feed(
            "response.reasoning_summary_text.delta",
            &json!({"output_index":0,"delta":"thinking step 1"}),
        );
        assert_eq!(r.len(), 1);
        if let AnthropicEvent::ContentBlockDelta { delta, .. } = &r[0] {
            assert_eq!(delta["type"], "thinking_delta");
            assert_eq!(delta["thinking"], "thinking step 1");
        }
    }

    #[test]
    fn reasoning_skipped_when_emit_disabled_codex_default() {
        // codex 路径默认 emit_reasoning=false, reasoning item 整个被 skip
        let conv = ResponsesSseConverter::new(); // 默认 codex 配置
        let mut conv = conv;
        conv.feed("response.created", &json!({"response":{"id":"r","model":"gpt-5.5"}}));

        let r = conv.feed(
            "response.output_item.added",
            &json!({"output_index":0,"item":{"id":"rs_1","type":"reasoning","encrypted_content":"ENC"}}),
        );
        assert!(r.is_empty(), "reasoning skipped in codex default");

        let r = conv.feed(
            "response.output_item.done",
            &json!({"output_index":0,"item":{"id":"rs_1","type":"reasoning","encrypted_content":"ENC"}}),
        );
        assert!(r.is_empty());

        // 后续 message item 用 index=0 (reasoning 没占索引)
        let r = conv.feed(
            "response.output_item.added",
            &json!({"output_index":1,"item":{"type":"message"}}),
        );
        if let AnthropicEvent::ContentBlockStart { index, .. } = &r[0] {
            assert_eq!(*index, 0, "reasoning skipped 不占 anthropic_index");
        }
    }

    #[test]
    fn nonstreaming_collector_assembles_thinking_block() {
        let mut cfg = ResponsesTransformConfig::openai_official(false);
        cfg.emit_reasoning = true;
        let mut col = NonStreamingCollector::new_with_config(cfg);

        col.feed("response.created", &json!({"response":{"id":"r","model":"o1"}}));
        col.feed(
            "response.output_item.added",
            &json!({"output_index":0,"item":{"id":"rs_1","type":"reasoning","encrypted_content":""}}),
        );
        col.feed(
            "response.reasoning_summary_text.delta",
            &json!({"output_index":0,"delta":"step 1 "}),
        );
        col.feed(
            "response.reasoning_summary_text.delta",
            &json!({"output_index":0,"delta":"step 2"}),
        );
        col.feed(
            "response.output_item.done",
            &json!({"output_index":0,"item":{"id":"rs_1","type":"reasoning","encrypted_content":"FULL_ENC","summary":[]}}),
        );
        col.feed(
            "response.output_item.added",
            &json!({"output_index":1,"item":{"type":"message"}}),
        );
        col.feed(
            "response.output_text.delta",
            &json!({"output_index":1,"delta":"answer"}),
        );
        col.feed(
            "response.output_item.done",
            &json!({"output_index":1,"item":{"type":"message"}}),
        );
        col.feed(
            "response.completed",
            &json!({"response":{"status":"completed","usage":{"input_tokens":5,"output_tokens":3}}}),
        );

        let msg = col.finalize();
        let content = msg["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["thinking"], "step 1 step 2");
        let sig = content[0]["signature"].as_str().unwrap();
        let (id, ec) = decode_reasoning_signature(sig).unwrap();
        assert_eq!(id, "rs_1");
        assert_eq!(ec, "FULL_ENC");
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "answer");
    }

    #[test]
    fn reasoning_signature_round_trip() {
        let sig = encode_reasoning_signature("rs_abc", "ENCRYPTED_BYTES");
        assert!(!sig.is_empty());
        let (id, ec) = decode_reasoning_signature(&sig).unwrap();
        assert_eq!(id, "rs_abc");
        assert_eq!(ec, "ENCRYPTED_BYTES");
    }

    #[test]
    fn reasoning_signature_garbage_returns_none() {
        assert!(decode_reasoning_signature("not-base64!@#").is_none());
        assert!(decode_reasoning_signature("").is_none());
    }

    #[test]
    fn codex_config_matches_legacy_behavior() {
        let cfg = ResponsesTransformConfig::codex_chatgpt(false);
        assert!(cfg.force_upstream_streaming);
        assert!(cfg.inject_store_false);
        assert_eq!(cfg.inject_default_include, vec!["reasoning.encrypted_content"]);
        assert!(cfg.force_instructions_present);
        assert!(cfg.drop_max_tokens);
        assert!(!cfg.emit_reasoning);
        assert!(!cfg.roundtrip_reasoning);
    }

    #[test]
    fn openai_config_loose_defaults() {
        let cfg = ResponsesTransformConfig::openai_official(false);
        assert!(!cfg.force_upstream_streaming);
        assert!(cfg.inject_store_false);
        assert!(cfg.inject_default_include.is_empty());
        assert!(!cfg.force_instructions_present);
        assert!(!cfg.drop_max_tokens);
    }

    #[test]
    fn openai_path_preserves_client_stream_false() {
        let body = json!({
            "model": "gpt-5",
            "stream": false,
            "messages": [{"role": "user", "content": "hi"}],
        });
        let out = build_responses_body(&body, &ResponsesTransformConfig::openai_official(false)).unwrap();
        assert_eq!(out["stream"], json!(false));
    }

    #[test]
    fn openai_path_maps_max_tokens_to_max_output_tokens() {
        let body = json!({
            "model": "gpt-5",
            "max_tokens": 512,
            "messages": [{"role": "user", "content": "hi"}],
        });
        let out = build_responses_body(&body, &ResponsesTransformConfig::openai_official(false)).unwrap();
        assert_eq!(out["max_output_tokens"], json!(512));
        assert!(out.get("max_tokens").is_none());
    }

    #[test]
    fn openai_path_skips_instructions_when_no_system() {
        let body = json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "hi"}],
        });
        let out = build_responses_body(&body, &ResponsesTransformConfig::openai_official(false)).unwrap();
        assert!(out.get("instructions").is_none(), "openai 路径无 system 不该注入 instructions");
    }

    #[test]
    fn openai_path_keeps_instructions_when_system_present() {
        let body = json!({
            "model": "gpt-5",
            "system": "你是助手",
            "messages": [{"role": "user", "content": "hi"}],
        });
        let out = build_responses_body(&body, &ResponsesTransformConfig::openai_official(false)).unwrap();
        assert_eq!(out["instructions"], json!("你是助手"));
    }

    #[test]
    fn openai_path_no_include_field() {
        let body = json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "hi"}],
        });
        let out = build_responses_body(&body, &ResponsesTransformConfig::openai_official(false)).unwrap();
        assert!(out.get("include").is_none(), "openai 路径默认不注入 include");
    }

    // ============================================================
    // signature 编码/解码 兼容性测试 (修跨 provider thinking 错喂)
    // ============================================================

    #[test]
    fn encode_decode_signature_v2_roundtrip() {
        let sig = encode_reasoning_signature("rs_xyz", "encrypted_blob");
        let decoded = decode_reasoning_signature(&sig).unwrap();
        assert_eq!(decoded.0, "rs_xyz");
        assert_eq!(decoded.1, "encrypted_blob");
    }

    #[test]
    fn decode_v1_signature_backwards_compatible() {
        // 老 cc-router (v=1, 无 p 字段) 包装的 signature, 新 cc-router 必须仍能解
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let legacy_payload = json!({ "v": 1, "id": "rs_legacy", "ec": "old_ec" });
        let legacy_sig = URL_SAFE_NO_PAD.encode(legacy_payload.to_string().as_bytes());
        let decoded = decode_reasoning_signature(&legacy_sig).unwrap();
        assert_eq!(decoded.0, "rs_legacy");
        assert_eq!(decoded.1, "old_ec");
    }

    #[test]
    fn decode_v2_with_wrong_provider_tag_fails() {
        // 假装 gemini 的 signature 喂到 openai_responses decoder → 必须拒绝
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let foreign = json!({ "v": 2, "p": "gemini", "id": "rs_x", "ec": "y" });
        let sig = URL_SAFE_NO_PAD.encode(foreign.to_string().as_bytes());
        assert!(decode_reasoning_signature(&sig).is_none());
    }

    #[test]
    fn decode_v3_or_unknown_version_fails() {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let future = json!({ "v": 99, "p": "openai_responses", "id": "x", "ec": "y" });
        let sig = URL_SAFE_NO_PAD.encode(future.to_string().as_bytes());
        assert!(decode_reasoning_signature(&sig).is_none());
    }

    #[test]
    fn looks_like_cc_router_signature_dispatch() {
        // v=2 openai 包装
        let openai_sig = encode_reasoning_signature("rs_a", "ec_a");
        assert_eq!(
            looks_like_cc_router_signature(&openai_sig),
            Some("openai_responses")
        );

        // gemini 包装
        let gemini_sig =
            crate::proxy::transform::gemini::encode_gemini_thought_signature("ts_blob");
        assert_eq!(
            looks_like_cc_router_signature(&gemini_sig),
            Some("gemini")
        );

        // 空字符串
        assert_eq!(looks_like_cc_router_signature(""), None);

        // Anthropic 原生 UUID 不是 base64url 合法 JSON, 应该 None
        assert_eq!(
            looks_like_cc_router_signature("03ea0953-5ece-4386-afea-31404f331c5f"),
            None
        );
    }
}
