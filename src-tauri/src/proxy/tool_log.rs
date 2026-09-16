//! 请求日志的「工具调用」观测字段 (spec 2026-09-16 §4.2)。
//!
//! **硬约束**: 本模块所有函数拿不到值一律 None / 0 / 空, 绝不 panic、绝不影响日志投递 ——
//! 与 [`crate::proxy::effort_log`] 同纪律。
//!
//! 三类值:
//! - 请求侧 [`RequestToolShape`]: 从 Anthropic 形态 body 算一次 (handler 构造 `ClientContext` 处)
//! - 响应侧 [`ToolUseTally`]: 每条 dispatch 路径持有一个, 在 usage 提取点旁喂 content block / stop_reason
//! - 落库 [`ToolLogFields`]: 两者合成, 进 `RequestLogEntry.tool_calls`

use serde_json::Value;

/// `tool_use_names` JSON 串上限 (字节), 与 `upstream_response_body` 同一截断纪律。
pub const TOOL_NAMES_LIMIT: usize = 4096;
/// 截断后追加到数组末尾的标记; flush 聚合时跳过它。
pub const TRUNCATED_MARKER: &str = "…";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestToolShape {
    /// 请求 `tools[]` 长度; `tools` 缺失或不是数组 → None
    pub tools_offered_count: Option<u32>,
    /// **最后一条** user 消息里的 `tool_result` 块数; 没有 user 消息 → None
    pub tool_result_count: Option<u32>,
}

pub fn request_tool_shape(body: &Value) -> RequestToolShape {
    let tools_offered_count = body
        .get("tools")
        .and_then(Value::as_array)
        .map(|a| a.len() as u32);

    let tool_result_count = body
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|msgs| {
            msgs.iter()
                .rev()
                .find(|m| m.get("role").and_then(Value::as_str) == Some("user"))
        })
        .map(|last_user| {
            last_user
                .get("content")
                .and_then(Value::as_array)
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
                        .count() as u32
                })
                .unwrap_or(0)
        });

    RequestToolShape { tools_offered_count, tool_result_count }
}

fn is_tool_block(block: &Value) -> bool {
    matches!(
        block.get("type").and_then(Value::as_str),
        Some("tool_use") | Some("server_tool_use")
    )
}

#[derive(Debug, Clone, Default)]
pub struct ToolUseTally {
    names: Vec<String>,
    stop_reason: Option<String>,
}

impl ToolUseTally {
    /// 直接记一个工具名 (透传 SSE 路径已在 `process_event` 里解析好 name)。
    pub fn observe_name(&mut self, name: &str) {
        self.names.push(name.to_string());
    }

    /// 喂一个 Anthropic content block; 只有 `tool_use` / `server_tool_use` 计入。
    pub fn observe_block(&mut self, block: &Value) {
        if !is_tool_block(block) {
            return;
        }
        let name = block.get("name").and_then(Value::as_str).unwrap_or("");
        self.observe_name(name);
    }

    /// 记 stop_reason; None 不覆盖已有值 (兜底 message_delta 可能不带)。
    pub fn observe_stop_reason(&mut self, reason: Option<&str>) {
        if let Some(r) = reason {
            self.stop_reason = Some(r.to_string());
        }
    }

    /// 喂一个 Anthropic SSE 事件的 data JSON (`{"type":"content_block_start",...}` 等)。
    /// 透传路径的 data 行、gemini / kiro converter 的 `AnthropicEvent.data` 都是这个形状。
    pub fn observe_event_json(&mut self, data: &Value) {
        match data.get("type").and_then(Value::as_str) {
            Some("content_block_start") => {
                if let Some(block) = data.get("content_block") {
                    self.observe_block(block);
                }
            }
            Some("message_delta") => {
                let reason = data
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(Value::as_str);
                self.observe_stop_reason(reason);
            }
            _ => {}
        }
    }

