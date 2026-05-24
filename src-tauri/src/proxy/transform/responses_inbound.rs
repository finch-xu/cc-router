//! OpenAI Responses → Anthropic Messages 反向翻译层 (入站方向).
//!
//! 配对方: [`responses_common`] (出站方向, Anthropic → OpenAI Responses).
//!
//! 用途: cc-router 对外的 `POST /v1/responses` 兼容入口 ([`handler::responses`]) 用本模块把
//! OpenAI Responses 请求翻译成 Anthropic Messages 走现有 pipeline, 再把 Anthropic 响应/SSE
//! 翻译回 OpenAI Responses 给客户端。
//!
//! ## 边界
//!
//! 本模块的函数都是纯函数 + 同步状态机, 不发起任何网络请求, 不读 DB / oauth metadata.
//! pipeline 不变, 上游全部 9 家 provider + codex/openai/gemini/kiro 路径零改动。
//!
//! ## 不支持
//!
//! - multimodal (image_url / image output) — 上游 Anthropic 透传不支持
//! - file_search / web_search / computer_use — OpenAI 独有 tool 类型
//! - parallel_tool_calls 字段 — 直接忽略

use std::collections::HashMap;

use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::proxy::transform::responses_common::{
    decode_reasoning_signature, encode_reasoning_signature,
};

// ============================================================
// 请求侧: OpenAI Responses request → Anthropic Messages request
// ============================================================

/// OpenAI Responses 请求体 → Anthropic Messages 请求体.
///
/// 入口字段映射:
/// - `instructions` (str) → `system` [{type:text,text}]
/// - `input` (items[]) → `messages` [{role, content[]}]
/// - `tools[]` (function spec) → `tools[]` (Anthropic spec)
/// - `tool_choice` → `tool_choice` (反向)
/// - `max_output_tokens` → `max_tokens` (无则默认 4096, Anthropic 必填)
/// - `temperature`/`top_p` → 同名透传
/// - `reasoning.effort` → `thinking {type:enabled, budget_tokens?}`
/// - `stream` → `stream`
pub fn request_to_anthropic(body: &Value) -> AppResult<Value> {
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::BadRequest("请求 body 缺少 model".into()))?;

    let mut out = json!({ "model": model });

    if let Some(s) = body.get("stream") {
        out["stream"] = s.clone();
    }

    // 1. 顶层 instructions 文本 (可能为空)
    let mut sys_text = body
        .get("instructions")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_default();

    // 2. input → messages (并把 role:developer/system 的 message 抽到 system 文本)
    if let Some(input) = body.get("input").and_then(|v| v.as_array()) {
        let (msgs, sys_from_input) = input_items_to_messages(input);
        out["messages"] = json!(msgs);
        if !sys_from_input.is_empty() {
            if !sys_text.is_empty() {
                sys_text.push_str("\n\n");
            }
            sys_text.push_str(&sys_from_input);
        }
    } else if let Some(input_text) = body.get("input").and_then(|v| v.as_str()) {
        // OpenAI Responses 容错: input 也可能是单个字符串
        out["messages"] = json!([{
            "role": "user",
            "content": [{"type": "text", "text": input_text}],
        }]);
    }

    // 3. 写顶层 system (instructions + developer/system role 文本合并后)
    if !sys_text.is_empty() {
        out["system"] = json!([{"type": "text", "text": sys_text}]);
    }

    // tools[] → tools[]
    if let Some(tools) = body.get("tools").and_then(|v| v.as_array()) {
        let converted: Vec<Value> = tools.iter().filter_map(convert_tool_to_anthropic).collect();
        if !converted.is_empty() {
            out["tools"] = json!(converted);
        }
    }

    // tool_choice
    if let Some(tc) = body.get("tool_choice") {
        if let Some(mapped) = map_tool_choice_to_anthropic(tc) {
            out["tool_choice"] = mapped;
        }
    }

    // 透传字段
    for key in ["temperature", "top_p"] {
        if let Some(v) = body.get(key) {
            out[key] = v.clone();
        }
    }

    // max_output_tokens → max_tokens (Anthropic 要求必填)
    let max_tokens = body
        .get("max_output_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(4096);
    out["max_tokens"] = json!(max_tokens);

    // reasoning.effort → thinking
    if let Some(reasoning) = body.get("reasoning") {
        if let Some(effort) = reasoning.get("effort").and_then(|v| v.as_str()) {
            out["thinking"] = json!({
                "type": "enabled",
                "budget_tokens": effort_to_budget_tokens(effort),
            });
        }
    }

    Ok(out)
}

/// OpenAI reasoning_effort → Anthropic thinking.budget_tokens.
///
/// 映射依据: 与 [`responses_common`] 里的反向映射 (`resolve_reasoning_effort`) 阈值对称。
/// minimal=1024 / low=2048 / medium=8192 / high=16384 / xhigh→high。
fn effort_to_budget_tokens(effort: &str) -> i64 {
    match effort {
        "minimal" => 1024,
        "low" => 2048,
        "medium" => 8192,
        "high" | "xhigh" => 16384,
        _ => 8192,
    }
}

