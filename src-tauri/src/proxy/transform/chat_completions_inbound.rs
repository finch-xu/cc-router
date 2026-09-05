//! OpenAI Chat Completions → Anthropic Messages 反向翻译层 (入站方向).
//!
//! 用途: cc-router 对外的 `POST /v1/chat/completions` 兼容入口 ([`handler::chat_completions`]) 用本模块
//! 把 Chat Completions 请求翻译成 Anthropic Messages 走现有 pipeline, 再把 Anthropic 响应 / SSE
//! 翻译回 Chat Completions 给客户端。
//!
//! 配对方 (方向相反, 代码不共享): [`super::openai_chat_completions`] 是出站方向
//! (Anthropic → 上游 Chat Completions, 用于 `auth_type=openai_chat_completions_api_key`)。
//!
//! ## 边界
//!
//! 本模块的函数都是纯函数 + 同步状态机, 不发起任何网络请求, 不读 DB / settings。
//! pipeline 不变, 上游全部 provider 与翻译 dispatch 零改动。
//!
//! ## 刻意不支持 (spec §1)
//!
//! - 旧版 `functions` / `function_call` → 400
//! - `n>1` / `logprobs` / `logit_bias` / `seed` / `presence_penalty` / `frequency_penalty` /
//!   `response_format` / `stream_options` → 忽略
//! - 历史消息里的 `reasoning_content` → 丢弃 (客户端拿不到合法 signature, Anthropic 透传会拒)
//! - 音频 / 文件类 content part → 400

use std::collections::HashMap;

use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

use super::responses_common::effort_to_budget_tokens;

/// Anthropic 要求 max_tokens 必填; 客户端没给时的缺省, 与 responses_inbound 一致.
const DEFAULT_MAX_TOKENS: i64 = 4096;

// ============================================================
// 请求侧: Chat Completions request → Anthropic Messages request
// ============================================================

/// OpenAI Chat Completions 请求体 → Anthropic Messages 请求体 (spec §3).
pub fn request_to_anthropic(body: &Value) -> AppResult<Value> {
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::BadRequest("请求 body 缺少 model".into()))?;
    if body.get("functions").is_some() || body.get("function_call").is_some() {
        return Err(AppError::BadRequest(
            "不支持旧版 functions / function_call 字段, 请使用 tools / tool_choice".into(),
        ));
    }
    let messages = body
        .get("messages")
        .and_then(|v| v.as_array())
        .filter(|a| !a.is_empty())
        .ok_or_else(|| AppError::BadRequest("messages 缺失或为空".into()))?;

    let mut out = json!({ "model": model });
    if let Some(s) = body.get("stream") {
        out["stream"] = s.clone();
    }

    let (anthropic_messages, system_text) = convert_messages(messages)?;
    if anthropic_messages.is_empty() {
        return Err(AppError::BadRequest("messages 中没有 user / assistant / tool 消息".into()));
    }
    out["messages"] = Value::Array(anthropic_messages);
    if !system_text.is_empty() {
        out["system"] = json!([{"type": "text", "text": system_text}]);
    }

    // max_completion_tokens 优先 (OpenAI 新字段), 其次 max_tokens; 都缺省 → 4096
    let max_tokens = body
        .get("max_completion_tokens")
        .and_then(|v| v.as_i64())
        .or_else(|| body.get("max_tokens").and_then(|v| v.as_i64()))
        .unwrap_or(DEFAULT_MAX_TOKENS);
    out["max_tokens"] = json!(max_tokens);

    for key in ["temperature", "top_p"] {
        if let Some(v) = body.get(key) {
            out[key] = v.clone();
        }
    }

    match body.get("stop") {
        Some(Value::String(s)) => out["stop_sequences"] = json!([s]),
        Some(Value::Array(arr)) => out["stop_sequences"] = Value::Array(arr.clone()),
        _ => {}
    }

    if let Some(user) = body.get("user").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        out["metadata"] = json!({ "user_id": user });
    }

    if let Some(effort) = body.get("reasoning_effort").and_then(|v| v.as_str()) {
        if effort != "none" {
            out["thinking"] = json!({
                "type": "enabled",
                "budget_tokens": effort_to_budget_tokens(effort),
            });
        }
    }

    let mut has_tools = false;
    if let Some(tools) = body.get("tools").and_then(|v| v.as_array()) {
        let mut converted = Vec::with_capacity(tools.len());
        for t in tools {
            converted.push(convert_tool(t)?);
        }
        if !converted.is_empty() {
            out["tools"] = Value::Array(converted);
            has_tools = true;
        }
    }

    // parallel_tool_calls:false → Anthropic 的 disable_parallel_tool_use 挂在 tool_choice 上,
    // 客户端没给 tool_choice 但有 tools 时以 auto 承载.
    let disable_parallel = body.get("parallel_tool_calls").and_then(|v| v.as_bool()) == Some(false);
    let mut tool_choice = match body.get("tool_choice") {
        Some(tc) => Some(convert_tool_choice(tc)?),
        None if has_tools && disable_parallel => Some(json!({"type": "auto"})),
        None => None,
    };
    if let Some(tc) = tool_choice.as_mut() {
        if disable_parallel && tc.get("type").and_then(|v| v.as_str()) != Some("none") {
            tc["disable_parallel_tool_use"] = json!(true);
        }
    }
    if let Some(tc) = tool_choice {
        out["tool_choice"] = tc;
    }

    Ok(out)
}

/// Chat `messages[]` → (Anthropic messages, 合并后的 system 文本).
///
/// 规则 (spec §3.2): system/developer 抽到顶层; assistant 的 tool_calls → tool_use 块,
/// reasoning_content 丢弃; **连续多条 tool 消息合并进同一条 user 消息** (Anthropic 要求
/// 同一轮的 tool_result 全在一条 user 消息里)。**转换完成后再做一趟相邻同 role 合并**
/// (`merge_adjacent_same_role`): tool_result 消息与紧随其后的普通 user 文本消息都是
/// `role:"user"`, 不合并会产生两条相邻 user 消息——Anthropic 官方能容忍, 但 cc-router
/// 还会分发给严格程度不一的第三方 Anthropic 兼容端点/翻译层, 合并后的形状在所有下游都合法。
fn convert_messages(messages: &[Value]) -> AppResult<(Vec<Value>, String)> {
    let mut out: Vec<Value> = Vec::new();
    let mut system = String::new();
    let mut pending_tool_results: Vec<Value> = Vec::new();

    fn flush_tool_results(out: &mut Vec<Value>, pending: &mut Vec<Value>) {
        if !pending.is_empty() {
            out.push(json!({ "role": "user", "content": std::mem::take(pending) }));
        }
    }

    for (i, msg) in messages.iter().enumerate() {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role != "tool" {
            flush_tool_results(&mut out, &mut pending_tool_results);
        }
        match role {
            "system" | "developer" => {
                let text = content_text(msg.get("content"));
                if !text.is_empty() {
                    if !system.is_empty() {
                        system.push_str("\n\n");
                    }
                    system.push_str(&text);
                }
            }
            "user" => {
                let blocks = user_content_blocks(msg.get("content"), i)?;
                out.push(json!({ "role": "user", "content": blocks }));
            }
            "assistant" => {
                let mut blocks: Vec<Value> = Vec::new();
                let text = content_text(msg.get("content"));
                if !text.is_empty() {
                    blocks.push(json!({ "type": "text", "text": text }));
                }
                if let Some(calls) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                    for call in calls {
                        blocks.push(convert_tool_call(call)?);
                    }
                }
                // reasoning_content 刻意丢弃 (spec §3.4)
                if !blocks.is_empty() {
                    out.push(json!({ "role": "assistant", "content": blocks }));
                }
            }
            "tool" => {
                let id = msg
                    .get("tool_call_id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| AppError::BadRequest(format!("messages[{i}] (tool) 缺少 tool_call_id")))?;
                pending_tool_results.push(json!({
                    "type": "tool_result",
                    "tool_use_id": id,
                    "content": content_text(msg.get("content")),
                }));
            }
            other => {
                return Err(AppError::BadRequest(format!("messages[{i}] 不支持的 role: {other:?}")));
            }
        }
    }
    flush_tool_results(&mut out, &mut pending_tool_results);
    Ok((merge_adjacent_same_role(out), system))
}

