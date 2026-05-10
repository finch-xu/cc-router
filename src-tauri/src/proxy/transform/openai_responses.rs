//! OpenAI Responses (ChatGPT 反代) ↔ Anthropic Messages 协议翻译.
//!
//! 协议事实 (Phase 0 实测确认, 详见 plan.md "协议事实" 段):
//! - 强制 `stream: true`, 非流式直接 400
//! - 禁 `max_output_tokens`, 即便用户传也得 strip
//! - 强制 `store: false` 与 `include: ["reasoning.encrypted_content"]`
//! - 强制 `instructions` 字段 present (即使空字符串), 否则 400 "Instructions are required"
//! - User-Agent 必须是 `codex-cli`, 还要带 `ChatGPT-Account-Id` header
//!
//! ## 请求侧 (anthropic_to_responses)
//!
//! Anthropic Messages → OpenAI Responses 的字段映射:
//!
//! | Anthropic 字段        | OpenAI Responses 字段        |
//! |---------------------- |-----------------------------|
//! | `system` (str / arr)  | `instructions` (str)         |
//! | `messages[]`          | `input[]` (扁平化, tool_use/tool_result 提升为顶层 item) |
//! | `temperature/top_p`   | 同名透传                      |
//! | `tools[]`             | `tools[]` (type=function, schema 字段重命名) |
//! | `tool_choice`         | `tool_choice` (字段名映射)   |
//! | `max_tokens`          | **DROP** (后端拒绝)         |
//! | `stop_sequences`      | DROP (后端不支持)           |
//!
//! 强制注入: `store=false`, `include=["reasoning.encrypted_content"]`, `stream=true`.
//!
//! ## 响应侧 (ResponsesSseConverter)
//!
//! 每个上游 SSE 事件触发一次状态更新, 可能 emit 0 或多个 Anthropic SSE 事件:
//!
//! | OpenAI Responses 事件                       | Anthropic 事件                         |
//! |--------------------------------------------|---------------------------------------|
//! | `response.created`                         | `message_start`                       |
//! | `response.in_progress`                     | (skip)                                |
//! | `response.output_item.added` (message)     | `content_block_start` (text)          |
//! | `response.output_item.added` (function_call) | `content_block_start` (tool_use)    |
//! | `response.output_item.added` (reasoning)   | (skip, Phase 1 不暴露 reasoning)     |
//! | `response.content_part.added/done`         | (skip, output_item 已 cover)          |
//! | `response.output_text.delta`               | `content_block_delta` (text_delta)    |
//! | `response.output_text.done`                | (skip, output_item.done 来 cover)    |
//! | `response.function_call_arguments.delta`   | `content_block_delta` (input_json_delta) |
//! | `response.function_call_arguments.done`    | (skip, output_item.done 来 cover)    |
//! | `response.output_item.done`                | `content_block_stop`                  |
//! | `response.completed`                       | `message_delta` (stop_reason+usage) + `message_stop` |

use std::collections::HashMap;

use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

// ============================================================
// 请求转换
// ============================================================

/// 把 Anthropic Messages 请求体转成 OpenAI Responses 请求体.
///
/// `model_override`: cc-router pipeline 已经在外面把 body["model"] 改成 slot 真实模型名,
/// 这里直接读用. 但因为 ChatGPT 后端只认特定 Codex 模型 (例如 `gpt-5.5`),
/// 用户在 yaml example_models 里填的就要是这种, 否则 400.
pub fn anthropic_to_responses(body: &Value) -> AppResult<Value> {
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::BadRequest("请求 body 缺少 model".into()))?;

    let mut out = json!({
        "model": model,
        // ChatGPT 后端强制约束 (Phase 0 实测)
        "stream": true,
        "store": false,
        "include": ["reasoning.encrypted_content"],
    });

    // system → instructions
    // Codex 后端 schema 强制要求 instructions 字段 present, 缺失会 400 "Instructions are required".
    // Anthropic 协议下 system 是可选的, 所以这里补默认空字符串.
    let instructions_text = body
        .get("system")
        .map(anthropic_system_to_text)
        .unwrap_or_default();
    out["instructions"] = json!(instructions_text);

    // messages[] → input[]
    if let Some(msgs) = body.get("messages").and_then(|m| m.as_array()) {
        let input = anthropic_messages_to_input(msgs)?;
        out["input"] = json!(input);
    }

    // tools[] → tools[]
    if let Some(tools) = body.get("tools").and_then(|t| t.as_array()) {
        let converted: Vec<Value> = tools
            .iter()
            .filter_map(convert_tool)
            .collect();
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

    // **不要** 拷 max_tokens / stop_sequences (后端会 400)

    Ok(out)
}