/// OpenAI Responses 的 input items 数组 → (Anthropic messages 数组, 抽出的 system 文本).
///
/// 关键: OpenAI 是「扁平 item 流」(message / function_call / function_call_output / reasoning
/// 各自一个 item), 而 Anthropic 是「按 role 合并的 message」(同一 turn 的 text + tool_use 在一个
/// assistant message 里). 需要合并相邻同 role 的 items 到一个 message。
///
/// **role 过滤**: OpenAI 协议允许 `role: "developer"` (GPT-4o+ 引入, 比 system 优先级更高)
/// 和 `role: "system"`, 但 Anthropic messages 只接受 user/assistant (system 是顶层独立字段)。
/// developer/system role 的 message item 文本抽到第二个返回值, 由调用方合并到顶层 system。
/// 未知 role 兜底退化为 user (防止上游严格 provider 直接 400 拒绝)。
fn input_items_to_messages(items: &[Value]) -> (Vec<Value>, String) {
    let mut messages: Vec<Value> = Vec::new();
    let mut current_role: Option<String> = None;
    let mut current_content: Vec<Value> = Vec::new();
    let mut system_acc = String::new();

    fn flush(messages: &mut Vec<Value>, role: &mut Option<String>, content: &mut Vec<Value>) {
        if let Some(r) = role.take() {
            if !content.is_empty() {
                messages.push(json!({
                    "role": r,
                    "content": std::mem::take(content),
                }));
            }
        }
    }

    // 从一个 message item 的 content 字段抽纯文本 (拼 input_text/output_text part 的 text).
    // OpenAI 也允许 content 是字符串而非数组, 兼容处理。
    fn extract_message_text(item: &Value) -> String {
        if let Some(parts) = item.get("content").and_then(|v| v.as_array()) {
            let mut out = String::new();
            for part in parts {
                let ptype = part.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if ptype == "input_text" || ptype == "output_text" {
                    if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                        if !out.is_empty() {
                            out.push('\n');
                        }
                        out.push_str(text);
                    }
                }
            }
            out
        } else if let Some(text) = item.get("content").and_then(|v| v.as_str()) {
            text.to_string()
        } else {
            String::new()
        }
    }

    for item in items {
        let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match item_type {
            "message" => {
                let raw_role = item
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("user");

                // developer/system role 抽到顶层 system, 不进 messages
                if raw_role == "developer" || raw_role == "system" {
                    flush(&mut messages, &mut current_role, &mut current_content);
                    let text = extract_message_text(item);
                    if !text.is_empty() {
                        if !system_acc.is_empty() {
                            system_acc.push_str("\n\n");
                        }
                        system_acc.push_str(&text);
                    }
                    continue;
                }

                // 未知 role 兜底退化为 user (防严格上游 400)
                let role = if raw_role == "assistant" { "assistant" } else { "user" }.to_string();
                if current_role.as_deref() != Some(&role) {
                    flush(&mut messages, &mut current_role, &mut current_content);
                    current_role = Some(role);
                }
                if let Some(parts) = item.get("content").and_then(|v| v.as_array()) {
                    for part in parts {
                        let ptype = part.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        if ptype == "input_text" || ptype == "output_text" {
                            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                                current_content
                                    .push(json!({"type": "text", "text": text}));
                            }
                        }
                        // input_image / 其他类型暂不支持
                    }
                } else if let Some(text) = item.get("content").and_then(|v| v.as_str()) {
                    // 容错: content 可能是字符串 (OpenAI Responses 也允许)
                    current_content.push(json!({"type": "text", "text": text}));
                }
            }
            "function_call" => {
                // function_call 必须落在 assistant message 里
                if current_role.as_deref() != Some("assistant") {
                    flush(&mut messages, &mut current_role, &mut current_content);
                    current_role = Some("assistant".to_string());
                }
                let call_id = item.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let arguments_str =
                    item.get("arguments").and_then(|v| v.as_str()).unwrap_or("{}");
                let input: Value = serde_json::from_str(arguments_str).unwrap_or(json!({}));
                current_content.push(json!({
                    "type": "tool_use",
                    "id": call_id,
                    "name": name,
                    "input": input,
                }));
            }
            "function_call_output" => {
                // function_call_output 必须落在 user message 里 (Anthropic tool_result 约定)
                if current_role.as_deref() != Some("user") {
                    flush(&mut messages, &mut current_role, &mut current_content);
                    current_role = Some("user".to_string());
                }
                let call_id = item.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                let output = item.get("output").and_then(|v| v.as_str()).unwrap_or("");
                current_content.push(json!({
                    "type": "tool_result",
                    "tool_use_id": call_id,
                    "content": output,
                }));
            }
            "reasoning" => {
                // reasoning 属于 assistant 上一轮的输出, 落在 assistant message 里
                if current_role.as_deref() != Some("assistant") {
                    flush(&mut messages, &mut current_role, &mut current_content);
                    current_role = Some("assistant".to_string());
                }
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let encrypted_content = item
                    .get("encrypted_content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !id.is_empty() && !encrypted_content.is_empty() {
                    let signature = encode_reasoning_signature(id, encrypted_content);
                    let summary_text = item
                        .get("summary")
                        .and_then(|v| v.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|s| s.get("text"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    current_content.push(json!({
                        "type": "thinking",
                        "thinking": summary_text,
                        "signature": signature,
                    }));
                }
            }
            _ => {} // image / unknown → 忽略
        }
    }
    flush(&mut messages, &mut current_role, &mut current_content);
    (messages, system_acc)
}

fn convert_tool_to_anthropic(t: &Value) -> Option<Value> {
    // OpenAI Responses tool: {"type":"function","name":...,"description":...,"parameters":{...}}
    // Anthropic: {"name":..., "description":..., "input_schema":{...}}
    if t.get("type").and_then(|v| v.as_str()) != Some("function") {
        return None;
    }
    let name = t.get("name").and_then(|v| v.as_str())?;
    let description = t.get("description").cloned().unwrap_or(Value::Null);
    let schema = t.get("parameters").cloned().unwrap_or(json!({}));
    Some(json!({
        "name": name,
        "description": description,
        "input_schema": schema,
    }))
}

fn map_tool_choice_to_anthropic(tc: &Value) -> Option<Value> {
    if let Some(s) = tc.as_str() {
        match s {
            "auto" => return Some(json!({"type": "auto"})),
            "required" | "any" => return Some(json!({"type": "any"})),
            // OpenAI "none" 在 Anthropic 没对应, 直接省略让 Anthropic 默认 auto
            "none" => return None,
            _ => return None,
        }
    }
    if let Some(obj) = tc.as_object() {
        if obj.get("type").and_then(|v| v.as_str()) == Some("function") {
            if let Some(name) = obj.get("name").and_then(|v| v.as_str()) {
                return Some(json!({"type": "tool", "name": name}));
            }
        }
    }
    None
}

// ============================================================
// 响应侧 JSON: Anthropic Message → OpenAI Responses Response
// ============================================================