/// 合并相邻的同 role 消息, 把后一条的 content 数组接到前一条后面.
///
/// 覆盖 user+user (含 "tool_result 块后接一条 user 文本" 这种常见 agentic 形状) 与
/// assistant+assistant。两条消息的 `content` 都保证是数组 (本模块构造的消息全部如此)。
fn merge_adjacent_same_role(messages: Vec<Value>) -> Vec<Value> {
    let mut merged: Vec<Value> = Vec::with_capacity(messages.len());
    for mut msg in messages {
        if let Some(last) = merged.last_mut() {
            if last["role"] == msg["role"] {
                if let (Some(last_content), Some(cur_content)) =
                    (last["content"].as_array_mut(), msg["content"].as_array_mut())
                {
                    last_content.append(cur_content);
                    continue;
                }
            }
        }
        merged.push(msg);
    }
    merged
}

/// content 为 string 直接返回; 为数组时拼接其中 `type:text` 的 text (以 "\n" 连接); 其他 → 空串.
fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// user 消息 content → Anthropic content blocks. 空文本 → 400 (Anthropic 拒绝空 text 块).
fn user_content_blocks(content: Option<&Value>, idx: usize) -> AppResult<Vec<Value>> {
    let mut blocks: Vec<Value> = Vec::new();
    match content {
        Some(Value::String(s)) => {
            if !s.is_empty() {
                blocks.push(json!({ "type": "text", "text": s }));
            }
        }
        Some(Value::Array(parts)) => {
            for part in parts {
                match part.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                    "text" => {
                        if let Some(t) = part.get("text").and_then(|t| t.as_str()).filter(|t| !t.is_empty()) {
                            blocks.push(json!({ "type": "text", "text": t }));
                        }
                    }
                    "image_url" => blocks.push(convert_image_url(part.get("image_url"), idx)?),
                    other => {
                        return Err(AppError::BadRequest(format!(
                            "messages[{idx}] 不支持的 content part 类型: {other:?}"
                        )));
                    }
                }
            }
        }
        _ => {}
    }
    if blocks.is_empty() {
        return Err(AppError::BadRequest(format!("messages[{idx}] (user) 内容为空")));
    }
    Ok(blocks)
}

/// `image_url` part → Anthropic image 块. data URI → base64 source; http(s) → url source; 其他 → 400.
fn convert_image_url(image_url: Option<&Value>, idx: usize) -> AppResult<Value> {
    let url = match image_url {
        Some(Value::String(s)) => s.as_str(),
        Some(obj) => obj.get("url").and_then(|u| u.as_str()).unwrap_or(""),
        None => "",
    };
    if let Some(rest) = url.strip_prefix("data:") {
        // data:<media_type>;base64,<payload>
        let (meta, data) = rest
            .split_once(',')
            .ok_or_else(|| AppError::BadRequest(format!("messages[{idx}] image data URI 缺少逗号分隔")))?;
        let media_type = meta.strip_suffix(";base64").ok_or_else(|| {
            AppError::BadRequest(format!("messages[{idx}] image data URI 必须是 base64 编码"))
        })?;
        return Ok(json!({
            "type": "image",
            "source": { "type": "base64", "media_type": media_type, "data": data },
        }));
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        return Ok(json!({ "type": "image", "source": { "type": "url", "url": url } }));
    }
    Err(AppError::BadRequest(format!(
        "messages[{idx}] image_url 只支持 data: 与 http(s): 两种形式"
    )))
}

/// `{type:"function", function:{name, description?, parameters?}}` → Anthropic tool spec.
fn convert_tool(t: &Value) -> AppResult<Value> {
    if t.get("type").and_then(|v| v.as_str()) != Some("function") {
        return Err(AppError::BadRequest(format!(
            "tools 只支持 type=function, 收到: {}",
            t.get("type").cloned().unwrap_or(Value::Null)
        )));
    }
    let f = t
        .get("function")
        .ok_or_else(|| AppError::BadRequest("tools[] 缺少 function 字段".into()))?;
    let name = f
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("tools[].function 缺少 name".into()))?;
    let mut out = json!({
        "name": name,
        "input_schema": f.get("parameters").cloned().unwrap_or_else(|| json!({"type": "object", "properties": {}})),
    });
    if let Some(d) = f.get("description") {
        out["description"] = d.clone();
    }
    Ok(out)
}

/// assistant.tool_calls[i] → Anthropic tool_use 块. arguments 是 JSON 字符串 (空串视为 {}),
/// 部分客户端会直接给对象, 也接受; 非法 JSON → 400 并带 tool_call id.
fn convert_tool_call(call: &Value) -> AppResult<Value> {
    let id = call.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let f = call
        .get("function")
        .ok_or_else(|| AppError::BadRequest(format!("tool_calls[{id}] 缺少 function 字段")))?;
    let name = f
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::BadRequest(format!("tool_calls[{id}] 缺少 function.name")))?;
    let input = match f.get("arguments") {
        None => json!({}),
        Some(Value::String(s)) if s.trim().is_empty() => json!({}),
        Some(Value::String(s)) => serde_json::from_str::<Value>(s).map_err(|e| {
            AppError::BadRequest(format!("tool_calls[{id}] 的 arguments 不是合法 JSON: {e}"))
        })?,
        Some(other) => other.clone(),
    };
    Ok(json!({ "type": "tool_use", "id": id, "name": name, "input": input }))
}

