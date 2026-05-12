//! Anthropic Messages ↔ AWS CodeWhisperer (Kiro IDE 后端) 协议翻译.
//!
//! ## 协议事实
//!
//! - 请求体: JSON, 含 `profileArn` + `conversationState`
//! - 响应: AWS Event Stream 二进制流 (见 [`super::aws_event_stream`] 解码)
//! - 模型 ID 必须用 Kiro 专属名 (`claude-sonnet-4.5` 等), 而非 `claude-sonnet-4-20250514` 这种 API 全名
//!
//! ## 请求侧字段映射
//!
//! ```text
//! Anthropic Body                  CodeWhisperer
//! ─────────────────────           ─────────────────────────────────────
//! model                        →  conversationState.currentMessage
//!                                  .userInputMessage.modelId
//! system (str / arr)           →  最后一条 user 消息前添加为 system context
//!                                  (CodeWhisperer 无独立 system 字段)
//! messages[*]                  →  前 N-1 条 → conversationState.history[]
//!                                  最后一条 → currentMessage.userInputMessage.content
//! tools[]                      →  currentMessage...userInputMessageContext.tools[]
//!                                  (toolSpecification 包一层)
//! cache_control                →  silent drop (CodeWhisperer 无对应字段)
//! ```
//!
//! ## 响应侧事件分发 (按 :event-type header)
//!
//! | :event-type                  | Payload                          | Anthropic SSE                                  |
//! |------------------------------|----------------------------------|-----------------------------------------------|
//! | `assistantResponseEvent`     | `{"content": "..."}`             | `content_block_delta` (text_delta)            |
//! | `toolUseEvent`               | `{"toolUseId","name","input","stop"}` | `content_block_start[tool_use]` + `input_json_delta` + `content_block_stop` |
//! | `messageMetadataEvent`       | `{"conversationId","utteranceId"}` | (skip)                                        |
//! | `codeReferenceEvent`         | (引用代码源)                     | (skip)                                        |
//! | `followupPromptEvent`        | (跟进提示)                       | (skip)                                        |
//! | `:message-type=exception`    | `{"message":"..."}`              | error event                                    |
//! | 未知 event-type              | -                                | warn 日志, 不 emit, 不 panic                  |
//!
//! 流开始时 emit `message_start`, 流结束时 emit `message_delta` (stop_reason) + `message_stop`.

use std::collections::HashMap;

use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::proxy::transform::aws_event_stream::EventStreamFrame;

// ============================================================
// 请求转换
// ============================================================

/// 把 Anthropic Messages 请求体转成 CodeWhisperer `generateAssistantResponse` JSON.
///
/// `profile_arn`: 来自订阅的 oauth_metadata.profile_arn. None 时不注入 (Builder ID 用户).
/// `conversation_id`: 若需保持多轮 conversation, 调用方传入持久化的 uuid; 否则随机.
pub fn anthropic_to_codewhisperer(
    body: &Value,
    profile_arn: Option<&str>,
    conversation_id: Option<&str>,
) -> AppResult<Value> {
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::BadRequest("请求 body 缺少 model".into()))?;
    let kiro_model = normalize_to_kiro_model(model);

    let system_text = body
        .get("system")
        .map(anthropic_system_to_text)
        .unwrap_or_default();

    let messages_arr = body
        .get("messages")
        .and_then(|m| m.as_array())
        .ok_or_else(|| AppError::BadRequest("请求 body 缺少 messages".into()))?;

    if messages_arr.is_empty() {
        return Err(AppError::BadRequest("messages 不能为空".into()));
    }

    // 把 messages 拆成 history (前 N-1) + currentMessage (最后一条).
    // 注意 Anthropic 协议下最后一条通常是 user, 否则 CodeWhisperer 后端会 reject.
    let last_idx = messages_arr.len() - 1;
    let history_msgs = &messages_arr[..last_idx];
    let current_msg = &messages_arr[last_idx];

    let history = build_history(history_msgs)?;
    let current_user_input = build_current_user_input(
        current_msg,
        &system_text,
        &kiro_model,
        body.get("tools").and_then(|t| t.as_array()),
    )?;

    let conversation_id =
        conversation_id.map(str::to_string).unwrap_or_else(|| Uuid::new_v4().to_string());

    let conv_state = json!({
        "chatTriggerType": "MANUAL",
        "conversationId": conversation_id,
        "currentMessage": {
            "userInputMessage": current_user_input,
        },
        "history": history,
    });

    let mut out = json!({ "conversationState": conv_state });
    if let Some(arn) = profile_arn {
        out["profileArn"] = json!(arn);
    }
    Ok(out)
}