/// Anthropic Message JSON → OpenAI Responses Response JSON (非流式).
///
/// 顶层结构:
/// ```text
/// {
///   "id": "resp_...",
///   "object": "response",
///   "created_at": <unix>,
///   "status": "completed" | "incomplete",
///   "model": "...",
///   "output": [<output_items>],
///   "usage": {input_tokens, output_tokens, total_tokens}
/// }
/// ```
pub fn response_to_responses_json(anthropic_msg: &Value) -> Value {
    let id = anthropic_msg
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| format!("resp_{}", Uuid::new_v4().simple()));
    let model = anthropic_msg
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let stop_reason = anthropic_msg
        .get("stop_reason")
        .and_then(|v| v.as_str())
        .unwrap_or("end_turn");
    let (status, incomplete_details) = anthropic_stop_reason_to_responses_status(stop_reason);

    let mut output_items: Vec<Value> = Vec::new();
    if let Some(content) = anthropic_msg.get("content").and_then(|v| v.as_array()) {
        let mut message_parts: Vec<Value> = Vec::new();
        for block in content {
            let btype = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match btype {
                "text" => {
                    let text = block.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    message_parts.push(json!({
                        "type": "output_text",
                        "text": text,
                        "annotations": [],
                    }));
                }
                "tool_use" => {
                    // tool_use 落在独立的 function_call item 里, 先 flush 之前的 message
                    flush_message_item(&mut output_items, &mut message_parts);
                    let call_id =
                        block.get("id").and_then(|v| v.as_str()).unwrap_or("call_unknown");
                    let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let arguments = block
                        .get("input")
                        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "{}".into()))
                        .unwrap_or_else(|| "{}".into());
                    output_items.push(json!({
                        "type": "function_call",
                        "id": format!("fc_{}", Uuid::new_v4().simple()),
                        "call_id": call_id,
                        "name": name,
                        "arguments": arguments,
                        "status": "completed",
                    }));
                }
                "thinking" => {
                    flush_message_item(&mut output_items, &mut message_parts);
                    let signature =
                        block.get("signature").and_then(|v| v.as_str()).unwrap_or("");
                    let summary_text =
                        block.get("thinking").and_then(|v| v.as_str()).unwrap_or("");
                    // signature → (id, encrypted_content). 用 cc-router 自家编码方案 (v=2/p=openai_responses).
                    // 老 build 的 v=1 也兼容; 其他 provider (gemini) 的 signature 解码会失败 → 退化 id/ec 都空。
                    let (reason_id, encrypted) = decode_reasoning_signature(signature)
                        .unwrap_or_else(|| (format!("rs_{}", Uuid::new_v4().simple()), String::new()));
                    let summary = if summary_text.is_empty() {
                        json!([])
                    } else {
                        json!([{"type": "summary_text", "text": summary_text}])
                    };
                    output_items.push(json!({
                        "type": "reasoning",
                        "id": reason_id,
                        "encrypted_content": encrypted,
                        "summary": summary,
                    }));
                }
                _ => {} // image / unknown → 忽略
            }
        }
        flush_message_item(&mut output_items, &mut message_parts);
    }

    let usage = anthropic_usage_to_responses(anthropic_msg.get("usage"));

    json!({
        "id": id,
        "object": "response",
        "created_at": current_unix_ts(),
        "status": status,
        "incomplete_details": incomplete_details,
        "model": model,
        "output": output_items,
        "usage": usage,
        "parallel_tool_calls": true,
    })
}

fn flush_message_item(out: &mut Vec<Value>, parts: &mut Vec<Value>) {
    if !parts.is_empty() {
        out.push(json!({
            "type": "message",
            "id": format!("msg_{}", Uuid::new_v4().simple()),
            "role": "assistant",
            "status": "completed",
            "content": std::mem::take(parts),
        }));
    }
}

fn anthropic_usage_to_responses(usage: Option<&Value>) -> Value {
    let input = usage.and_then(|u| u.get("input_tokens")).and_then(|v| v.as_i64()).unwrap_or(0);
    let output = usage.and_then(|u| u.get("output_tokens")).and_then(|v| v.as_i64()).unwrap_or(0);
    json!({
        "input_tokens": input,
        "output_tokens": output,
        "total_tokens": input + output,
    })
}

/// Anthropic stop_reason → (OpenAI status, incomplete_details).
pub fn anthropic_stop_reason_to_responses_status(stop_reason: &str) -> (&'static str, Value) {
    match stop_reason {
        "end_turn" | "stop_sequence" => ("completed", Value::Null),
        "tool_use" => ("completed", Value::Null),
        "max_tokens" => (
            "incomplete",
            json!({"reason": "max_output_tokens"}),
        ),
        _ => ("completed", Value::Null),
    }
}

fn current_unix_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ============================================================
// 响应侧 SSE: Anthropic events → OpenAI Responses events
// ============================================================

/// Anthropic Messages SSE 事件 → OpenAI Responses SSE 事件状态机.
///
/// 输入: Anthropic 事件 (`message_start` / `content_block_start` / `content_block_delta` /
/// `content_block_stop` / `message_delta` / `message_stop`).
/// 输出: OpenAI Responses 事件 (`response.created` / `response.output_item.added` /
/// `response.output_text.delta` / `response.function_call_arguments.delta` / `response.completed` 等).
///
/// 与 [`responses_common::ResponsesSseConverter`] 完全镜像 (反向状态机)。
pub struct AnthropicToResponsesSseConverter {
    response_id: String,
    response_model: String,
    started: bool,
    completed: bool,
    /// 内容块状态: Anthropic content_block_index → ItemKind + sequence_number.
    blocks: HashMap<u32, InboundBlock>,
    /// 累积的文本/参数, 用于 done 事件携带完整字段.
    text_acc: HashMap<u32, String>,
    args_acc: HashMap<u32, String>,
    thinking_acc: HashMap<u32, String>,
    /// 已分配的 OpenAI output_index 计数器.
    next_output_index: u32,
    /// OpenAI 要求 SSE 事件递增 sequence_number.
    seq: u32,
    /// 最终 usage / stop_reason, 由 message_delta 拿到.
    final_stop_reason: String,
    final_usage: Value,
}

#[derive(Debug)]
struct InboundBlock {
    kind: InboundKind,
    output_index: u32,
    item_id: String,
    /// 仅对 tool_use 块需要 (function_call 的 call_id / name).
    tool_call_id: String,
    tool_name: String,
    /// 仅对 reasoning 块需要.
    reasoning_signature: String,
}

#[derive(Debug)]
enum InboundKind {
    Text,
    ToolUse,
    Thinking,
}

impl Default for AnthropicToResponsesSseConverter {
    fn default() -> Self {
        Self::new()
    }
}

impl AnthropicToResponsesSseConverter {
    pub fn new() -> Self {
        Self {
            response_id: String::new(),
            response_model: String::new(),
            started: false,
            completed: false,
            blocks: HashMap::new(),
            text_acc: HashMap::new(),
            args_acc: HashMap::new(),
            thinking_acc: HashMap::new(),
            next_output_index: 0,
            seq: 0,
            final_stop_reason: "end_turn".to_string(),
            final_usage: json!({"input_tokens": 0, "output_tokens": 0}),
        }
    }

    /// 喂入一帧 Anthropic SSE (`event_name`, `data`), 返回若干 OpenAI Responses SSE 帧 (已序列化为字符串).
    pub fn feed(&mut self, event_name: &str, data: &Value) -> Vec<String> {
        match event_name {
            "message_start" => self.handle_message_start(data),
            "content_block_start" => self.handle_content_block_start(data),
            "content_block_delta" => self.handle_content_block_delta(data),
            "content_block_stop" => self.handle_content_block_stop(data),
            "message_delta" => self.handle_message_delta(data),
            "message_stop" => self.handle_message_stop(),
            "ping" | "error" => Vec::new(), // 忽略
            _ => Vec::new(),
        }
    }