    /// 喂 Responses / Chat Completions 翻译层产出的 [`AnthropicEvent`] 枚举
    /// (codex / openai / openai-chat 三条路径共用这个事件类型)。
    pub fn observe_responses_event(
        &mut self,
        evt: &crate::proxy::transform::responses_common::AnthropicEvent,
    ) {
        use crate::proxy::transform::responses_common::AnthropicEvent as E;
        match evt {
            E::ContentBlockStart { content_block, .. } => self.observe_block(content_block),
            E::MessageDelta { delta, .. } => {
                self.observe_stop_reason(delta.get("stop_reason").and_then(Value::as_str))
            }
            _ => {}
        }
    }

    /// 喂一个完整的非流式 Anthropic message JSON (`content[]` + `stop_reason`)。
    pub fn observe_message(&mut self, message: &Value) {
        if let Some(blocks) = message.get("content").and_then(Value::as_array) {
            for b in blocks {
                self.observe_block(b);
            }
        }
        self.observe_stop_reason(message.get("stop_reason").and_then(Value::as_str));
    }

    /// 合成落库字段。`tool_use_count` 恒 Some (成功路径观测过就有值, 哪怕是 0);
    /// `tool_use_names` 在 0 次时为 None (省空间, 前端好判空)。
    pub fn fields(&self, req: &RequestToolShape) -> ToolLogFields {
        ToolLogFields {
            stop_reason: self.stop_reason.clone(),
            tools_offered_count: req.tools_offered_count,
            tool_result_count: req.tool_result_count,
            tool_use_count: Some(self.names.len() as u32),
            tool_use_names: if self.names.is_empty() {
                None
            } else {
                Some(names_json_truncated(&self.names))
            },
        }
    }
}