fn anthropic_system_to_text(system: &Value) -> String {
    if let Some(s) = system.as_str() {
        return s.to_string();
    }
    if let Some(arr) = system.as_array() {
        return arr
            .iter()
            .filter_map(|item| item.get("text").and_then(|t| t.as_str()).map(str::to_string))
            .collect::<Vec<_>>()
            .join("\n\n");
    }
    String::new()
}

/// history[] 元素结构 (按上游协议推测):
/// ```json
/// { "userInputMessage": {"content": "...", "modelId": "..."} }
/// { "assistantResponseMessage": {"content": "...", "toolUses": [...]} }
/// ```
fn build_history(msgs: &[Value]) -> AppResult<Vec<Value>> {
    let mut out = Vec::with_capacity(msgs.len());
    for m in msgs {
        let role = m.get("role").and_then(|v| v.as_str()).unwrap_or("user");
        let (text, tool_uses, tool_results) = split_content_blocks(m.get("content"));

        match role {
            "user" => {
                // user 消息: content + 可能的 tool_results (上一轮 assistant 调工具的反馈)
                let mut user_msg = json!({ "content": text });
                if !tool_results.is_empty() {
                    user_msg["userInputMessageContext"] = json!({
                        "toolResults": tool_results,
                    });
                }
                out.push(json!({ "userInputMessage": user_msg }));
            }
            "assistant" => {
                let mut asst = json!({ "content": text });
                if !tool_uses.is_empty() {
                    asst["toolUses"] = json!(tool_uses);
                }
                out.push(json!({ "assistantResponseMessage": asst }));
            }
            _ => {}
        }
    }
    Ok(out)
}