    /// 流结束兜底: 没收到 message_stop 时, 至少 emit response.completed.
    pub fn finalize_if_needed(&mut self) -> Vec<String> {
        if !self.completed && self.started {
            self.handle_message_stop()
        } else {
            Vec::new()
        }
    }

    // ---------- handlers ----------

    fn handle_message_start(&mut self, data: &Value) -> Vec<String> {
        if self.started {
            return Vec::new();
        }
        self.started = true;
        let msg = data.get("message");
        self.response_id = msg
            .and_then(|m| m.get("id"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("resp_{}", Uuid::new_v4().simple()));
        self.response_model = msg
            .and_then(|m| m.get("model"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let response_obj = self.make_response_obj("in_progress", Value::Null);
        vec![
            self.emit("response.created", json!({"response": response_obj.clone()})),
            self.emit("response.in_progress", json!({"response": response_obj})),
        ]
    }

    fn handle_content_block_start(&mut self, data: &Value) -> Vec<String> {
        let index = data.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let cb = match data.get("content_block") {
            Some(v) => v,
            None => return Vec::new(),
        };
        let btype = cb.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let output_index = self.next_output_index;
        self.next_output_index += 1;
        match btype {
            "text" => {
                let item_id = format!("msg_{}", Uuid::new_v4().simple());
                self.blocks.insert(
                    index,
                    InboundBlock {
                        kind: InboundKind::Text,
                        output_index,
                        item_id: item_id.clone(),
                        tool_call_id: String::new(),
                        tool_name: String::new(),
                        reasoning_signature: String::new(),
                    },
                );
                let item = json!({
                    "type": "message",
                    "id": item_id,
                    "role": "assistant",
                    "status": "in_progress",
                    "content": [],
                });
                vec![
                    self.emit(
                        "response.output_item.added",
                        json!({"output_index": output_index, "item": item}),
                    ),
                    self.emit(
                        "response.content_part.added",
                        json!({
                            "output_index": output_index,
                            "content_index": 0,
                            "part": {"type": "output_text", "text": "", "annotations": []},
                        }),
                    ),
                ]
            }
            "tool_use" => {
                let call_id =
                    cb.get("id").and_then(|v| v.as_str()).unwrap_or("call_unknown").to_string();
                let name =
                    cb.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let item_id = format!("fc_{}", Uuid::new_v4().simple());
                self.blocks.insert(
                    index,
                    InboundBlock {
                        kind: InboundKind::ToolUse,
                        output_index,
                        item_id: item_id.clone(),
                        tool_call_id: call_id.clone(),
                        tool_name: name.clone(),
                        reasoning_signature: String::new(),
                    },
                );
                let item = json!({
                    "type": "function_call",
                    "id": item_id,
                    "call_id": call_id,
                    "name": name,
                    "arguments": "",
                    "status": "in_progress",
                });
                vec![self.emit(
                    "response.output_item.added",
                    json!({"output_index": output_index, "item": item}),
                )]
            }
            "thinking" => {
                let item_id = format!("rs_{}", Uuid::new_v4().simple());
                self.blocks.insert(
                    index,
                    InboundBlock {
                        kind: InboundKind::Thinking,
                        output_index,
                        item_id: item_id.clone(),
                        tool_call_id: String::new(),
                        tool_name: String::new(),
                        reasoning_signature: String::new(),
                    },
                );
                let item = json!({
                    "type": "reasoning",
                    "id": item_id,
                    "summary": [],
                    "encrypted_content": null,
                });
                vec![self.emit(
                    "response.output_item.added",
                    json!({"output_index": output_index, "item": item}),
                )]
            }
            _ => {
                // 未知 block, 回滚 output_index
                self.next_output_index -= 1;
                Vec::new()
            }
        }
    }

    fn handle_content_block_delta(&mut self, data: &Value) -> Vec<String> {
        let index = data.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let delta = match data.get("delta") {
            Some(v) => v,
            None => return Vec::new(),
        };
        let dtype = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let Some(block) = self.blocks.get(&index) else {
            return Vec::new();
        };
        let output_index = block.output_index;
        let item_id = block.item_id.clone();
        match dtype {
            "text_delta" => {
                let text = delta.get("text").and_then(|v| v.as_str()).unwrap_or("");
                if text.is_empty() {
                    return Vec::new();
                }
                self.text_acc.entry(index).or_default().push_str(text);
                vec![self.emit(
                    "response.output_text.delta",
                    json!({
                        "output_index": output_index,
                        "item_id": item_id,
                        "content_index": 0,
                        "delta": text,
                    }),
                )]
            }
            "input_json_delta" => {
                let partial =
                    delta.get("partial_json").and_then(|v| v.as_str()).unwrap_or("");
                if partial.is_empty() {
                    return Vec::new();
                }
                self.args_acc.entry(index).or_default().push_str(partial);
                vec![self.emit(
                    "response.function_call_arguments.delta",
                    json!({
                        "output_index": output_index,
                        "item_id": item_id,
                        "delta": partial,
                    }),
                )]
            }
            "thinking_delta" => {
                let text = delta.get("thinking").and_then(|v| v.as_str()).unwrap_or("");
                if text.is_empty() {
                    return Vec::new();
                }
                self.thinking_acc.entry(index).or_default().push_str(text);
                vec![self.emit(
                    "response.reasoning_summary_text.delta",
                    json!({
                        "output_index": output_index,
                        "item_id": item_id,
                        "summary_index": 0,
                        "delta": text,
                    }),
                )]
            }
            "signature_delta" => {
                let sig =
                    delta.get("signature").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if let Some(b) = self.blocks.get_mut(&index) {
                    b.reasoning_signature = sig;
                }
                Vec::new() // signature 不单发, 留到 output_item.done
            }
            _ => Vec::new(),
        }
    }

    fn handle_content_block_stop(&mut self, data: &Value) -> Vec<String> {
        let index = data.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        // 先把 block 所需字段拷出, 避免在调用 self.emit (mut self) 期间持有 &self.blocks 的不可变借用.
        let Some(block_ref) = self.blocks.get(&index) else {
            return Vec::new();
        };
        let output_index = block_ref.output_index;
        let item_id = block_ref.item_id.clone();
        let tool_call_id = block_ref.tool_call_id.clone();
        let tool_name = block_ref.tool_name.clone();
        let reasoning_signature = block_ref.reasoning_signature.clone();
        let kind = match block_ref.kind {
            InboundKind::Text => 0,
            InboundKind::ToolUse => 1,
            InboundKind::Thinking => 2,
        };

        let mut events: Vec<String> = Vec::new();
        let item = match kind {
            0 => {
                let text = self.text_acc.get(&index).cloned().unwrap_or_default();
                events.push(self.emit(
                    "response.output_text.done",
                    json!({
                        "output_index": output_index,
                        "item_id": item_id,
                        "content_index": 0,
                        "text": text,
                    }),
                ));
                events.push(self.emit(
                    "response.content_part.done",
                    json!({
                        "output_index": output_index,
                        "item_id": item_id,
                        "content_index": 0,
                        "part": {"type": "output_text", "text": text, "annotations": []},
                    }),
                ));
                json!({
                    "type": "message",
                    "id": item_id,
                    "role": "assistant",
                    "status": "completed",
                    "content": [{"type": "output_text", "text": text, "annotations": []}],
                })
            }
            1 => {
                let arguments = self.args_acc.get(&index).cloned().unwrap_or_default();
                events.push(self.emit(
                    "response.function_call_arguments.done",
                    json!({
                        "output_index": output_index,
                        "item_id": item_id,
                        "arguments": arguments,
                    }),
                ));
                json!({
                    "type": "function_call",
                    "id": item_id,
                    "call_id": tool_call_id,
                    "name": tool_name,
                    "arguments": arguments,
                    "status": "completed",
                })
            }
            _ => {
                let summary_text = self.thinking_acc.get(&index).cloned().unwrap_or_default();
                let (reason_id, encrypted) = decode_reasoning_signature(&reasoning_signature)
                    .unwrap_or_else(|| (item_id.clone(), String::new()));
                let summary = if summary_text.is_empty() {
                    json!([])
                } else {
                    json!([{"type": "summary_text", "text": summary_text}])
                };
                json!({
                    "type": "reasoning",
                    "id": reason_id,
                    "encrypted_content": encrypted,
                    "summary": summary,
                })
            }
        };
        events.push(self.emit(
            "response.output_item.done",
            json!({"output_index": output_index, "item": item}),
        ));
        events
    }

    fn handle_message_delta(&mut self, data: &Value) -> Vec<String> {
        if let Some(delta) = data.get("delta") {
            if let Some(sr) = delta.get("stop_reason").and_then(|v| v.as_str()) {
                self.final_stop_reason = sr.to_string();
            }
        }
        if let Some(u) = data.get("usage") {
            self.final_usage = u.clone();
        }
        Vec::new()
    }

    fn handle_message_stop(&mut self) -> Vec<String> {
        if self.completed {
            return Vec::new();
        }
        self.completed = true;
        let (status, incomplete) =
            anthropic_stop_reason_to_responses_status(&self.final_stop_reason);
        let usage = anthropic_usage_to_responses(Some(&self.final_usage));
        let response_obj = json!({
            "id": self.response_id,
            "object": "response",
            "created_at": current_unix_ts(),
            "status": status,
            "incomplete_details": incomplete,
            "model": self.response_model,
            "output": [],
            "usage": usage,
            "parallel_tool_calls": true,
        });
        vec![self.emit("response.completed", json!({"response": response_obj}))]
    }

    fn make_response_obj(&self, status: &str, incomplete: Value) -> Value {
        json!({
            "id": self.response_id,
            "object": "response",
            "created_at": current_unix_ts(),
            "status": status,
            "incomplete_details": incomplete,
            "model": self.response_model,
            "output": [],
            "usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0},
            "parallel_tool_calls": true,
        })
    }

    /// 序列化为 OpenAI Responses SSE 帧字符串.
    /// 格式: `event: <name>\ndata: {<json>...,"sequence_number":N,"type":"<name>"}\n\n`.
    fn emit(&mut self, event_name: &str, mut data: Value) -> String {
        self.seq += 1;
        if let Some(obj) = data.as_object_mut() {
            obj.insert("type".into(), json!(event_name));
            obj.insert("sequence_number".into(), json!(self.seq));
        }
        let json = serde_json::to_string(&data).unwrap_or_else(|_| "{}".into());
        format!("event: {}\ndata: {}\n\n", event_name, json)
    }
}

// ============================================================
// 非流式收集器: 把 Anthropic SSE 收齐 → 单一 OpenAI Responses JSON
// ============================================================

/// 客户端要 stream=false 但 pipeline 返回 SSE 时使用. (上游 Anthropic 透传通常会
/// 跟随客户端 stream 字段, 但某些 provider 翻译路径强制流式 → 这里兜底.)
pub struct InboundNonStreamingCollector {
    /// 复用 SSE 状态机 (内部已经维护 response_id/model/blocks 状态),
    /// 但本结构体不输出 SSE 字符串 — 而是把状态机的内部状态最终塑形成 Anthropic Message JSON.
    converter: AnthropicToResponsesSseConverter,
    /// 按 anthropic content_block index 顺序记录, 保持输出顺序.
    block_order: Vec<u32>,
}

impl Default for InboundNonStreamingCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl InboundNonStreamingCollector {
    pub fn new() -> Self {
        Self {
            converter: AnthropicToResponsesSseConverter::new(),
            block_order: Vec::new(),
        }
    }

    pub fn feed(&mut self, event_name: &str, data: &Value) {
        if event_name == "content_block_start" {
            if let Some(idx) = data.get("index").and_then(|v| v.as_u64()) {
                let idx = idx as u32;
                if !self.block_order.contains(&idx) {
                    self.block_order.push(idx);
                }
            }
        }
        // 调 feed 但丢弃 SSE 输出 — 我们只关心累积态.
        let _ = self.converter.feed(event_name, data);
    }

    pub fn finalize(mut self) -> Value {
        let _ = self.converter.finalize_if_needed();
        let mut output: Vec<Value> = Vec::new();
        for idx in &self.block_order {
            let Some(block) = self.converter.blocks.get(idx) else {
                continue;
            };
            match block.kind {
                InboundKind::Text => {
                    let text = self.converter.text_acc.get(idx).cloned().unwrap_or_default();
                    output.push(json!({
                        "type": "message",
                        "id": block.item_id,
                        "role": "assistant",
                        "status": "completed",
                        "content": [{"type": "output_text", "text": text, "annotations": []}],
                    }));
                }
                InboundKind::ToolUse => {
                    let arguments = self.converter.args_acc.get(idx).cloned().unwrap_or_default();
                    output.push(json!({
                        "type": "function_call",
                        "id": block.item_id,
                        "call_id": block.tool_call_id,
                        "name": block.tool_name,
                        "arguments": arguments,
                        "status": "completed",
                    }));
                }
                InboundKind::Thinking => {
                    let summary_text =
                        self.converter.thinking_acc.get(idx).cloned().unwrap_or_default();
                    let (reason_id, encrypted) =
                        decode_reasoning_signature(&block.reasoning_signature)
                            .unwrap_or_else(|| (block.item_id.clone(), String::new()));
                    let summary = if summary_text.is_empty() {
                        json!([])
                    } else {
                        json!([{"type": "summary_text", "text": summary_text}])
                    };
                    output.push(json!({
                        "type": "reasoning",
                        "id": reason_id,
                        "encrypted_content": encrypted,
                        "summary": summary,
                    }));
                }
            }
        }
        let (status, incomplete) =
            anthropic_stop_reason_to_responses_status(&self.converter.final_stop_reason);
        let usage = anthropic_usage_to_responses(Some(&self.converter.final_usage));
        json!({
            "id": if self.converter.response_id.is_empty() {
                format!("resp_{}", Uuid::new_v4().simple())
            } else {
                self.converter.response_id
            },
            "object": "response",
            "created_at": current_unix_ts(),
            "status": status,
            "incomplete_details": incomplete,
            "model": self.converter.response_model,
            "output": output,
            "usage": usage,
            "parallel_tool_calls": true,
        })
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- 请求侧 ----

    #[test]
    fn request_text_only() {
        let body = json!({
            "model": "gpt-5.4",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "Hello"}],
            }],
        });
        let out = request_to_anthropic(&body).unwrap();
        assert_eq!(out["model"], "gpt-5.4");
        assert_eq!(out["max_tokens"], 4096);
        assert_eq!(out["messages"][0]["role"], "user");
        assert_eq!(out["messages"][0]["content"][0]["type"], "text");
        assert_eq!(out["messages"][0]["content"][0]["text"], "Hello");
    }