/// 序列化名字数组; 超过 [`TOOL_NAMES_LIMIT`] 时从尾部丢名并追加 [`TRUNCATED_MARKER`]。
fn names_json_truncated(names: &[String]) -> String {
    let full = serde_json::to_string(names).unwrap_or_else(|_| "[]".to_string());
    if full.len() <= TOOL_NAMES_LIMIT {
        return full;
    }
    let mut kept: Vec<&str> = names.iter().map(String::as_str).collect();
    loop {
        kept.pop();
        let mut with_marker: Vec<&str> = kept.clone();
        with_marker.push(TRUNCATED_MARKER);
        let s = serde_json::to_string(&with_marker).unwrap_or_else(|_| "[]".to_string());
        if s.len() <= TOOL_NAMES_LIMIT || kept.is_empty() {
            return s;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolLogFields {
    pub stop_reason: Option<String>,
    pub tools_offered_count: Option<u32>,
    pub tool_result_count: Option<u32>,
    pub tool_use_count: Option<u32>,
    pub tool_use_names: Option<String>,
}

impl ToolLogFields {
    /// 错误 / 未拿到响应的路径: 只有请求侧两格, 响应侧三格全 None。
    pub fn request_only(req: &RequestToolShape) -> Self {
        Self {
            stop_reason: None,
            tools_offered_count: req.tools_offered_count,
            tool_result_count: req.tool_result_count,
            tool_use_count: None,
            tool_use_names: None,
        }
    }

    /// 全 None。测试 helper 与无 body 的路径用。刻意不实现 `Default`,
    /// 让 24 个 `RequestLogEntry` 构造点漏写 `tool_calls` 时编译失败。
    pub fn empty() -> Self {
        Self::request_only(&RequestToolShape::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_shape_counts_tools_and_last_user_tool_results() {
        let body = json!({
            "tools": [{"name": "Read"}, {"name": "Bash"}, {"name": "Write"}],
            "messages": [
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "t1", "name": "Read", "input": {}},
                    {"type": "tool_use", "id": "t2", "name": "Bash", "input": {}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "t1", "content": "a"},
                    {"type": "tool_result", "tool_use_id": "t2", "content": "b"}
                ]},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "t3", "name": "Write", "input": {}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "t3", "content": "c"},
                    {"type": "text", "text": "继续"}
                ]}
            ]
        });
        let shape = request_tool_shape(&body);
        assert_eq!(shape.tools_offered_count, Some(3));
        // 只数最后一条 user 消息 (1 个), 不把历史轮的 2 个也加进来
        assert_eq!(shape.tool_result_count, Some(1));
    }

    #[test]
    fn request_shape_handles_missing_and_malformed_fields() {
        assert_eq!(
            request_tool_shape(&json!({"messages": [{"role": "user", "content": "hi"}]})),
            RequestToolShape { tools_offered_count: None, tool_result_count: Some(0) }
        );
        assert_eq!(
            request_tool_shape(&json!({"tools": [], "messages": []})),
            RequestToolShape { tools_offered_count: Some(0), tool_result_count: None }
        );
        assert_eq!(
            request_tool_shape(&json!({"tools": "oops", "messages": "oops"})),
            RequestToolShape::default()
        );
        assert_eq!(request_tool_shape(&json!(null)), RequestToolShape::default());
        // 最后一条是 assistant → 往前找最近的 user
        let body = json!({"messages": [
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "x", "content": "r"}]},
            {"role": "assistant", "content": "ok"}
        ]});
        assert_eq!(request_tool_shape(&body).tool_result_count, Some(1));
    }

    #[test]
    fn tally_counts_tool_use_and_server_tool_use_blocks() {
        let mut t = ToolUseTally::default();
        t.observe_block(&json!({"type": "tool_use", "id": "a", "name": "Read", "input": {}}));
        t.observe_block(&json!({"type": "text", "text": "x"}));
        t.observe_block(&json!({"type": "server_tool_use", "id": "b", "name": "web_search", "input": {}}));
        t.observe_block(&json!({"type": "tool_use", "id": "c", "name": "Read", "input": {}}));
        t.observe_block(&json!({"type": "tool_use", "id": "d"})); // 缺 name → 计数但名为空串
        t.observe_stop_reason(Some("tool_use"));
        let f = t.fields(&RequestToolShape { tools_offered_count: Some(5), tool_result_count: Some(0) });
        assert_eq!(f.tool_use_count, Some(4));
        assert_eq!(f.tool_use_names.as_deref(), Some(r#"["Read","web_search","Read",""]"#));
        assert_eq!(f.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(f.tools_offered_count, Some(5));
        assert_eq!(f.tool_result_count, Some(0));
    }

    #[test]
    fn tally_with_no_tool_use_yields_zero_count_and_no_names() {
        let mut t = ToolUseTally::default();
        t.observe_stop_reason(Some("end_turn"));
        let f = t.fields(&RequestToolShape::default());
        assert_eq!(f.tool_use_count, Some(0));
        assert_eq!(f.tool_use_names, None);
        assert_eq!(f.stop_reason.as_deref(), Some("end_turn"));
    }

    #[test]
    fn tally_observe_event_json_handles_content_block_start_and_message_delta() {
        let mut t = ToolUseTally::default();
        t.observe_event_json(&json!({
            "type": "content_block_start", "index": 0,
            "content_block": {"type": "tool_use", "id": "a", "name": "Bash", "input": {}}
        }));
        t.observe_event_json(&json!({"type": "content_block_delta", "index": 0, "delta": {}}));
        t.observe_event_json(&json!({
            "type": "message_delta", "delta": {"stop_reason": "tool_use", "stop_sequence": null}, "usage": {}
        }));
        t.observe_event_json(&json!("garbage"));
        let f = t.fields(&RequestToolShape::default());
        assert_eq!(f.tool_use_count, Some(1));
        assert_eq!(f.tool_use_names.as_deref(), Some(r#"["Bash"]"#));
        assert_eq!(f.stop_reason.as_deref(), Some("tool_use"));
    }

    #[test]
    fn tally_observe_message_scans_content_and_stop_reason() {
        let mut t = ToolUseTally::default();
        t.observe_message(&json!({
            "type": "message",
            "content": [
                {"type": "text", "text": "hi"},
                {"type": "tool_use", "id": "a", "name": "mcp__ctx7__query", "input": {}}
            ],
            "stop_reason": "tool_use"
        }));
        let f = t.fields(&RequestToolShape::default());
        assert_eq!(f.tool_use_count, Some(1));
        assert_eq!(f.tool_use_names.as_deref(), Some(r#"["mcp__ctx7__query"]"#));
        assert_eq!(f.stop_reason.as_deref(), Some("tool_use"));
        // 非对象 / 无 content 不 panic
        let mut t2 = ToolUseTally::default();
        t2.observe_message(&json!(null));
        t2.observe_message(&json!({"content": "not an array"}));
        assert_eq!(t2.fields(&RequestToolShape::default()).tool_use_count, Some(0));
    }

    #[test]
    fn tally_stop_reason_last_non_null_wins_and_null_is_ignored() {
        let mut t = ToolUseTally::default();
        t.observe_stop_reason(Some("end_turn"));
        t.observe_stop_reason(None);
        assert_eq!(t.fields(&RequestToolShape::default()).stop_reason.as_deref(), Some("end_turn"));
        t.observe_stop_reason(Some("max_tokens"));
        assert_eq!(t.fields(&RequestToolShape::default()).stop_reason.as_deref(), Some("max_tokens"));
    }

    #[test]
    fn names_json_is_truncated_with_marker_when_over_limit() {
        let mut t = ToolUseTally::default();
        let long = "x".repeat(300);
        for _ in 0..30 {
            t.observe_name(&long); // 30 × ~303 B ≈ 9 KB > 4096
        }
        let f = t.fields(&RequestToolShape::default());
        let names = f.tool_use_names.expect("names present");
        assert!(names.len() <= TOOL_NAMES_LIMIT, "len={}", names.len());
        let parsed: Vec<String> = serde_json::from_str(&names).unwrap();
        assert_eq!(parsed.last().map(String::as_str), Some(TRUNCATED_MARKER));
        assert!(parsed.len() < 30);
        // 计数记的是真实次数, 不因截断而变
        assert_eq!(f.tool_use_count, Some(30));
    }

    #[test]
    fn tally_observe_responses_event_enum() {
        use crate::proxy::transform::responses_common::AnthropicEvent;
        let mut t = ToolUseTally::default();
        t.observe_responses_event(&AnthropicEvent::ContentBlockStart {
            index: 0,
            content_block: json!({"type": "tool_use", "id": "call_1", "name": "get_weather", "input": {}}),
        });
        t.observe_responses_event(&AnthropicEvent::ContentBlockStart {
            index: 1,
            content_block: json!({"type": "text", "text": ""}),
        });
        t.observe_responses_event(&AnthropicEvent::MessageDelta {
            delta: json!({"stop_reason": "tool_use", "stop_sequence": null}),
            usage: json!({"input_tokens": 1, "output_tokens": 1}),
        });
        t.observe_responses_event(&AnthropicEvent::MessageStop);
        let f = t.fields(&RequestToolShape::default());
        assert_eq!(f.tool_use_count, Some(1));
        assert_eq!(f.tool_use_names.as_deref(), Some(r#"["get_weather"]"#));
        assert_eq!(f.stop_reason.as_deref(), Some("tool_use"));
    }

    #[test]
    fn request_only_and_empty_fields() {
        let req = RequestToolShape { tools_offered_count: Some(2), tool_result_count: Some(1) };
        let f = ToolLogFields::request_only(&req);
        assert_eq!(f.tools_offered_count, Some(2));
        assert_eq!(f.tool_result_count, Some(1));
        assert_eq!(f.tool_use_count, None);
        assert_eq!(f.tool_use_names, None);
        assert_eq!(f.stop_reason, None);
        assert_eq!(ToolLogFields::empty(), ToolLogFields::request_only(&RequestToolShape::default()));
    }

    #[test]
    fn tally_reads_gemini_style_struct_events() {
        use crate::proxy::transform::gemini::GeminiSseConverter;
        let mut conv = GeminiSseConverter::new("gemini-2.5-pro");
        let frame = json!({
            "candidates": [{
                "content": {"parts": [{"functionCall": {"name": "get_weather", "args": {"city": "Tokyo"}}}]},
                "finishReason": "STOP"
            }],
            "usageMetadata": {"promptTokenCount": 3, "candidatesTokenCount": 4}
        });
        let mut t = ToolUseTally::default();
        for evt in conv.feed(&frame) {
            t.observe_event_json(&evt.data);
        }
        for evt in conv.finalize() {
            t.observe_event_json(&evt.data);
        }
        let f = t.fields(&RequestToolShape::default());
        assert_eq!(f.tool_use_count, Some(1));
        assert_eq!(f.tool_use_names.as_deref(), Some(r#"["get_weather"]"#));
        assert!(f.stop_reason.is_some());
    }

    #[test]
    fn tally_reads_gemini_interactions_struct_events() {
        use crate::proxy::transform::gemini_interactions::InteractionsSseConverter;
        // 帧形状照抄 gemini_interactions.rs::tests::sse_converter_function_call_sets_tool_use_stop_reason
        let frames = vec![
            json!({"interaction":{"model":"m"},"event_type":"interaction.created"}),
            json!({"index":0,"step":{"type":"function_call","id":"ca1","name":"get_weather","arguments":{"city":"Tokyo"}},"event_type":"step.start"}),
            json!({"index":0,"event_type":"step.stop"}),
            json!({"interaction":{"status":"requires_action","usage":{"total_input_tokens":50,"total_output_tokens":15}},"event_type":"interaction.completed"}),
        ];
        let mut conv = InteractionsSseConverter::new("gemini-3.1-flash-lite");
        let mut t = ToolUseTally::default();
        for f in &frames {
            for evt in conv.feed(f) {
                t.observe_event_json(&evt.data);
            }
        }
        for evt in conv.finalize() {
            t.observe_event_json(&evt.data);
        }
        let f = t.fields(&RequestToolShape::default());
        assert_eq!(f.tool_use_count, Some(1));
        assert_eq!(f.tool_use_names.as_deref(), Some(r#"["get_weather"]"#));
        assert_eq!(f.stop_reason.as_deref(), Some("tool_use"));
    }

    #[test]
    fn tally_reads_kiro_struct_events() {
        use crate::proxy::transform::aws_event_stream::{
            build_frame, EventStreamDecoder, HeaderValue,
        };
        use crate::proxy::transform::kiro_codewhisperer::KiroSseConverter;

        fn make_frame(event_type: &str, payload_json: Value) -> crate::proxy::transform::aws_event_stream::EventStreamFrame {
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

        let frame = make_frame(
            "toolUseEvent",
            json!({"toolUseId":"tu_1","name":"Bash","input":"{\"cmd\":\"ls\"}","stop":true}),
        );
        let mut conv = KiroSseConverter::new("minimax-m2.5");
        let mut t = ToolUseTally::default();
        for evt in conv.feed(&frame) {
            t.observe_event_json(&evt.data);
        }
        for evt in conv.finalize() {
            t.observe_event_json(&evt.data);
        }
        let f = t.fields(&RequestToolShape::default());
        assert_eq!(f.tool_use_count, Some(1));
        assert_eq!(f.tool_use_names.as_deref(), Some(r#"["Bash"]"#));
        assert_eq!(f.stop_reason.as_deref(), Some("tool_use"));
    }
}