fn anthropic_system_to_text(system: &Value) -> String {
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
fn anthropic_messages_to_input(msgs: &[Value]) -> AppResult<Vec<Value>> {
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
                            // 先 flush 文本
                            if !text_parts.is_empty() {
                                out.push(json!({
                                    "type": "message",
                                    "role": role,
                                    "content": text_parts,
                                }));
                                text_parts = Vec::new();
                            }
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
                            if !text_parts.is_empty() {
                                out.push(json!({
                                    "type": "message",
                                    "role": role,
                                    "content": text_parts,
                                }));
                                text_parts = Vec::new();
                            }
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
                        // image, document 等 Phase 1 暂不支持: 直接忽略, 后续补.
                        _ => {}
                    }
                }
                // 末尾还残留的文本
                if !text_parts.is_empty() {
                    out.push(json!({
                        "type": "message",
                        "role": role,
                        "content": text_parts,
                    }));
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

fn make_message_item(role: &str, text: &str) -> Value {
    json!({
        "type": "message",
        "role": role,
        "content": [{ "type": text_block_type(role), "text": text }],
    })
}

/// OpenAI Responses 里区分 input_text (用户/工具结果) 和 output_text (助手回复).
fn text_block_type(role: &str) -> &'static str {
    if role == "assistant" {
        "output_text"
    } else {
        "input_text"
    }
}

fn convert_tool(t: &Value) -> Option<Value> {
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

fn map_tool_choice(tc: &Value) -> Option<Value> {
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

/// 上游 SSE 事件的解码视图. 我们只关心 type 和少量字段,
/// 用 untagged enum + serde 直接 try_into 较烦, 走手动 match.
#[derive(Debug)]
enum UpstreamEvent {
    /// response 整体生命周期事件
    Created(Value),
    Completed(Value),
    /// 单个 output item 边界 + 内容
    OutputItemAdded {
        output_index: u32,
        item: Value,
    },
    OutputItemDone {
        output_index: u32,
        item: Value,
    },
    /// 文本 delta
    OutputTextDelta {
        output_index: u32,
        delta: String,
    },
    /// tool 参数 delta
    FunctionCallArgsDelta {
        output_index: u32,
        delta: String,
    },
    /// 不关心或暂时忽略的事件 (in_progress / content_part.added / output_text.done 等)
    Ignored,
    /// 解析失败但不 fatal
    Unknown,
}

fn parse_upstream_event(event_name: &str, data: &Value) -> UpstreamEvent {
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
        "response.in_progress"
        | "response.content_part.added"
        | "response.content_part.done"
        | "response.output_text.done"
        | "response.function_call_arguments.done" => UpstreamEvent::Ignored,
        _ => UpstreamEvent::Unknown,
    }
}

/// Anthropic SSE 事件 (输出端). 序列化时按 `event: <type>\ndata: <json>\n\n` 写出.
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
    ContentBlockDelta {
        index: u32,
        delta: Value,
    },
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

    /// 序列化为完整 SSE 帧文本 (包含 trailing \n\n).
    pub fn to_sse_frame(&self) -> String {
        let json = serde_json::to_string(self).unwrap_or_else(|_| "{}".into());
        format!("event: {}\ndata: {}\n\n", self.event_name(), json)
    }
}

/// 单个内容块 (Anthropic 视角) 的状态.
#[derive(Debug, Clone)]
struct ContentBlock {
    /// "text" | "tool_use"
    kind: BlockKind,
    /// Anthropic 侧 content_blocks 索引 (从 0 开始, 与 OpenAI output_index 不同——我们跳过了 reasoning items)
    anthropic_index: u32,
}

#[derive(Debug, Clone)]
enum BlockKind {
    Text,
    ToolUse,
    /// reasoning / 暂不支持的类型, 跳过
    Skipped,
}

/// SSE 状态机. pipeline 在每次拿到 chunk 时, 自己按 `\n\n` 切帧, 解 event_name + data,
/// 然后调用 `feed_event` 拿一组 (可能为 0) 待写出的 Anthropic 帧.
pub struct ResponsesSseConverter {
    /// 是否已 emit 过 message_start
    started: bool,
    /// 真正暴露给 Claude Code 的 message id (Anthropic 习惯 msg_xxx, 我们沿用上游 response.id)
    message_id: String,
    /// 真实模型名 (从 response.created 拿)
    response_model: String,
    /// 下一个 Anthropic content block 用的索引 (顺序分配)
    next_anthropic_index: u32,
    /// output_index → 该块的 Anthropic 状态
    blocks: HashMap<u32, ContentBlock>,
    /// 累积 usage, 由 response.completed 决定最终值
    final_usage: Option<Value>,
    /// 是否已 emit message_stop, 防重复
    stopped: bool,
}