    #[test]
    fn request_instructions_to_system() {
        let body = json!({
            "model": "gpt-5.4",
            "instructions": "You are concise.",
            "input": [],
        });
        let out = request_to_anthropic(&body).unwrap();
        assert_eq!(out["system"][0]["type"], "text");
        assert_eq!(out["system"][0]["text"], "You are concise.");
    }

    #[test]
    fn request_max_output_tokens_mapped() {
        let body = json!({"model":"gpt-5.4","input":[],"max_output_tokens": 8192});
        let out = request_to_anthropic(&body).unwrap();
        assert_eq!(out["max_tokens"], 8192);
    }

    #[test]
    fn request_tools_mapped() {
        let body = json!({
            "model": "gpt-5.4",
            "input": [],
            "tools": [{
                "type": "function",
                "name": "get_weather",
                "description": "Get weather",
                "parameters": {"type":"object","properties":{"city":{"type":"string"}}},
            }],
        });
        let out = request_to_anthropic(&body).unwrap();
        assert_eq!(out["tools"][0]["name"], "get_weather");
        assert_eq!(out["tools"][0]["description"], "Get weather");
        assert_eq!(out["tools"][0]["input_schema"]["type"], "object");
    }

    #[test]
    fn request_tool_choice_function() {
        let body = json!({
            "model":"gpt-5.4","input":[],
            "tool_choice": {"type":"function","name":"foo"},
        });
        let out = request_to_anthropic(&body).unwrap();
        assert_eq!(out["tool_choice"], json!({"type":"tool","name":"foo"}));
    }