/// Chat tool_choice → Anthropic tool_choice (spec §3.3).
fn convert_tool_choice(tc: &Value) -> AppResult<Value> {
    if let Some(s) = tc.as_str() {
        return match s {
            "auto" => Ok(json!({ "type": "auto" })),
            "none" => Ok(json!({ "type": "none" })),
            "required" => Ok(json!({ "type": "any" })),
            other => Err(AppError::BadRequest(format!("不支持的 tool_choice: {other:?}"))),
        };
    }
    if tc.get("type").and_then(|v| v.as_str()) == Some("function") {
        if let Some(name) = tc.get("function").and_then(|f| f.get("name")).and_then(|n| n.as_str()) {
            return Ok(json!({ "type": "tool", "name": name }));
        }
    }
    Err(AppError::BadRequest("tool_choice 形状不合法".into()))
}

// ============================================================
// 响应侧 JSON: Anthropic Message → Chat Completion
// ============================================================

/// Anthropic Message JSON → `chat.completion` (spec §4.1). `requested_model` 回显客户端请求名.
pub fn response_to_chat_json(anthropic_msg: &Value, requested_model: &str) -> Value {
    let id = chat_id_from(anthropic_msg.get("id").and_then(|v| v.as_str()));
    let parts = collect_content(anthropic_msg.get("content"));
    let mut message = json!({
        "role": "assistant",
        "content": parts.text.map(Value::String).unwrap_or(Value::Null),
    });
    if let Some(r) = parts.reasoning {
        message["reasoning_content"] = Value::String(r);
    }
    let has_tool_calls = !parts.tool_calls.is_empty();
    if has_tool_calls {
        message["tool_calls"] = Value::Array(parts.tool_calls);
    }
    let stop_reason = anthropic_msg.get("stop_reason").and_then(|v| v.as_str());
    json!({
        "id": id,
        "object": "chat.completion",
        "created": now_unix(),
        "model": requested_model,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": finish_reason_for(stop_reason, has_tool_calls),
        }],
        "usage": usage_to_chat(anthropic_msg.get("usage")),
    })
}

struct CollectedContent {
    text: Option<String>,
    reasoning: Option<String>,
    tool_calls: Vec<Value>,
}

/// 遍历 Anthropic content blocks: text 拼接 / thinking 拼接 / tool_use → OpenAI tool_calls.
fn collect_content(content: Option<&Value>) -> CollectedContent {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut tool_calls = Vec::new();
    if let Some(blocks) = content.and_then(|c| c.as_array()) {
        for b in blocks {
            match b.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                "text" => text.push_str(b.get("text").and_then(|t| t.as_str()).unwrap_or("")),
                "thinking" => reasoning.push_str(b.get("thinking").and_then(|t| t.as_str()).unwrap_or("")),
                "tool_use" => tool_calls.push(json!({
                    "id": b.get("id").cloned().unwrap_or(Value::Null),
                    "type": "function",
                    "function": {
                        "name": b.get("name").cloned().unwrap_or(Value::Null),
                        "arguments": serde_json::to_string(b.get("input").unwrap_or(&json!({})))
                            .unwrap_or_else(|_| "{}".to_string()),
                    },
                })),
                _ => {}
            }
        }
    }
    CollectedContent {
        text: (!text.is_empty()).then_some(text),
        reasoning: (!reasoning.is_empty()).then_some(reasoning),
        tool_calls,
    }
}

/// `msg_abc` → `chatcmpl-abc`; 无 id → `chatcmpl-<uuid simple>`.
pub fn chat_id_from(anthropic_id: Option<&str>) -> String {
    match anthropic_id.filter(|s| !s.is_empty()) {
        Some(id) => format!("chatcmpl-{}", id.strip_prefix("msg_").unwrap_or(id)),
        None => format!("chatcmpl-{}", Uuid::new_v4().simple()),
    }
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Anthropic stop_reason → OpenAI finish_reason (spec §4.3). 纯查表, 不看内容里是否有 tool_use.
pub fn map_finish_reason(stop_reason: Option<&str>) -> &'static str {
    match stop_reason {
        Some("max_tokens") => "length",
        Some("tool_use") => "tool_calls",
        Some("refusal") => "content_filter",
        _ => "stop",
    }
}

/// finish_reason 判定入口 (spec §4.3 补充): 响应含至少一个 tool_use 块, 且 stop_reason 不是
/// `max_tokens` / `refusal` 时一律报 `tool_calls` —— 兼容经翻译层/中转站回传 `end_turn` 等
/// "错误" stop_reason 但实际带 tool_use 内容的上游 (OpenAI 客户端常按 finish_reason=="tool_calls"
/// 门控工具执行, 报成 "stop" 会导致客户端不执行工具调用)。`max_tokens` / `refusal` 优先级更高,
/// 仍走 [`map_finish_reason`] 的对应结果。
pub fn finish_reason_for(stop_reason: Option<&str>, has_tool_calls: bool) -> &'static str {
    if has_tool_calls {
        match stop_reason {
            Some("max_tokens") | Some("refusal") => map_finish_reason(stop_reason),
            _ => "tool_calls",
        }
    } else {
        map_finish_reason(stop_reason)
    }
}

/// Anthropic usage → OpenAI usage. prompt = input + cache_creation + cache_read;
/// cache_read 另落 `prompt_tokens_details.cached_tokens`.
pub fn usage_to_chat(usage: Option<&Value>) -> Value {
    let get = |k: &str| usage.and_then(|u| u.get(k)).and_then(|v| v.as_i64()).unwrap_or(0);
    let cache_read = get("cache_read_input_tokens");
    let prompt = get("input_tokens") + get("cache_creation_input_tokens") + cache_read;
    let completion = get("output_tokens");
    json!({
        "prompt_tokens": prompt,
        "completion_tokens": completion,
        "total_tokens": prompt + completion,
        "prompt_tokens_details": { "cached_tokens": cache_read },
    })
}

/// OpenAI 风格错误体 (spec §4.4). `type` 按表映射, `code` 保留 Anthropic 原始 error type 供排查.
pub fn chat_error_body(anthropic_type: &str, message: &str) -> Value {
    let openai_type = match anthropic_type {
        "invalid_request_error" | "authentication_error" | "permission_error" => "invalid_request_error",
        "rate_limit_error" | "overloaded_error" => "rate_limit_error",
        _ => "server_error",
    };
    json!({
        "error": {
            "message": message,
            "type": openai_type,
            "code": anthropic_type,
            "param": Value::Null,
        }
    })
}

// ============================================================
// SSE: Anthropic 事件流 → chat.completion.chunk 帧
// ============================================================

/// Anthropic SSE → OpenAI Chat Completions SSE 转换器 (spec §4.2).
///
/// 输入: Anthropic 事件名 + data JSON (由 handler 拆帧后喂入).
/// 输出: 已序列化的 `data: {...}\n\n` 帧; 末尾 `data: [DONE]\n\n`.
///
/// 生命周期: `message_start` 置 started → 各 delta → `message_delta` 发 finish_reason 帧
/// (finished) → `message_stop` 发 usage 帧 + [DONE] (done). `error` 事件直接发错误帧 + [DONE].
/// done 之后任何输入都不再产生输出. 上游断流时 handler 调 `finalize_if_needed` 兜底.
pub struct AnthropicToChatSseConverter {
    id: String,
    model: String,
    created: i64,
    started: bool,
    finished: bool,
    done: bool,
    /// Anthropic content_block index → 块类型 (tool_use 记 OpenAI tool_calls 序号).
    blocks: HashMap<u32, ChatBlock>,
    next_tool_index: u32,
    input_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
    output_tokens: i64,
    stop_reason: Option<String>,
    /// 是否见过至少一个 tool_use 块 (finish_reason 的 tool_calls 判定, spec §4.3 补充).
    saw_tool_use: bool,
}