impl Default for ResponsesSseConverter {
    fn default() -> Self {
        Self::new()
    }
}

impl ResponsesSseConverter {
    pub fn new() -> Self {
        Self {
            started: false,
            message_id: String::new(),
            response_model: String::new(),
            next_anthropic_index: 0,
            blocks: HashMap::new(),
            final_usage: None,
            stopped: false,
        }
    }

    /// 喂一个上游 SSE 事件 (event_name + data JSON), 返回需要发给 client 的 Anthropic 帧列表.
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
            UpstreamEvent::Completed(resp) => self.handle_completed(&resp),
            UpstreamEvent::Ignored | UpstreamEvent::Unknown => Vec::new(),
        }
    }

    /// 流结束时调一次 (不依赖 response.completed): 兜底 emit message_stop.
    pub fn finalize_if_needed(&mut self) -> Vec<AnthropicEvent> {
        if !self.stopped && self.started {
            self.stopped = true;
            return vec![AnthropicEvent::MessageStop];
        }
        Vec::new()
    }

    /// 响应模型名 (从 response.created 拿). 流首事件之前为空串.
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
            .unwrap_or_else(|| "msg_unknown")
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
                // 暂不立即 emit content_block_start, 等到第一个 output_text.delta 再发,
                // 因为 OpenAI 的 message 容器内可能有多个 content_part. 但是为了保持顺序,
                // 实际上 content_part.added 是「output_text 容器」的开始, 我们提前发以便文本立即跟上.
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
                    .unwrap_or_else(|| "call_unknown");
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
            // reasoning / 其他不暴露
            _ => {
                self.blocks.insert(
                    output_index,
                    ContentBlock { kind: BlockKind::Skipped, anthropic_index: 0 },
                );
                Vec::new()
            }
        }
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
        _item: &Value,
    ) -> Vec<AnthropicEvent> {
        let Some(block) = self.blocks.get(&output_index) else {
            return Vec::new();
        };
        if matches!(block.kind, BlockKind::Skipped) {
            return Vec::new();
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

        // usage: OpenAI 给的是 {input_tokens, output_tokens, ...}, 字段名居然和 Anthropic 一致.
        let usage = response
            .and_then(|r| r.get("usage"))
            .cloned()
            .unwrap_or_else(|| json!({"input_tokens": 0, "output_tokens": 0}));
        self.final_usage = Some(usage.clone());

        let mut out = Vec::new();
        out.push(AnthropicEvent::MessageDelta {
            delta: json!({"stop_reason": stop_reason, "stop_sequence": null}),
            usage,
        });
        out.push(AnthropicEvent::MessageStop);
        out
    }
}

fn map_status_to_anthropic_stop_reason(status: &str) -> &'static str {
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

/// 给非流式 Claude Code 请求用. 上游强制 SSE, 我们这边累积 deltas, 拼成最终的 message JSON.
/// 实现思路: 复用 `ResponsesSseConverter` 喂事件, 同时跟踪每个 content_block 的累积内容,
/// 流结束时拼出 Anthropic message.
pub struct NonStreamingCollector {
    converter: ResponsesSseConverter,
    /// anthropic_index → 累积的 text
    text_acc: HashMap<u32, String>,
    /// anthropic_index → 累积的 tool input JSON 字符串
    tool_args_acc: HashMap<u32, String>,
    /// anthropic_index → ContentBlockStart 时的元信息 (id, name, type)
    block_meta: HashMap<u32, Value>,
    /// 顺序记录 anthropic_index 出现顺序
    order: Vec<u32>,
}

impl Default for NonStreamingCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl NonStreamingCollector {
    pub fn new() -> Self {
        Self {
            converter: ResponsesSseConverter::new(),
            text_acc: HashMap::new(),
            tool_args_acc: HashMap::new(),
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
                    _ => None,
                }
            })
            .collect();

        // 来自 message_delta 的 usage; 拿不到就 0
        let usage = self.converter.final_usage.clone().unwrap_or_else(|| {
            json!({"input_tokens": 0, "output_tokens": 0})
        });
        let stop_reason = "end_turn"; // converter 已经在 message_delta 里发过, 这里给最终 message 一个保守值

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
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// 解 SSE 帧成 (event_name, data_json). 给调用方反复调.
/// 解析失败返回 None, 让调用方继续读下一帧 (与现有 sse.rs 「失败 warn 不 fatal」一致).
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
// 测试
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