    #[test]
    fn request_tool_choice_required_to_any() {
        let body = json!({"model":"gpt-5.4","input":[],"tool_choice":"required"});
        let out = request_to_anthropic(&body).unwrap();
        assert_eq!(out["tool_choice"], json!({"type":"any"}));
    }

    #[test]
    fn request_reasoning_effort_to_thinking_budget() {
        let body = json!({
            "model":"gpt-5.4","input":[],
            "reasoning": {"effort": "high"},
        });
        let out = request_to_anthropic(&body).unwrap();
        assert_eq!(out["thinking"]["type"], "enabled");
        assert_eq!(out["thinking"]["budget_tokens"], 16384);
    }

    #[test]
    fn request_multiturn_with_tool_call() {
        // user → assistant tool_use → user tool_result → assistant final
        let body = json!({
            "model": "gpt-5.4",
            "input": [
                {"type":"message","role":"user","content":[{"type":"input_text","text":"weather?"}]},
                {"type":"function_call","call_id":"c1","name":"get_weather","arguments":"{\"city\":\"NYC\"}"},
                {"type":"function_call_output","call_id":"c1","output":"sunny"},
                {"type":"message","role":"assistant","content":[{"type":"output_text","text":"It is sunny in NYC."}]},
            ],
        });
        let out = request_to_anthropic(&body).unwrap();
        let msgs = out["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 4);
        // 1: user text
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"][0]["type"], "text");
        // 2: assistant tool_use
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[1]["content"][0]["type"], "tool_use");
        assert_eq!(msgs[1]["content"][0]["name"], "get_weather");
        assert_eq!(msgs[1]["content"][0]["input"]["city"], "NYC");
        // 3: user tool_result
        assert_eq!(msgs[2]["role"], "user");
        assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
        assert_eq!(msgs[2]["content"][0]["tool_use_id"], "c1");
        assert_eq!(msgs[2]["content"][0]["content"], "sunny");
        // 4: assistant final
        assert_eq!(msgs[3]["role"], "assistant");
        assert_eq!(msgs[3]["content"][0]["text"], "It is sunny in NYC.");
    }

    #[test]
    fn request_developer_role_extracted_to_system() {
        // codex Desktop / GPT-4o+ 实战场景: input 里第一个 message 用 role=developer 传 sys prompt
        let body = json!({
            "model": "gpt-5.4",
            "input": [{
                "type": "message",
                "role": "developer",
                "content": [{"type": "input_text", "text": "You are concise."}],
            }],
        });
        let out = request_to_anthropic(&body).unwrap();
        assert_eq!(out["system"][0]["text"], "You are concise.");
        // messages 不应该再包含 developer role - 否则上游严格 provider (xiaomi/deepseek) 400
        assert!(
            out["messages"].as_array().map(|a| a.is_empty()).unwrap_or(true),
            "developer role 不应进 messages 数组"
        );
    }

    #[test]
    fn request_system_role_extracted_to_system() {
        let body = json!({
            "model": "gpt-5.4",
            "input": [{
                "type": "message",
                "role": "system",
                "content": [{"type": "input_text", "text": "sys text"}],
            }],
        });
        let out = request_to_anthropic(&body).unwrap();
        assert_eq!(out["system"][0]["text"], "sys text");
    }