#[derive(Debug, Clone, Copy)]
enum ChatBlock {
    Text,
    Thinking,
    ToolUse { tool_index: u32 },
    Other,
}

const DONE_FRAME: &str = "data: [DONE]\n\n";

impl AnthropicToChatSseConverter {
    pub fn new(requested_model: String) -> Self {
        Self {
            id: chat_id_from(None),
            model: requested_model,
            created: now_unix(),
            started: false,
            finished: false,
            done: false,
            blocks: HashMap::new(),
            next_tool_index: 0,
            input_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            output_tokens: 0,
            stop_reason: None,
            saw_tool_use: false,
        }
    }

    /// 喂入一个 Anthropic SSE 事件, 返回若干 chunk 帧.
    pub fn feed(&mut self, event_name: &str, data: &Value) -> Vec<String> {
        if self.done {
            return Vec::new();
        }
        match event_name {
            "message_start" => self.on_message_start(data),
            "content_block_start" if self.started => self.on_block_start(data),
            "content_block_delta" if self.started => self.on_block_delta(data),
            "message_delta" if self.started => self.on_message_delta(data),
            "message_stop" if self.started => self.terminate(),
            // 刻意不加 `if self.started` 守卫: 上游可能在 message_start 之前就报错
            // (鉴权失败 / 限流等), 必须把错误帧 + 终结 [DONE] 送达客户端, 否则客户端会一直挂起等待.
            "error" => self.on_error(data),
            // ping / content_block_stop / 未 started 的增量 / 未知事件
            _ => Vec::new(),
        }
    }

    /// 上游断流兜底: started 且未 done 时补 finish + usage + [DONE].
    pub fn finalize_if_needed(&mut self) -> Vec<String> {
        if self.started && !self.done {
            self.terminate()
        } else {
            Vec::new()
        }
    }

    // ---------- handlers ----------

    fn on_message_start(&mut self, data: &Value) -> Vec<String> {
        if self.started {
            return Vec::new();
        }
        self.started = true;
        let msg = data.get("message");
        self.id = chat_id_from(msg.and_then(|m| m.get("id")).and_then(|v| v.as_str()));
        self.absorb_usage(msg.and_then(|m| m.get("usage")));
        vec![self.chunk(json!({"role": "assistant", "content": ""}), None)]
    }

    fn on_block_start(&mut self, data: &Value) -> Vec<String> {
        let Some(index) = data.get("index").and_then(|v| v.as_u64()).map(|v| v as u32) else {
            return Vec::new();
        };
        let block = data.get("content_block");
        match block.and_then(|b| b.get("type")).and_then(|t| t.as_str()).unwrap_or("") {
            "text" => {
                self.blocks.insert(index, ChatBlock::Text);
                Vec::new()
            }
            "thinking" => {
                self.blocks.insert(index, ChatBlock::Thinking);
                Vec::new()
            }
            "tool_use" => {
                self.saw_tool_use = true;
                let tool_index = self.next_tool_index;
                self.next_tool_index += 1;
                self.blocks.insert(index, ChatBlock::ToolUse { tool_index });
                let id = block.and_then(|b| b.get("id")).cloned().unwrap_or(Value::Null);
                let name = block.and_then(|b| b.get("name")).cloned().unwrap_or(Value::Null);
                vec![self.chunk(
                    json!({"tool_calls": [{
                        "index": tool_index,
                        "id": id,
                        "type": "function",
                        "function": {"name": name, "arguments": ""},
                    }]}),
                    None,
                )]
            }
            _ => {
                self.blocks.insert(index, ChatBlock::Other);
                Vec::new()
            }
        }
    }

    fn on_block_delta(&mut self, data: &Value) -> Vec<String> {
        let Some(index) = data.get("index").and_then(|v| v.as_u64()).map(|v| v as u32) else {
            return Vec::new();
        };
        let Some(delta) = data.get("delta") else {
            return Vec::new();
        };
        let block = self.blocks.get(&index).copied().unwrap_or(ChatBlock::Other);
        match (delta.get("type").and_then(|t| t.as_str()).unwrap_or(""), block) {
            ("text_delta", _) => {
                let text = delta.get("text").and_then(|t| t.as_str()).unwrap_or("");
                vec![self.chunk(json!({"content": text}), None)]
            }
            ("thinking_delta", _) => {
                let text = delta.get("thinking").and_then(|t| t.as_str()).unwrap_or("");
                vec![self.chunk(json!({"reasoning_content": text}), None)]
            }
            ("input_json_delta", ChatBlock::ToolUse { tool_index }) => {
                let partial = delta.get("partial_json").and_then(|t| t.as_str()).unwrap_or("");
                vec![self.chunk(
                    json!({"tool_calls": [{"index": tool_index, "function": {"arguments": partial}}]}),
                    None,
                )]
            }
            // signature_delta / 不认识的 delta / 没登记的块
            _ => Vec::new(),
        }
    }

    fn on_message_delta(&mut self, data: &Value) -> Vec<String> {
        if let Some(sr) = data.get("delta").and_then(|d| d.get("stop_reason")).and_then(|v| v.as_str()) {
            self.stop_reason = Some(sr.to_string());
        }
        self.absorb_usage(data.get("usage"));
        self.finish_frame()
    }

    fn on_error(&mut self, data: &Value) -> Vec<String> {
        let err = data.get("error");
        let etype = err.and_then(|e| e.get("type")).and_then(|v| v.as_str()).unwrap_or("api_error");
        let msg = err.and_then(|e| e.get("message")).and_then(|v| v.as_str()).unwrap_or("upstream error");
        self.done = true;
        vec![
            format!("data: {}\n\n", chat_error_body(etype, msg)),
            DONE_FRAME.to_string(),
        ]
    }

    /// message_stop / 兜底共用: (未发过 finish 则补) + usage 帧 + [DONE].
    fn terminate(&mut self) -> Vec<String> {
        let mut out = self.finish_frame();
        out.push(self.usage_frame());
        out.push(DONE_FRAME.to_string());
        self.done = true;
        out
    }