/// 拆 Anthropic content blocks 为 (合并文本, tool_use 列表, tool_result 列表).
/// thinking 块的文本会被 silently drop (CodeWhisperer 接收方不需要).
fn split_content_blocks(content: Option<&Value>) -> (String, Vec<Value>, Vec<Value>) {
    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_uses: Vec<Value> = Vec::new();
    let mut tool_results: Vec<Value> = Vec::new();

    match content {
        Some(Value::String(s)) => text_parts.push(s.clone()),
        Some(Value::Array(blocks)) => {
            for blk in blocks {
                let ty = blk.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match ty {
                    "text" => {
                        if let Some(t) = blk.get("text").and_then(|v| v.as_str()) {
                            text_parts.push(t.to_string());
                        }
                    }
                    "thinking" | "redacted_thinking" => {
                        // CodeWhisperer 不接受 thinking 块, silent drop.
                    }
                    "tool_use" => {
                        let id = blk.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        let name = blk.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        let input = blk.get("input").cloned().unwrap_or_else(|| json!({}));
                        tool_uses.push(json!({
                            "toolUseId": id,
                            "name": name,
                            "input": input,
                        }));
                    }
                    "tool_result" => {
                        let id = blk
                            .get("tool_use_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let tr_content = match blk.get("content") {
                            Some(Value::String(s)) => json!([{"text": s}]),
                            Some(Value::Array(arr)) => {
                                let items: Vec<Value> = arr
                                    .iter()
                                    .filter_map(|b| {
                                        b.get("text")
                                            .and_then(|t| t.as_str())
                                            .map(|t| json!({"text": t}))
                                    })
                                    .collect();
                                json!(items)
                            }
                            _ => json!([]),
                        };
                        let status = if blk
                            .get("is_error")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false)
                        {
                            "error"
                        } else {
                            "success"
                        };
                        tool_results.push(json!({
                            "toolUseId": id,
                            "content": tr_content,
                            "status": status,
                        }));
                    }
                    "image" => {
                        // CodeWhisperer 是否支持 image 输入未文档化, 暂以 media_type 占位文本兜底.
                        if let Some(src) = blk.get("source") {
                            if let Some(media_type) =
                                src.get("media_type").and_then(|v| v.as_str())
                            {
                                text_parts.push(format!("[image: {}]", media_type));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }

    (text_parts.join("\n"), tool_uses, tool_results)
}

fn build_current_user_input(
    current_msg: &Value,
    system_text: &str,
    kiro_model: &str,
    tools: Option<&Vec<Value>>,
) -> AppResult<Value> {
    let role = current_msg
        .get("role")
        .and_then(|v| v.as_str())
        .unwrap_or("user");
    if role != "user" {
        return Err(AppError::BadRequest(
            "Kiro 协议要求 messages 最后一条必须是 user".into(),
        ));
    }
    let (text, _tool_uses, tool_results) = split_content_blocks(current_msg.get("content"));

    // 把 system 拼到 content 前 (CodeWhisperer 无独立 system 字段). 留空行分隔.
    let content_with_system = if system_text.is_empty() {
        text
    } else {
        format!("{}\n\n{}", system_text, text)
    };

    let mut out = json!({
        "content": content_with_system,
        "modelId": kiro_model,
        "origin": "AI_EDITOR",
    });

    let mut ctx = json!({});
    let mut ctx_has_field = false;

    if let Some(tools_arr) = tools {
        let kiro_tools: Vec<Value> = tools_arr
            .iter()
            .filter_map(|t| {
                let name = t.get("name").and_then(|v| v.as_str())?;
                let desc = t.get("description").and_then(|v| v.as_str()).unwrap_or("");
                let schema = t.get("input_schema").cloned().unwrap_or_else(|| json!({}));
                Some(json!({
                    "toolSpecification": {
                        "name": name,
                        "description": desc,
                        "inputSchema": { "json": schema },
                    }
                }))
            })
            .collect();
        if !kiro_tools.is_empty() {
            ctx["tools"] = json!(kiro_tools);
            ctx_has_field = true;
        }
    }
    if !tool_results.is_empty() {
        ctx["toolResults"] = json!(tool_results);
        ctx_has_field = true;
    }
    if ctx_has_field {
        out["userInputMessageContext"] = ctx;
    }

    Ok(out)
}

/// 把用户在 slot 里填的全模型名(`claude-sonnet-4-5-20250929`)归一为 Kiro 短名(`claude-sonnet-4.5`).
/// 已经是短名则直接返回.
pub fn normalize_to_kiro_model(model: &str) -> String {
    let lower = model.to_ascii_lowercase();
    if lower.contains("opus") {
        // 4-5 / 4.5 → 4.5; 其他 opus → 4.5 兜底 (Kiro 免费层 2026-01 已下架 opus, 留兜底)
        return "claude-opus-4.5".to_string();
    }
    if lower.contains("haiku") {
        return "claude-haiku-4.5".to_string();
    }
    if lower.contains("3-7-sonnet") || lower.contains("3.7-sonnet") {
        return "claude-3-7-sonnet".to_string();
    }
    if lower.contains("sonnet-4-5") || lower.contains("sonnet-4.5") {
        return "claude-sonnet-4.5".to_string();
    }
    if lower.contains("sonnet-4") || lower.contains("sonnet-4-") {
        return "claude-sonnet-4".to_string();
    }
    // 用户已经填的短名直接透传
    model.to_string()
}

// ============================================================
// 响应转换 (SSE 状态机)
// ============================================================

/// Anthropic SSE 事件的最小输出表示. 调用方序列化成 `event: <name>\ndata: <json>\n\n`.
#[derive(Debug, Clone)]
pub struct AnthropicEvent {
    pub event: &'static str,
    pub data: Value,
}

impl AnthropicEvent {
    pub fn to_sse_bytes(&self) -> Vec<u8> {
        let payload = serde_json::to_string(&self.data).unwrap_or_else(|_| "{}".into());
        format!("event: {}\ndata: {}\n\n", self.event, payload).into_bytes()
    }
}

/// 流式状态机. 喂上游 EventStreamFrame, 返回 Anthropic SSE 事件序列.
///
/// 维护多 content block (因为可能 text + 多 tool_use 交替). 每个 tool_use 用 toolUseId 索引.
pub struct KiroSseConverter {
    started: bool,
    stopped: bool,
    message_id: String,
    response_model: String,
    /// 当前活跃的 text block index (None 表示尚未开 text block)
    text_block_index: Option<u32>,
    /// tool_use_id → 已分配的 anthropic content_block index
    tool_blocks: HashMap<String, u32>,
    /// 下一个分配的 content_block index
    next_index: u32,
    /// 累计 usage (流末从 messageMetadataEvent / contextUsageEvent 提取)
    usage: Value,
    stop_reason: String,
}

impl KiroSseConverter {
    pub fn new(response_model: &str) -> Self {
        Self {
            started: false,
            stopped: false,
            message_id: format!("msg_{}", Uuid::new_v4().simple()),
            response_model: response_model.to_string(),
            text_block_index: None,
            tool_blocks: HashMap::new(),
            next_index: 0,
            usage: json!({ "input_tokens": 0, "output_tokens": 0 }),
            stop_reason: "end_turn".to_string(),
        }
    }

    /// 喂一个上游 frame, 返回 0 或多个 Anthropic SSE 事件.
    /// Frame 解析失败或未知 event-type 时只 warn 日志, 不 emit 事件.
    pub fn feed(&mut self, frame: &EventStreamFrame) -> Vec<AnthropicEvent> {
        let mut out = Vec::new();
        if !self.started {
            out.push(self.emit_message_start());
            self.started = true;
        }

        let message_type = frame.message_type().unwrap_or("event");
        if message_type == "exception" || message_type == "error" {
            let msg = parse_payload_json(&frame.payload)
                .as_ref()
                .and_then(|v| v.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("kiro upstream error")
                .to_string();
            let err_type = frame.event_type().unwrap_or("upstream_error").to_string();
            out.push(AnthropicEvent {
                event: "error",
                data: json!({
                    "type": "error",
                    "error": { "type": err_type, "message": msg }
                }),
            });
            self.stop_reason = "error".to_string();
            return out;
        }

        match frame.event_type().unwrap_or("") {
            "assistantResponseEvent" => {
                let payload = parse_payload_json(&frame.payload).unwrap_or(json!({}));
                let text = payload.get("content").and_then(|v| v.as_str()).unwrap_or("");
                if !text.is_empty() {
                    let idx = self.ensure_text_block(&mut out);
                    out.push(AnthropicEvent {
                        event: "content_block_delta",
                        data: json!({
                            "type": "content_block_delta",
                            "index": idx,
                            "delta": { "type": "text_delta", "text": text },
                        }),
                    });
                }
            }
            "toolUseEvent" => {
                let payload = parse_payload_json(&frame.payload).unwrap_or(json!({}));
                let tool_use_id = payload
                    .get("toolUseId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = payload.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let input_fragment = payload.get("input").and_then(|v| v.as_str()).unwrap_or("");
                let stop = payload.get("stop").and_then(|v| v.as_bool()).unwrap_or(false);
                if tool_use_id.is_empty() {
                    return out;
                }

                let idx = if let Some(&idx) = self.tool_blocks.get(&tool_use_id) {
                    idx
                } else {
                    self.close_text_block_if_open(&mut out);
                    let idx = self.next_index;
                    self.next_index += 1;
                    self.tool_blocks.insert(tool_use_id.clone(), idx);
                    out.push(AnthropicEvent {
                        event: "content_block_start",
                        data: json!({
                            "type": "content_block_start",
                            "index": idx,
                            "content_block": {
                                "type": "tool_use",
                                "id": tool_use_id,
                                "name": name,
                                "input": {},
                            }
                        }),
                    });
                    idx
                };

                if !input_fragment.is_empty() {
                    out.push(AnthropicEvent {
                        event: "content_block_delta",
                        data: json!({
                            "type": "content_block_delta",
                            "index": idx,
                            "delta": {
                                "type": "input_json_delta",
                                "partial_json": input_fragment,
                            }
                        }),
                    });
                }

                if stop {
                    out.push(AnthropicEvent {
                        event: "content_block_stop",
                        data: json!({ "type": "content_block_stop", "index": idx }),
                    });
                    self.stop_reason = "tool_use".to_string();
                }
            }
            "contextUsageEvent" => {
                let payload = parse_payload_json(&frame.payload).unwrap_or(json!({}));
                if let Some(input_tokens) = payload
                    .get("inputTokenCount")
                    .and_then(|v| v.as_u64())
                {
                    self.usage["input_tokens"] = json!(input_tokens);
                }
                if let Some(output_tokens) = payload
                    .get("outputTokenCount")
                    .and_then(|v| v.as_u64())
                {
                    self.usage["output_tokens"] = json!(output_tokens);
                }
            }
            "messageMetadataEvent" | "codeReferenceEvent" | "followupPromptEvent" => {}
            other if !other.is_empty() => {
                tracing::warn!(target: "kiro", event_type = %other, "未知 Kiro 事件类型, 已忽略");
            }
            _ => {}
        }

        out
    }

    /// 当前累计 usage (`{ "input_tokens": N, "output_tokens": M }`). 流末读取做请求日志.
    pub fn usage(&self) -> &Value {
        &self.usage
    }

    /// 流结束时调用. emit `message_delta` + `message_stop` (若尚未 emit).
    pub fn finalize(&mut self) -> Vec<AnthropicEvent> {
        let mut out = Vec::new();
        if !self.started {
            out.push(self.emit_message_start());
            self.started = true;
        }
        self.close_text_block_if_open(&mut out);
        if !self.stopped {
            out.push(AnthropicEvent {
                event: "message_delta",
                data: json!({
                    "type": "message_delta",
                    "delta": { "stop_reason": self.stop_reason, "stop_sequence": null },
                    "usage": self.usage,
                }),
            });
            out.push(AnthropicEvent {
                event: "message_stop",
                data: json!({ "type": "message_stop" }),
            });
            self.stopped = true;
        }
        out
    }

    fn emit_message_start(&self) -> AnthropicEvent {
        AnthropicEvent {
            event: "message_start",
            data: json!({
                "type": "message_start",
                "message": {
                    "id": self.message_id,
                    "type": "message",
                    "role": "assistant",
                    "model": self.response_model,
                    "content": [],
                    "stop_reason": null,
                    "stop_sequence": null,
                    "usage": self.usage,
                }
            }),
        }
    }

    fn ensure_text_block(&mut self, out: &mut Vec<AnthropicEvent>) -> u32 {
        if let Some(idx) = self.text_block_index {
            return idx;
        }
        let idx = self.next_index;
        self.next_index += 1;
        self.text_block_index = Some(idx);
        out.push(AnthropicEvent {
            event: "content_block_start",
            data: json!({
                "type": "content_block_start",
                "index": idx,
                "content_block": { "type": "text", "text": "" }
            }),
        });
        idx
    }

    fn close_text_block_if_open(&mut self, out: &mut Vec<AnthropicEvent>) {
        if let Some(idx) = self.text_block_index.take() {
            out.push(AnthropicEvent {
                event: "content_block_stop",
                data: json!({ "type": "content_block_stop", "index": idx }),
            });
        }
    }
}

fn parse_payload_json(payload: &[u8]) -> Option<Value> {
    serde_json::from_slice(payload).ok()
}

// ============================================================
// 非流式收集器
// ============================================================

/// 把所有上游 frame 收完, finalize 出 Anthropic Message JSON (非 SSE).
/// 用于 Claude Code 发非流式 (stream=false) 请求时把上游强制流式响应拼回 JSON.
pub struct NonStreamingCollector {
    converter: KiroSseConverter,
    /// content blocks 累积. 按 index 索引.
    text_acc: HashMap<u32, String>,
    tool_args_acc: HashMap<u32, String>,
    block_kind: HashMap<u32, BlockKind>,
    /// 保留 block 出现顺序, 输出时按此排序
    order: Vec<u32>,
    /// tool_use 元数据 (name / id) 存档供 finalize 用
    tool_meta: HashMap<u32, (String, String)>,
}

enum BlockKind {
    Text,
    ToolUse,
}

impl NonStreamingCollector {
    pub fn new(response_model: &str) -> Self {
        Self {
            converter: KiroSseConverter::new(response_model),
            text_acc: HashMap::new(),
            tool_args_acc: HashMap::new(),
            block_kind: HashMap::new(),
            order: Vec::new(),
            tool_meta: HashMap::new(),
        }
    }

    pub fn feed(&mut self, frame: &EventStreamFrame) {
        let events = self.converter.feed(frame);
        for ev in events {
            self.consume_event(&ev);
        }
    }

    pub fn finalize(mut self) -> Value {
        let tail = self.converter.finalize();
        for ev in &tail {
            self.consume_event(ev);
        }

        let content: Vec<Value> = self
            .order
            .iter()
            .filter_map(|idx| match self.block_kind.get(idx)? {
                BlockKind::Text => {
                    let text = self.text_acc.get(idx).cloned().unwrap_or_default();
                    Some(json!({ "type": "text", "text": text }))
                }
                BlockKind::ToolUse => {
                    let (name, id) = self.tool_meta.get(idx).cloned()?;
                    let args_str = self.tool_args_acc.get(idx).cloned().unwrap_or_default();
                    let input: Value =
                        serde_json::from_str(&args_str).unwrap_or_else(|_| json!({}));
                    Some(json!({
                        "type": "tool_use",
                        "id": id,
                        "name": name,
                        "input": input,
                    }))
                }
            })
            .collect();

        json!({
            "id": self.converter.message_id,
            "type": "message",
            "role": "assistant",
            "model": self.converter.response_model,
            "content": content,
            "stop_reason": self.converter.stop_reason,
            "stop_sequence": null,
            "usage": self.converter.usage,
        })
    }

    fn consume_event(&mut self, ev: &AnthropicEvent) {
        match ev.event {
            "content_block_start" => {
                let idx = ev.data.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let cb = match ev.data.get("content_block") {
                    Some(v) => v,
                    None => return,
                };
                let ty = cb.get("type").and_then(|v| v.as_str()).unwrap_or("");
                self.order.push(idx);
                match ty {
                    "text" => {
                        self.block_kind.insert(idx, BlockKind::Text);
                        self.text_acc.insert(idx, String::new());
                    }
                    "tool_use" => {
                        self.block_kind.insert(idx, BlockKind::ToolUse);
                        self.tool_args_acc.insert(idx, String::new());
                        let name = cb.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let id = cb.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        self.tool_meta.insert(idx, (name, id));
                    }
                    _ => {}
                }
            }
            "content_block_delta" => {
                let idx = ev.data.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let delta = match ev.data.get("delta") {
                    Some(v) => v,
                    None => return,
                };
                let delta_ty = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match delta_ty {
                    "text_delta" => {
                        if let Some(t) = delta.get("text").and_then(|v| v.as_str()) {
                            self.text_acc.entry(idx).or_default().push_str(t);
                        }
                    }
                    "input_json_delta" => {
                        if let Some(t) = delta.get("partial_json").and_then(|v| v.as_str()) {
                            self.tool_args_acc.entry(idx).or_default().push_str(t);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::transform::aws_event_stream::{
        build_frame, EventStreamDecoder, HeaderValue,
    };

    fn make_frame(event_type: &str, payload_json: Value) -> EventStreamFrame {
        let bytes = build_frame(
            &[
                (":event-type", HeaderValue::String(event_type.into())),
                (":message-type", HeaderValue::String("event".into())),
                (":content-type", HeaderValue::String("application/json".into())),
            ],
            payload_json.to_string().as_bytes(),
        );
        let mut d = EventStreamDecoder::new();
        d.feed_and_drain(&bytes).unwrap().pop().unwrap()
    }

    #[test]
    fn normalize_model_short_names() {
        assert_eq!(normalize_to_kiro_model("claude-sonnet-4.5"), "claude-sonnet-4.5");
        assert_eq!(
            normalize_to_kiro_model("claude-sonnet-4-5-20250929"),
            "claude-sonnet-4.5"
        );
        assert_eq!(normalize_to_kiro_model("claude-haiku-4-5"), "claude-haiku-4.5");
        assert_eq!(normalize_to_kiro_model("claude-3-7-sonnet-latest"), "claude-3-7-sonnet");
    }

    #[test]
    fn anthropic_to_codewhisperer_text_only() {
        let body = json!({
            "model": "claude-sonnet-4.5",
            "messages": [{"role": "user", "content": "hi"}],
        });
        let cw = anthropic_to_codewhisperer(&body, Some("arn:test"), None).unwrap();
        assert_eq!(cw["profileArn"], "arn:test");
        let cur = &cw["conversationState"]["currentMessage"]["userInputMessage"];
        assert_eq!(cur["content"], "hi");
        assert_eq!(cur["modelId"], "claude-sonnet-4.5");
        assert!(cw["conversationState"]["history"].as_array().unwrap().is_empty());
    }

    #[test]
    fn anthropic_to_codewhisperer_with_system_and_history() {
        let body = json!({
            "model": "claude-sonnet-4.5",
            "system": "You are helpful.",
            "messages": [
                {"role": "user", "content": "first"},
                {"role": "assistant", "content": "ok"},
                {"role": "user", "content": "second"},
            ],
        });
        let cw = anthropic_to_codewhisperer(&body, None, None).unwrap();
        let history = cw["conversationState"]["history"].as_array().unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0]["userInputMessage"]["content"], "first");
        assert_eq!(history[1]["assistantResponseMessage"]["content"], "ok");
        let cur = &cw["conversationState"]["currentMessage"]["userInputMessage"];
        // system 拼到当前 user message 前
        assert!(cur["content"].as_str().unwrap().starts_with("You are helpful."));
        assert!(cur["content"].as_str().unwrap().contains("second"));
        assert!(!cw.as_object().unwrap().contains_key("profileArn"));
    }

    #[test]
    fn anthropic_to_codewhisperer_with_tools_and_tool_results() {
        let body = json!({
            "model": "claude-sonnet-4.5",
            "messages": [
                {"role": "user", "content": "use bash"},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "running"},
                    {"type": "tool_use", "id": "tu_1", "name": "Bash", "input": {"cmd": "ls"}},
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "tu_1", "content": "ok"},
                    {"type": "text", "text": "next"},
                ]},
            ],
            "tools": [
                {"name": "Bash", "description": "run cmd", "input_schema": {"type": "object"}}
            ],
        });
        let cw = anthropic_to_codewhisperer(&body, None, None).unwrap();
        let history = cw["conversationState"]["history"].as_array().unwrap();
        assert_eq!(history.len(), 2);
        let asst = &history[1]["assistantResponseMessage"];
        assert_eq!(asst["toolUses"][0]["toolUseId"], "tu_1");
        assert_eq!(asst["toolUses"][0]["name"], "Bash");
        let cur = &cw["conversationState"]["currentMessage"]["userInputMessage"];
        let ctx = &cur["userInputMessageContext"];
        assert_eq!(ctx["tools"][0]["toolSpecification"]["name"], "Bash");
        assert_eq!(ctx["toolResults"][0]["toolUseId"], "tu_1");
        assert_eq!(ctx["toolResults"][0]["status"], "success");
    }

    #[test]
    fn kiro_sse_converter_emits_message_start_on_first_event() {
        let mut conv = KiroSseConverter::new("claude-sonnet-4.5");
        let frame = make_frame("assistantResponseEvent", json!({"content": "h"}));
        let events = conv.feed(&frame);
        let names: Vec<_> = events.iter().map(|e| e.event).collect();
        assert_eq!(names, vec!["message_start", "content_block_start", "content_block_delta"]);
    }

    #[test]
    fn kiro_sse_converter_text_delta_accumulation() {
        let mut conv = KiroSseConverter::new("claude-sonnet-4.5");
        let f1 = make_frame("assistantResponseEvent", json!({"content": "hel"}));
        let f2 = make_frame("assistantResponseEvent", json!({"content": "lo"}));
        let mut all_events = Vec::new();
        all_events.extend(conv.feed(&f1));
        all_events.extend(conv.feed(&f2));
        all_events.extend(conv.finalize());

        // 期望: message_start, content_block_start, delta("hel"), delta("lo"),
        //       content_block_stop, message_delta, message_stop
        let names: Vec<_> = all_events.iter().map(|e| e.event).collect();
        assert_eq!(
            names,
            vec![
                "message_start",
                "content_block_start",
                "content_block_delta",
                "content_block_delta",
                "content_block_stop",
                "message_delta",
                "message_stop"
            ]
        );
    }

    #[test]
    fn kiro_sse_converter_tool_use_lifecycle() {
        let mut conv = KiroSseConverter::new("claude-sonnet-4.5");
        let f_text = make_frame("assistantResponseEvent", json!({"content": "running"}));
        let f_tool1 = make_frame(
            "toolUseEvent",
            json!({"toolUseId":"tu_1","name":"Bash","input":"{\"cmd\":\"","stop":false}),
        );
        let f_tool2 = make_frame(
            "toolUseEvent",
            json!({"toolUseId":"tu_1","name":"Bash","input":"ls\"}","stop":true}),
        );
        let mut all = Vec::new();
        all.extend(conv.feed(&f_text));
        all.extend(conv.feed(&f_tool1));
        all.extend(conv.feed(&f_tool2));
        all.extend(conv.finalize());
        let names: Vec<_> = all.iter().map(|e| e.event).collect();
        assert_eq!(
            names,
            vec![
                "message_start",
                "content_block_start", // text block
                "content_block_delta", // "running"
                "content_block_stop",  // close text
                "content_block_start", // tool_use block
                "content_block_delta", // input partial 1
                "content_block_delta", // input partial 2
                "content_block_stop",  // stop=true 触发
                "message_delta",
                "message_stop"
            ]
        );
        // 找 message_delta 验 stop_reason
        let md = all.iter().find(|e| e.event == "message_delta").unwrap();
        assert_eq!(md.data["delta"]["stop_reason"], "tool_use");
    }

    #[test]
    fn kiro_sse_converter_usage_from_context_event() {
        let mut conv = KiroSseConverter::new("claude-sonnet-4.5");
        let f_text = make_frame("assistantResponseEvent", json!({"content": "x"}));
        let f_usage = make_frame(
            "contextUsageEvent",
            json!({"inputTokenCount": 100, "outputTokenCount": 5}),
        );
        let _ = conv.feed(&f_text);
        let _ = conv.feed(&f_usage);
        let tail = conv.finalize();
        let md = tail.iter().find(|e| e.event == "message_delta").unwrap();
        assert_eq!(md.data["usage"]["input_tokens"], 100);
        assert_eq!(md.data["usage"]["output_tokens"], 5);
    }

    #[test]
    fn kiro_sse_converter_unknown_event_ignored() {
        let mut conv = KiroSseConverter::new("claude-sonnet-4.5");
        let f = make_frame("someUnknownEvent", json!({"foo": "bar"}));
        let events = conv.feed(&f);
        // 仅 emit message_start, 未知事件被吞掉
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "message_start");
    }

    #[test]
    fn non_streaming_collector_assembles_full_message() {
        let mut col = NonStreamingCollector::new("claude-sonnet-4.5");
        let f1 = make_frame("assistantResponseEvent", json!({"content": "hello "}));
        let f2 = make_frame("assistantResponseEvent", json!({"content": "world"}));
        let f3 = make_frame(
            "toolUseEvent",
            json!({"toolUseId":"tu_1","name":"Bash","input":"{\"x\":1}","stop":true}),
        );
        col.feed(&f1);
        col.feed(&f2);
        col.feed(&f3);
        let msg = col.finalize();
        assert_eq!(msg["type"], "message");
        assert_eq!(msg["role"], "assistant");
        assert_eq!(msg["content"][0]["type"], "text");
        assert_eq!(msg["content"][0]["text"], "hello world");
        assert_eq!(msg["content"][1]["type"], "tool_use");
        assert_eq!(msg["content"][1]["id"], "tu_1");
        assert_eq!(msg["content"][1]["name"], "Bash");
        assert_eq!(msg["content"][1]["input"]["x"], 1);
        assert_eq!(msg["stop_reason"], "tool_use");
    }
}