    #[test]
    fn request_instructions_and_developer_role_merged() {
        let body = json!({
            "model": "gpt-5.4",
            "instructions": "top-level",
            "input": [
                {"type":"message","role":"developer","content":[{"type":"input_text","text":"dev"}]},
                {"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]},
            ],
        });
        let out = request_to_anthropic(&body).unwrap();
        // 顺序: instructions 在前, input 里 developer role 内容追加在后
        assert_eq!(out["system"][0]["text"], "top-level\n\ndev");
        assert_eq!(out["messages"][0]["role"], "user");
        assert_eq!(out["messages"][0]["content"][0]["text"], "hi");
    }

    #[test]
    fn request_mixed_roles_preserves_user_assistant_order() {
        // 真实多轮会话: user 提问 → developer 插指令 → assistant 回 → user 追问
        // developer 要被抽到 system, 其他保留 messages 原序
        let body = json!({
            "model": "gpt-5.4",
            "input": [
                {"type":"message","role":"user","content":[{"type":"input_text","text":"q1"}]},
                {"type":"message","role":"developer","content":[{"type":"input_text","text":"sys"}]},
                {"type":"message","role":"assistant","content":[{"type":"output_text","text":"a1"}]},
                {"type":"message","role":"user","content":[{"type":"input_text","text":"q2"}]},
            ],
        });
        let out = request_to_anthropic(&body).unwrap();
        assert_eq!(out["system"][0]["text"], "sys");
        let msgs = out["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"][0]["text"], "q1");
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[2]["role"], "user");
        assert_eq!(msgs[2]["content"][0]["text"], "q2");
    }

    #[test]
    fn request_reasoning_roundtrip() {
        // 客户端回灌一个上次拿到的 reasoning item (含 encrypted_content), 应翻成 Anthropic thinking block
        let body = json!({
            "model":"gpt-5.4",
            "input":[{
                "type":"reasoning",
                "id":"rs_abc",
                "encrypted_content":"ENC_XXX",
                "summary":[{"type":"summary_text","text":"my thoughts"}],
            }],
        });
        let out = request_to_anthropic(&body).unwrap();
        let msg = &out["messages"][0];
        assert_eq!(msg["role"], "assistant");
        let block = &msg["content"][0];
        assert_eq!(block["type"], "thinking");
        assert_eq!(block["thinking"], "my thoughts");
        // signature 解码后应恢复原始 id + encrypted_content
        let sig = block["signature"].as_str().unwrap();
        let (id, ec) = decode_reasoning_signature(sig).unwrap();
        assert_eq!(id, "rs_abc");
        assert_eq!(ec, "ENC_XXX");
    }

    // ---- 响应 JSON 侧 ----

    #[test]
    fn response_json_text_only() {
        let msg = json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-6",
            "content": [{"type":"text","text":"Hi there."}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5},
        });
        let out = response_to_responses_json(&msg);
        assert_eq!(out["id"], "msg_123");
        assert_eq!(out["object"], "response");
        assert_eq!(out["status"], "completed");
        assert_eq!(out["model"], "claude-sonnet-4-6");
        let item = &out["output"][0];
        assert_eq!(item["type"], "message");
        assert_eq!(item["role"], "assistant");
        assert_eq!(item["content"][0]["type"], "output_text");
        assert_eq!(item["content"][0]["text"], "Hi there.");
        assert_eq!(out["usage"]["input_tokens"], 10);
        assert_eq!(out["usage"]["output_tokens"], 5);
        assert_eq!(out["usage"]["total_tokens"], 15);
    }

    #[test]
    fn response_json_tool_use() {
        let msg = json!({
            "id":"msg_1","type":"message","role":"assistant","model":"claude-sonnet-4-6",
            "content":[
                {"type":"text","text":"Calling tool..."},
                {"type":"tool_use","id":"call_1","name":"get_weather","input":{"city":"NYC"}},
            ],
            "stop_reason":"tool_use",
            "usage":{"input_tokens":1,"output_tokens":2},
        });
        let out = response_to_responses_json(&msg);
        assert_eq!(out["status"], "completed");
        assert_eq!(out["output"].as_array().unwrap().len(), 2);
        assert_eq!(out["output"][0]["type"], "message");
        assert_eq!(out["output"][1]["type"], "function_call");
        assert_eq!(out["output"][1]["call_id"], "call_1");
        assert_eq!(out["output"][1]["name"], "get_weather");
        // arguments 是 JSON 字符串
        let args: Value = serde_json::from_str(out["output"][1]["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(args["city"], "NYC");
    }

    #[test]
    fn response_json_with_thinking() {
        let sig = encode_reasoning_signature("rs_abc", "ENC_XXX");
        let msg = json!({
            "id":"msg_1","type":"message","role":"assistant","model":"claude-sonnet-4-6",
            "content":[
                {"type":"thinking","thinking":"reasoning here","signature": sig},
                {"type":"text","text":"answer"},
            ],
            "stop_reason":"end_turn",
            "usage":{"input_tokens":1,"output_tokens":2},
        });
        let out = response_to_responses_json(&msg);
        let items = out["output"].as_array().unwrap();
        assert_eq!(items[0]["type"], "reasoning");
        assert_eq!(items[0]["id"], "rs_abc");
        assert_eq!(items[0]["encrypted_content"], "ENC_XXX");
        assert_eq!(items[0]["summary"][0]["text"], "reasoning here");
        assert_eq!(items[1]["type"], "message");
        assert_eq!(items[1]["content"][0]["text"], "answer");
    }

    #[test]
    fn response_json_stop_reason_max_tokens() {
        let msg = json!({
            "id":"msg_1","type":"message","role":"assistant","model":"claude",
            "content":[{"type":"text","text":"truncated"}],
            "stop_reason":"max_tokens",
            "usage":{"input_tokens":1,"output_tokens":2},
        });
        let out = response_to_responses_json(&msg);
        assert_eq!(out["status"], "incomplete");
        assert_eq!(out["incomplete_details"]["reason"], "max_output_tokens");
    }

    // ---- SSE 状态机 ----

    fn parse_emitted(frame: &str) -> (String, Value) {
        // "event: <name>\ndata: <json>\n\n" → (name, json)
        let mut lines = frame.lines();
        let event_line = lines.next().unwrap();
        let data_line = lines.next().unwrap();
        let name = event_line.trim_start_matches("event: ").to_string();
        let json: Value = serde_json::from_str(data_line.trim_start_matches("data: ")).unwrap();
        (name, json)
    }

    #[test]
    fn sse_text_streaming_basic() {
        let mut conv = AnthropicToResponsesSseConverter::new();
        let mut frames: Vec<String> = Vec::new();
        frames.extend(conv.feed(
            "message_start",
            &json!({"type":"message_start","message":{"id":"msg_1","model":"claude-sonnet-4-6"}}),
        ));
        frames.extend(conv.feed(
            "content_block_start",
            &json!({"index":0,"content_block":{"type":"text","text":""}}),
        ));
        frames.extend(conv.feed(
            "content_block_delta",
            &json!({"index":0,"delta":{"type":"text_delta","text":"Hello"}}),
        ));
        frames.extend(conv.feed(
            "content_block_delta",
            &json!({"index":0,"delta":{"type":"text_delta","text":" world"}}),
        ));
        frames.extend(conv.feed("content_block_stop", &json!({"index":0})));
        frames.extend(conv.feed(
            "message_delta",
            &json!({"delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":5,"output_tokens":2}}),
        ));
        frames.extend(conv.feed("message_stop", &json!({"type":"message_stop"})));

        let names: Vec<String> = frames.iter().map(|f| parse_emitted(f).0).collect();
        assert!(names.contains(&"response.created".to_string()));
        assert!(names.contains(&"response.in_progress".to_string()));
        assert!(names.contains(&"response.output_item.added".to_string()));
        assert!(names.contains(&"response.content_part.added".to_string()));
        let delta_count = names.iter().filter(|n| n == &"response.output_text.delta").count();
        assert_eq!(delta_count, 2);
        assert!(names.contains(&"response.output_text.done".to_string()));
        assert!(names.contains(&"response.content_part.done".to_string()));
        assert!(names.contains(&"response.output_item.done".to_string()));
        assert!(names.contains(&"response.completed".to_string()));

        // 校验 completed 帧里 status / usage 正确
        let completed = frames.iter().find(|f| f.starts_with("event: response.completed")).unwrap();
        let (_, body) = parse_emitted(completed);
        assert_eq!(body["response"]["status"], "completed");
        assert_eq!(body["response"]["usage"]["input_tokens"], 5);
        assert_eq!(body["response"]["usage"]["output_tokens"], 2);
        assert_eq!(body["response"]["usage"]["total_tokens"], 7);
    }

    #[test]
    fn sse_tool_use_streaming() {
        let mut conv = AnthropicToResponsesSseConverter::new();
        let mut frames: Vec<String> = Vec::new();
        frames.extend(conv.feed(
            "message_start",
            &json!({"type":"message_start","message":{"id":"msg_1","model":"claude"}}),
        ));
        frames.extend(conv.feed(
            "content_block_start",
            &json!({"index":0,"content_block":{"type":"tool_use","id":"call_1","name":"get_weather","input":{}}}),
        ));
        frames.extend(conv.feed(
            "content_block_delta",
            &json!({"index":0,"delta":{"type":"input_json_delta","partial_json":"{\"city\":"}}),
        ));
        frames.extend(conv.feed(
            "content_block_delta",
            &json!({"index":0,"delta":{"type":"input_json_delta","partial_json":"\"NYC\"}"}}),
        ));
        frames.extend(conv.feed("content_block_stop", &json!({"index":0})));
        frames.extend(conv.feed(
            "message_delta",
            &json!({"delta":{"stop_reason":"tool_use"},"usage":{"input_tokens":1,"output_tokens":2}}),
        ));
        frames.extend(conv.feed("message_stop", &json!({})));

        let added = frames
            .iter()
            .find(|f| f.starts_with("event: response.output_item.added"))
            .unwrap();
        let (_, body) = parse_emitted(added);
        assert_eq!(body["item"]["type"], "function_call");
        assert_eq!(body["item"]["call_id"], "call_1");
        assert_eq!(body["item"]["name"], "get_weather");

        let args_done = frames
            .iter()
            .find(|f| f.starts_with("event: response.function_call_arguments.done"))
            .unwrap();
        let (_, body) = parse_emitted(args_done);
        assert_eq!(body["arguments"], "{\"city\":\"NYC\"}");
    }

    #[test]
    fn sse_thinking_with_signature() {
        let sig = encode_reasoning_signature("rs_xyz", "ENC_DATA");
        let mut conv = AnthropicToResponsesSseConverter::new();
        let _ = conv.feed(
            "message_start",
            &json!({"type":"message_start","message":{"id":"msg_1","model":"claude"}}),
        );
        let _ = conv.feed(
            "content_block_start",
            &json!({"index":0,"content_block":{"type":"thinking","thinking":""}}),
        );
        let _ = conv.feed(
            "content_block_delta",
            &json!({"index":0,"delta":{"type":"thinking_delta","thinking":"step1"}}),
        );
        let _ = conv.feed(
            "content_block_delta",
            &json!({"index":0,"delta":{"type":"signature_delta","signature": sig}}),
        );
        let stop_frames = conv.feed("content_block_stop", &json!({"index":0}));

        let done = stop_frames
            .iter()
            .find(|f| f.starts_with("event: response.output_item.done"))
            .unwrap();
        let (_, body) = parse_emitted(done);
        assert_eq!(body["item"]["type"], "reasoning");
        assert_eq!(body["item"]["id"], "rs_xyz");
        assert_eq!(body["item"]["encrypted_content"], "ENC_DATA");
        assert_eq!(body["item"]["summary"][0]["text"], "step1");
    }

    #[test]
    fn sse_sequence_numbers_monotonic() {
        let mut conv = AnthropicToResponsesSseConverter::new();
        let mut frames: Vec<String> = Vec::new();
        frames.extend(conv.feed(
            "message_start",
            &json!({"message":{"id":"msg_1","model":"claude"}}),
        ));
        frames.extend(conv.feed(
            "content_block_start",
            &json!({"index":0,"content_block":{"type":"text","text":""}}),
        ));
        frames.extend(conv.feed(
            "content_block_delta",
            &json!({"index":0,"delta":{"type":"text_delta","text":"a"}}),
        ));
        let seqs: Vec<u64> = frames
            .iter()
            .map(|f| parse_emitted(f).1.get("sequence_number").unwrap().as_u64().unwrap())
            .collect();
        for w in seqs.windows(2) {
            assert!(w[1] > w[0], "sequence_number must strictly increase: {:?}", seqs);
        }
    }

    #[test]
    fn sse_finalize_if_needed_emits_completed() {
        let mut conv = AnthropicToResponsesSseConverter::new();
        let _ = conv.feed(
            "message_start",
            &json!({"message":{"id":"msg_1","model":"claude"}}),
        );
        let extra = conv.finalize_if_needed();
        assert!(extra.iter().any(|f| f.starts_with("event: response.completed")));
    }
}