    fn finish_frame(&mut self) -> Vec<String> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        let reason = finish_reason_for(self.stop_reason.as_deref(), self.saw_tool_use);
        vec![self.chunk(json!({}), Some(reason))]
    }

    fn usage_frame(&self) -> String {
        let usage = usage_to_chat(Some(&json!({
            "input_tokens": self.input_tokens,
            "cache_creation_input_tokens": self.cache_creation_tokens,
            "cache_read_input_tokens": self.cache_read_tokens,
            "output_tokens": self.output_tokens,
        })));
        let body = json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [],
            "usage": usage,
        });
        format!("data: {body}\n\n")
    }

    /// message_start 与 message_delta 都可能带 usage; 有值就覆盖 (Anthropic 语义是累计值).
    fn absorb_usage(&mut self, usage: Option<&Value>) {
        let Some(u) = usage else { return };
        let read = |k: &str| u.get(k).and_then(|v| v.as_i64());
        if let Some(v) = read("input_tokens") { self.input_tokens = v; }
        if let Some(v) = read("cache_creation_input_tokens") { self.cache_creation_tokens = v; }
        if let Some(v) = read("cache_read_input_tokens") { self.cache_read_tokens = v; }
        if let Some(v) = read("output_tokens") { self.output_tokens = v; }
    }

    fn chunk(&self, delta: Value, finish_reason: Option<&str>) -> String {
        let body = json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [{
                "index": 0,
                "delta": delta,
                "finish_reason": finish_reason.map(Value::from).unwrap_or(Value::Null),
            }],
        });
        format!("data: {body}\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(messages: Value) -> Value {
        json!({"model": "gpt-5.5", "messages": messages})
    }

    // ---- 请求侧 ----

    #[test]
    fn request_text_only_defaults() {
        let out = request_to_anthropic(&base(json!([{"role":"user","content":"Hello"}]))).unwrap();
        assert_eq!(out["model"], "gpt-5.5");
        assert_eq!(out["max_tokens"], 4096);
        assert_eq!(out["messages"][0]["role"], "user");
        assert_eq!(out["messages"][0]["content"][0], json!({"type":"text","text":"Hello"}));
        assert!(out.get("system").is_none());
        assert!(out.get("stream").is_none());
        assert!(out.get("thinking").is_none());
    }

    #[test]
    fn request_system_and_developer_merge_into_system_in_order() {
        let out = request_to_anthropic(&base(json!([
            {"role":"system","content":"A"},
            {"role":"user","content":"hi"},
            {"role":"developer","content":[{"type":"text","text":"B"}]},
        ])))
        .unwrap();
        assert_eq!(out["system"], json!([{"type":"text","text":"A\n\nB"}]));
        assert_eq!(out["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn request_max_completion_tokens_wins_over_max_tokens() {
        let mut b = base(json!([{"role":"user","content":"hi"}]));
        b["max_tokens"] = json!(100);
        b["max_completion_tokens"] = json!(200);
        assert_eq!(request_to_anthropic(&b).unwrap()["max_tokens"], 200);
        let mut b2 = base(json!([{"role":"user","content":"hi"}]));
        b2["max_tokens"] = json!(100);
        assert_eq!(request_to_anthropic(&b2).unwrap()["max_tokens"], 100);
    }

    #[test]
    fn request_stream_temperature_top_p_stop_user_passthrough() {
        let mut b = base(json!([{"role":"user","content":"hi"}]));
        b["stream"] = json!(true);
        b["temperature"] = json!(0.3);
        b["top_p"] = json!(0.9);
        b["stop"] = json!("END");
        b["user"] = json!("u-1");
        let out = request_to_anthropic(&b).unwrap();
        assert_eq!(out["stream"], true);
        assert_eq!(out["temperature"], 0.3);
        assert_eq!(out["top_p"], 0.9);
        assert_eq!(out["stop_sequences"], json!(["END"]));
        assert_eq!(out["metadata"]["user_id"], "u-1");
        b["stop"] = json!(["a", "b"]);
        assert_eq!(request_to_anthropic(&b).unwrap()["stop_sequences"], json!(["a", "b"]));
    }

    #[test]
    fn request_reasoning_effort_to_thinking_and_none_disables() {
        let mut b = base(json!([{"role":"user","content":"hi"}]));
        b["reasoning_effort"] = json!("low");
        let out = request_to_anthropic(&b).unwrap();
        assert_eq!(out["thinking"], json!({"type":"enabled","budget_tokens":2048}));
        b["reasoning_effort"] = json!("none");
        assert!(request_to_anthropic(&b).unwrap().get("thinking").is_none());
    }

    #[test]
    fn request_tools_and_tool_choice_variants() {
        let mut b = base(json!([{"role":"user","content":"hi"}]));
        b["tools"] = json!([
            {"type":"function","function":{"name":"get_weather","description":"d","parameters":{"type":"object","properties":{"city":{"type":"string"}}}}},
            {"type":"function","function":{"name":"noop"}},
        ]);
        let out = request_to_anthropic(&b).unwrap();
        assert_eq!(out["tools"][0]["name"], "get_weather");
        assert_eq!(out["tools"][0]["description"], "d");
        assert_eq!(out["tools"][0]["input_schema"]["properties"]["city"]["type"], "string");
        assert_eq!(out["tools"][1]["input_schema"], json!({"type":"object","properties":{}}));
        assert!(out.get("tool_choice").is_none());

        for (input, expected) in [
            (json!("auto"), json!({"type":"auto"})),
            (json!("none"), json!({"type":"none"})),
            (json!("required"), json!({"type":"any"})),
            (json!({"type":"function","function":{"name":"get_weather"}}), json!({"type":"tool","name":"get_weather"})),
        ] {
            b["tool_choice"] = input;
            assert_eq!(request_to_anthropic(&b).unwrap()["tool_choice"], expected);
        }

        b["tool_choice"] = json!("bogus");
        assert!(request_to_anthropic(&b).is_err());
        b["tool_choice"] = json!("auto");
        b["tools"] = json!([{"type":"file_search"}]);
        assert!(request_to_anthropic(&b).is_err());
    }

    #[test]
    fn request_parallel_tool_calls_false_sets_disable_parallel() {
        let mut b = base(json!([{"role":"user","content":"hi"}]));
        b["tools"] = json!([{"type":"function","function":{"name":"f"}}]);
        b["parallel_tool_calls"] = json!(false);
        let out = request_to_anthropic(&b).unwrap();
        assert_eq!(out["tool_choice"], json!({"type":"auto","disable_parallel_tool_use":true}));
        b["tool_choice"] = json!("none");
        assert_eq!(request_to_anthropic(&b).unwrap()["tool_choice"], json!({"type":"none"}));
    }

    #[test]
    fn request_assistant_tool_calls_and_consecutive_tool_results() {
        let out = request_to_anthropic(&base(json!([
            {"role":"user","content":"weather?"},
            {"role":"assistant","content":null,"reasoning_content":"thinking...","tool_calls":[
                {"id":"call_1","type":"function","function":{"name":"get_weather","arguments":"{\"city\":\"SH\"}"}},
                {"id":"call_2","type":"function","function":{"name":"get_time","arguments":""}}
            ]},
            {"role":"tool","tool_call_id":"call_1","content":"sunny"},
            {"role":"tool","tool_call_id":"call_2","content":[{"type":"text","text":"12:00"}]},
            {"role":"user","content":"thanks"}
        ])))
        .unwrap();
        let msgs = out["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3, "tool_result 消息与后续 user 文本消息合并为一条");
        for pair in msgs.windows(2) {
            assert_ne!(pair[0]["role"], pair[1]["role"], "相邻消息不应同 role");
        }
        let asst = &msgs[1]["content"];
        assert_eq!(asst.as_array().unwrap().len(), 2, "空 content 不产生 text 块, reasoning_content 丢弃");
        assert_eq!(asst[0], json!({"type":"tool_use","id":"call_1","name":"get_weather","input":{"city":"SH"}}));
        assert_eq!(asst[1]["input"], json!({}), "空 arguments 视为 {{}}");
        // 两条 tool 结果 + 之后的 user 文本合并进同一条 user 消息
        assert_eq!(msgs[2]["role"], "user");
        assert_eq!(msgs[2]["content"], json!([
            {"type":"tool_result","tool_use_id":"call_1","content":"sunny"},
            {"type":"tool_result","tool_use_id":"call_2","content":"12:00"},
            {"type":"text","text":"thanks"}
        ]));
    }

    #[test]
    fn request_merges_adjacent_same_role_messages() {
        let out = request_to_anthropic(&base(json!([
            {"role":"user","content":"a"},
            {"role":"user","content":"b"},
            {"role":"assistant","content":"x"},
            {"role":"assistant","content":"y"},
            {"role":"user","content":"c"}
        ])))
        .unwrap();
        let msgs = out["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"], json!([
            {"type":"text","text":"a"},
            {"type":"text","text":"b"}
        ]));
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[1]["content"], json!([
            {"type":"text","text":"x"},
            {"type":"text","text":"y"}
        ]));
        assert_eq!(msgs[2]["role"], "user");
        assert_eq!(msgs[2]["content"], json!([{"type":"text","text":"c"}]));
    }

    #[test]
    fn request_assistant_text_with_tool_calls_keeps_both() {
        let out = request_to_anthropic(&base(json!([
            {"role":"user","content":"x"},
            {"role":"assistant","content":"Let me check","tool_calls":[
                {"id":"c","type":"function","function":{"name":"f","arguments":{"a":1}}}
            ]}
        ])))
        .unwrap();
        let asst = &out["messages"][1]["content"];
        assert_eq!(asst[0], json!({"type":"text","text":"Let me check"}));
        assert_eq!(asst[1]["input"], json!({"a":1}), "arguments 已是对象时直接采用");
    }

    #[test]
    fn request_invalid_tool_call_arguments_is_400_with_id() {
        let err = request_to_anthropic(&base(json!([
            {"role":"user","content":"x"},
            {"role":"assistant","tool_calls":[{"id":"call_bad","type":"function","function":{"name":"f","arguments":"{not json"}}]}
        ])))
        .unwrap_err();
        assert!(matches!(err, AppError::BadRequest(ref m) if m.contains("call_bad")), "{err:?}");
    }

    #[test]
    fn request_image_data_uri_and_http_url() {
        let out = request_to_anthropic(&base(json!([{"role":"user","content":[
            {"type":"text","text":"看图"},
            {"type":"image_url","image_url":{"url":"data:image/png;base64,AAAA","detail":"low"}},
            {"type":"image_url","image_url":"https://example.com/a.jpg"}
        ]}])))
        .unwrap();
        let c = &out["messages"][0]["content"];
        assert_eq!(c[0], json!({"type":"text","text":"看图"}));
        assert_eq!(c[1], json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAA"}}));
        assert_eq!(c[2], json!({"type":"image","source":{"type":"url","url":"https://example.com/a.jpg"}}));
        let bad = base(json!([{"role":"user","content":[{"type":"image_url","image_url":{"url":"ftp://x"}}]}]));
        assert!(request_to_anthropic(&bad).is_err());
        let audio = base(json!([{"role":"user","content":[{"type":"input_audio","input_audio":{}}]}]));
        assert!(request_to_anthropic(&audio).is_err());
    }

    #[test]
    fn request_rejects_legacy_functions_empty_messages_and_unknown_role() {
        let mut b = base(json!([{"role":"user","content":"x"}]));
        b["functions"] = json!([]);
        assert!(matches!(request_to_anthropic(&b).unwrap_err(), AppError::BadRequest(_)));
        assert!(request_to_anthropic(&base(json!([]))).is_err());
        assert!(request_to_anthropic(&json!({"model":"m"})).is_err());
        assert!(request_to_anthropic(&base(json!([{"role":"robot","content":"x"}]))).is_err());
        assert!(request_to_anthropic(&base(json!([{"role":"tool","content":"x"}]))).is_err(), "tool 缺 tool_call_id");
        assert!(request_to_anthropic(&base(json!([{"role":"system","content":"only system"}]))).is_err(), "没有任何对话消息");
        assert!(request_to_anthropic(&json!({"messages":[{"role":"user","content":"x"}]})).is_err(), "缺 model");
    }

    #[test]
    fn request_empty_user_text_is_400() {
        assert!(request_to_anthropic(&base(json!([{"role":"user","content":""}]))).is_err());
    }

    // ---- 响应侧 JSON ----

    fn anthropic_msg(content: Value, stop_reason: &str) -> Value {
        json!({
            "id": "msg_abc",
            "type": "message",
            "role": "assistant",
            "model": "model-opus",
            "content": content,
            "stop_reason": stop_reason,
            "usage": {"input_tokens": 10, "output_tokens": 5, "cache_creation_input_tokens": 3, "cache_read_input_tokens": 7}
        })
    }

    #[test]
    fn response_text_with_usage_and_cache() {
        let out = response_to_chat_json(&anthropic_msg(json!([{"type":"text","text":"Hi"}]), "end_turn"), "gpt-5.5");
        assert_eq!(out["id"], "chatcmpl-abc");
        assert_eq!(out["object"], "chat.completion");
        assert!(out["created"].as_i64().unwrap() > 1_700_000_000);
        assert_eq!(out["model"], "gpt-5.5", "回显客户端请求名, 不是虚拟名");
        let choice = &out["choices"][0];
        assert_eq!(choice["index"], 0);
        assert_eq!(choice["message"]["role"], "assistant");
        assert_eq!(choice["message"]["content"], "Hi");
        assert!(choice["message"].get("tool_calls").is_none());
        assert!(choice["message"].get("reasoning_content").is_none());
        assert_eq!(choice["finish_reason"], "stop");
        assert_eq!(out["usage"], json!({
            "prompt_tokens": 20, "completion_tokens": 5, "total_tokens": 25,
            "prompt_tokens_details": {"cached_tokens": 7}
        }));
    }

    #[test]
    fn response_tool_calls_and_thinking() {
        let out = response_to_chat_json(&anthropic_msg(json!([
            {"type":"thinking","thinking":"let me think","signature":"sig"},
            {"type":"text","text":"A"},
            {"type":"tool_use","id":"toolu_1","name":"get_weather","input":{"city":"SH"}},
            {"type":"text","text":"B"}
        ]), "tool_use"), "m");
        let m = &out["choices"][0]["message"];
        assert_eq!(m["content"], "AB");
        assert_eq!(m["reasoning_content"], "let me think");
        assert_eq!(m["tool_calls"], json!([{
            "id":"toolu_1","type":"function","function":{"name":"get_weather","arguments":"{\"city\":\"SH\"}"}
        }]));
        assert_eq!(out["choices"][0]["finish_reason"], "tool_calls");
    }

    #[test]
    fn response_empty_content_is_null_and_missing_id_gets_uuid() {
        let mut msg = anthropic_msg(json!([]), "max_tokens");
        msg.as_object_mut().unwrap().remove("id");
        let out = response_to_chat_json(&msg, "m");
        assert_eq!(out["choices"][0]["message"]["content"], Value::Null);
        assert_eq!(out["choices"][0]["finish_reason"], "length");
        assert!(out["id"].as_str().unwrap().starts_with("chatcmpl-"));
        assert!(out["id"].as_str().unwrap().len() > "chatcmpl-".len());
    }

    #[test]
    fn finish_reason_table() {
        assert_eq!(map_finish_reason(Some("end_turn")), "stop");
        assert_eq!(map_finish_reason(Some("stop_sequence")), "stop");
        assert_eq!(map_finish_reason(None), "stop");
        assert_eq!(map_finish_reason(Some("max_tokens")), "length");
        assert_eq!(map_finish_reason(Some("tool_use")), "tool_calls");
        assert_eq!(map_finish_reason(Some("refusal")), "content_filter");
        assert_eq!(map_finish_reason(Some("something_new")), "stop");
    }

    #[test]
    fn finish_reason_for_table() {
        assert_eq!(finish_reason_for(Some("end_turn"), true), "tool_calls");
        assert_eq!(finish_reason_for(None, true), "tool_calls");
        assert_eq!(finish_reason_for(Some("max_tokens"), true), "length");
        assert_eq!(finish_reason_for(Some("refusal"), true), "content_filter");
        assert_eq!(finish_reason_for(Some("end_turn"), false), "stop");
        assert_eq!(finish_reason_for(Some("tool_use"), false), "tool_calls");
    }

    #[test]
    fn response_tool_calls_with_end_turn_reports_tool_calls() {
        let content = json!([
            {"type":"tool_use","id":"toolu_1","name":"get_weather","input":{"city":"SH"}}
        ]);
        let out = response_to_chat_json(&anthropic_msg(content.clone(), "end_turn"), "m");
        assert_eq!(
            out["choices"][0]["finish_reason"], "tool_calls",
            "翻译层上游把 tool_use 误报成 end_turn 时仍要报 tool_calls, 否则 OpenAI 客户端不执行工具"
        );
        let out2 = response_to_chat_json(&anthropic_msg(content, "max_tokens"), "m");
        assert_eq!(out2["choices"][0]["finish_reason"], "length", "max_tokens 优先级高于 tool_use 判定");
    }

    #[test]
    fn usage_missing_fields_default_zero() {
        assert_eq!(usage_to_chat(None), json!({
            "prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0,
            "prompt_tokens_details": {"cached_tokens": 0}
        }));
        assert_eq!(usage_to_chat(Some(&json!({"input_tokens": 4})))["total_tokens"], 4);
    }

    #[test]
    fn error_body_maps_type_and_keeps_code() {
        let e = chat_error_body("overloaded_error", "busy");
        assert_eq!(e, json!({"error":{"message":"busy","type":"rate_limit_error","code":"overloaded_error","param":null}}));
        assert_eq!(chat_error_body("rate_limit_error", "x")["error"]["type"], "rate_limit_error");
        assert_eq!(chat_error_body("invalid_request_error", "x")["error"]["type"], "invalid_request_error");
        assert_eq!(chat_error_body("authentication_error", "x")["error"]["type"], "invalid_request_error");
        assert_eq!(chat_error_body("permission_error", "x")["error"]["type"], "invalid_request_error");
        assert_eq!(chat_error_body("api_error", "x")["error"]["type"], "server_error");
        assert_eq!(chat_error_body("api_error", "x")["error"]["code"], "api_error");
    }

    // ---- SSE 状态机 ----

    /// 解析 "data: {...}\n\n" 帧为 JSON; "[DONE]" 返回 None.
    fn frame_json(frame: &str) -> Option<Value> {
        let payload = frame.strip_prefix("data: ").unwrap().trim_end();
        if payload == "[DONE]" { None } else { Some(serde_json::from_str(payload).unwrap()) }
    }

    fn feed_all(conv: &mut AnthropicToChatSseConverter, events: &[(&str, Value)]) -> Vec<String> {
        let mut out = Vec::new();
        for (name, data) in events {
            out.extend(conv.feed(name, data));
        }
        out
    }

    fn msg_start() -> (&'static str, Value) {
        ("message_start", json!({"type":"message_start","message":{"id":"msg_1","model":"model-opus","usage":{"input_tokens":10,"cache_read_input_tokens":2,"cache_creation_input_tokens":0,"output_tokens":1}}}))
    }

    #[test]
    fn sse_text_flow_emits_role_content_finish_usage_done() {
        let mut conv = AnthropicToChatSseConverter::new("gpt-5.5".into());
        let frames = feed_all(&mut conv, &[
            msg_start(),
            ("ping", json!({"type":"ping"})),
            ("content_block_start", json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
            ("content_block_delta", json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hel"}})),
            ("content_block_delta", json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"lo"}})),
            ("content_block_stop", json!({"type":"content_block_stop","index":0})),
            ("message_delta", json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}})),
            ("message_stop", json!({"type":"message_stop"})),
        ]);
        assert!(frames.iter().all(|f| f.starts_with("data: ") && f.ends_with("\n\n")));
        let jsons: Vec<Option<Value>> = frames.iter().map(|f| frame_json(f)).collect();
        // 首帧 role
        let first = jsons[0].as_ref().unwrap();
        assert_eq!(first["id"], "chatcmpl-1");
        assert_eq!(first["object"], "chat.completion.chunk");
        assert_eq!(first["model"], "gpt-5.5");
        assert_eq!(first["choices"][0]["delta"], json!({"role":"assistant","content":""}));
        assert_eq!(first["choices"][0]["finish_reason"], Value::Null);
        // 文本增量
        assert_eq!(jsons[1].as_ref().unwrap()["choices"][0]["delta"]["content"], "Hel");
        assert_eq!(jsons[2].as_ref().unwrap()["choices"][0]["delta"]["content"], "lo");
        // finish 帧
        let fin = jsons[3].as_ref().unwrap();
        assert_eq!(fin["choices"][0]["delta"], json!({}));
        assert_eq!(fin["choices"][0]["finish_reason"], "stop");
        // usage 帧: choices 空 + usage
        let usage = jsons[4].as_ref().unwrap();
        assert_eq!(usage["choices"], json!([]));
        assert_eq!(usage["usage"], json!({"prompt_tokens":12,"completion_tokens":5,"total_tokens":17,"prompt_tokens_details":{"cached_tokens":2}}));
        // [DONE]
        assert!(jsons[5].is_none());
        assert_eq!(frames.len(), 6);
        // 之后再 feed / finalize 不再输出
        assert!(conv.feed("message_stop", &json!({})).is_empty());
        assert!(conv.finalize_if_needed().is_empty());
    }

    #[test]
    fn sse_thinking_and_two_parallel_tool_calls() {
        let mut conv = AnthropicToChatSseConverter::new("m".into());
        let frames = feed_all(&mut conv, &[
            msg_start(),
            ("content_block_start", json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}})),
            ("content_block_delta", json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}})),
            ("content_block_delta", json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}})),
            ("content_block_stop", json!({"type":"content_block_stop","index":0})),
            ("content_block_start", json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_a","name":"get_weather","input":{}}})),
            ("content_block_delta", json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"city\""}})),
            ("content_block_start", json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_b","name":"get_time","input":{}}})),
            ("content_block_delta", json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":":\"SH\"}"}})),
            ("content_block_delta", json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{}"}})),
            ("content_block_stop", json!({"type":"content_block_stop","index":1})),
            ("content_block_stop", json!({"type":"content_block_stop","index":2})),
            ("message_delta", json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}})),
            ("message_stop", json!({"type":"message_stop"})),
        ]);
        let deltas: Vec<Value> = frames.iter().filter_map(|f| frame_json(f)).map(|j| j["choices"].get(0).map(|c| c["delta"].clone()).unwrap_or(Value::Null)).collect();
        assert_eq!(deltas[1], json!({"reasoning_content":"hmm"}));
        // signature_delta 不输出, 所以下一个是 tool_call 开头帧
        assert_eq!(deltas[2], json!({"tool_calls":[{"index":0,"id":"toolu_a","type":"function","function":{"name":"get_weather","arguments":""}}]}));
        assert_eq!(deltas[3], json!({"tool_calls":[{"index":0,"function":{"arguments":"{\"city\""}}]}));
        assert_eq!(deltas[4], json!({"tool_calls":[{"index":1,"id":"toolu_b","type":"function","function":{"name":"get_time","arguments":""}}]}));
        assert_eq!(deltas[5], json!({"tool_calls":[{"index":0,"function":{"arguments":":\"SH\"}"}}]}));
        assert_eq!(deltas[6], json!({"tool_calls":[{"index":1,"function":{"arguments":"{}"}}]}));
        assert_eq!(deltas[7], json!({}));
        let fin = frame_json(&frames[7]).unwrap();
        assert_eq!(fin["choices"][0]["finish_reason"], "tool_calls");
    }

    #[test]
    fn sse_tool_use_with_end_turn_reports_tool_calls() {
        let mut conv = AnthropicToChatSseConverter::new("m".into());
        let frames = feed_all(&mut conv, &[
            msg_start(),
            ("content_block_start", json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_a","name":"get_weather","input":{}}})),
            ("content_block_delta", json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{}"}})),
            ("content_block_stop", json!({"type":"content_block_stop","index":0})),
            ("message_delta", json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}})),
            ("message_stop", json!({"type":"message_stop"})),
        ]);
        let fin = frames.iter().find_map(|f| {
            let j = frame_json(f)?;
            let fr = j["choices"][0]["finish_reason"].clone();
            (!fr.is_null()).then_some(fr)
        });
        assert_eq!(fin, Some(json!("tool_calls")), "上游把 tool_use 回成 end_turn 时 SSE 也要报 tool_calls");
    }

    #[test]
    fn sse_finalize_without_message_stop_adds_finish_usage_done() {
        let mut conv = AnthropicToChatSseConverter::new("m".into());
        let mut frames = feed_all(&mut conv, &[
            msg_start(),
            ("content_block_start", json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
            ("content_block_delta", json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial"}})),
        ]);
        assert_eq!(frames.len(), 2);
        frames.extend(conv.finalize_if_needed());
        assert_eq!(frames.len(), 5, "补 finish + usage + [DONE]");
        assert_eq!(frame_json(&frames[2]).unwrap()["choices"][0]["finish_reason"], "stop");
        assert_eq!(frame_json(&frames[3]).unwrap()["choices"], json!([]));
        assert!(frame_json(&frames[4]).is_none());
        assert!(conv.finalize_if_needed().is_empty());
    }

    #[test]
    fn sse_finalize_after_message_delta_does_not_duplicate_finish() {
        let mut conv = AnthropicToChatSseConverter::new("m".into());
        let mut frames = feed_all(&mut conv, &[
            msg_start(),
            ("message_delta", json!({"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":1}})),
        ]);
        frames.extend(conv.finalize_if_needed());
        // role + finish(length) + usage + DONE
        assert_eq!(frames.len(), 4);
        assert_eq!(frame_json(&frames[1]).unwrap()["choices"][0]["finish_reason"], "length");
        assert_eq!(frame_json(&frames[2]).unwrap()["choices"], json!([]));
    }

    #[test]
    fn sse_error_event_emits_error_then_done() {
        let mut conv = AnthropicToChatSseConverter::new("m".into());
        let frames = feed_all(&mut conv, &[
            msg_start(),
            ("error", json!({"type":"error","error":{"type":"overloaded_error","message":"busy"}})),
            ("message_stop", json!({"type":"message_stop"})),
        ]);
        assert_eq!(frames.len(), 3);
        let err = frame_json(&frames[1]).unwrap();
        assert_eq!(err["error"]["type"], "rate_limit_error");
        assert_eq!(err["error"]["code"], "overloaded_error");
        assert_eq!(err["error"]["message"], "busy");
        assert!(frame_json(&frames[2]).is_none());
        assert!(conv.finalize_if_needed().is_empty());
    }

    #[test]
    fn sse_nothing_before_message_start_and_no_finalize_when_never_started() {
        let mut conv = AnthropicToChatSseConverter::new("m".into());
        assert!(conv.feed("content_block_delta", &json!({"index":0,"delta":{"type":"text_delta","text":"x"}})).is_empty());
        assert!(conv.finalize_if_needed().is_empty());
    }

    #[test]
    fn sse_error_before_message_start_still_emits_error_and_done() {
        let mut conv = AnthropicToChatSseConverter::new("m".into());
        let frames = conv.feed("error", &json!({"type":"error","error":{"type":"authentication_error","message":"bad key"}}));
        assert_eq!(frames.len(), 2);
        let err = frame_json(&frames[0]).unwrap();
        assert_eq!(err["error"]["type"], "invalid_request_error");
        assert_eq!(err["error"]["code"], "authentication_error");
        assert_eq!(err["error"]["message"], "bad key");
        assert!(frame_json(&frames[1]).is_none(), "[DONE]");
        // 之后 message_start 也不再输出
        assert!(conv.feed("message_start", &json!({"type":"message_start","message":{"id":"msg_1"}})).is_empty());
        assert!(conv.finalize_if_needed().is_empty());
    }
}
