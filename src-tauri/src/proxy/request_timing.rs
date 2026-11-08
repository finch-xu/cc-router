//! Timing is measured at the proxy's content boundary, never from heartbeats or headers.
use std::time::Instant;

use serde_json::Value;

pub struct StreamTiming {
    start: Instant,
    first_content: Option<Instant>,
    error: Option<String>,
    completed: bool,
    output_usage: bool,
}

impl StreamTiming {
    pub fn new(start: Instant) -> Self {
        Self {
            start,
            first_content: None,
            error: None,
            completed: false,
            output_usage: false,
        }
    }

    /// Input consists of complete emitted SSE frames (possibly several per buffer).
    pub fn observe_sse(&mut self, frames: &[u8], now: Instant) {
        let Ok(text) = std::str::from_utf8(frames) else {
            return;
        };
        for line in text.lines() {
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            if let Ok(event) = serde_json::from_str::<Value>(data.trim()) {
                if event["type"] == "message_stop"
                    || (event["type"] == "message_delta"
                        && event
                            .pointer("/delta/stop_reason")
                            .is_some_and(|v| !v.is_null()))
                {
                    self.completed = true;
                }
                if event.get("type").and_then(Value::as_str) == Some("error") {
                    self.fail(
                        event["error"]["message"]
                            .as_str()
                            .unwrap_or("Upstream stream error")
                            .to_owned(),
                    );
                }
                if self.first_content.is_none() && has_content(&event) {
                    self.first_content = Some(now);
                }
            }
        }
    }

    pub fn first_content_ms(&self) -> Option<u64> {
        self.first_content
            .map(|t| t.saturating_duration_since(self.start).as_millis() as u64)
    }

    pub fn fail(&mut self, message: String) {
        self.error.get_or_insert(message);
    }

    pub fn error_message(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Call BEFORE synthetic converter finalization, which may invent a message_stop.
    pub fn finish_stream(&mut self) {
        if !self.completed {
            self.fail("Upstream stream ended before completion".to_owned());
        }
    }

    pub fn has_output_usage(&self) -> bool {
        self.output_usage
    }

    pub fn observe_responses_frame(&mut self, name: &str, data: &Value) {
        if matches!(name, "response.completed" | "response.incomplete") {
            self.completed = true;
            self.output_usage = data
                .pointer("/response/usage/output_tokens")
                .and_then(Value::as_u64)
                .is_some();
        }
    }

    pub fn observe_gemini_frame(&mut self, data: &Value) {
        if data
            .pointer("/candidates/0/finishReason")
            .and_then(Value::as_str)
            .is_some_and(|v| !v.is_empty() && v != "FINISH_REASON_UNSPECIFIED")
        {
            self.completed = true;
        }
        // Match GeminiSseConverter's output-token mapping; thoughts alone are not output usage.
        if self.completed && data.get("usageMetadata").is_some() {
            self.output_usage = data
                .pointer("/usageMetadata/candidatesTokenCount")
                .and_then(Value::as_u64)
                .is_some();
        }
        if let Some(error) = data.get("error") {
            self.fail(
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Gemini stream error")
                    .to_owned(),
            );
        }
    }

    pub fn observe_interactions_frame(&mut self, data: &Value) {
        match data.get("event_type").and_then(Value::as_str) {
            Some("interaction.completed") => {
                self.completed = true;
                self.output_usage = data
                    .pointer("/interaction/usage/total_output_tokens")
                    .and_then(Value::as_u64)
                    .is_some();
            }
            Some("error") => self.fail(
                data.pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("Gemini Interactions stream error")
                    .to_owned(),
            ),
            _ => {}
        }
    }

    pub fn observe_chat_frame(&mut self, frame: &str) {
        for line in frame.lines() {
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            if data.trim() == "[DONE]" {
                self.completed = true;
                continue;
            }
            let Ok(data) = serde_json::from_str::<Value>(data.trim()) else {
                continue;
            };
            self.completed |= data
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)
                .is_some_and(|v| !v.is_empty());
            if self.completed {
                self.output_usage |= data.pointer("/usage/completion_tokens").is_some_and(|v| {
                    v.as_f64()
                        .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
                        .is_some_and(|n| n.is_finite() && n >= 0.0)
                });
            }
        }
    }
}

fn has_content(event: &Value) -> bool {
    let nonempty = |v: &Value, key: &str| {
        v.get(key)
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
    };
    let block_has_content = |block: &Value| match block.get("type").and_then(Value::as_str) {
        Some("text") => nonempty(block, "text"),
        Some("thinking") => nonempty(block, "thinking"),
        Some("tool_use" | "server_tool_use") => nonempty(block, "name"),
        _ => false,
    };
    match event.get("type").and_then(Value::as_str) {
        Some("content_block_delta") => {
            let delta = &event["delta"];
            match delta.get("type").and_then(Value::as_str) {
                Some("text_delta") => nonempty(delta, "text"),
                Some("thinking_delta") => nonempty(delta, "thinking"),
                Some("input_json_delta") => nonempty(delta, "partial_json"),
                _ => false,
            }
        }
        Some("content_block_start") => block_has_content(&event["content_block"]),
        Some("message_start") => event["message"]["content"]
            .as_array()
            .is_some_and(|blocks| blocks.iter().any(block_has_content)),
        _ => false,
    }
}

pub fn output_tokens_per_second(
    streaming: bool,
    status: &str,
    output_tokens: Option<i64>,
    total_ms: Option<i64>,
    first_content_ms: Option<i64>,
) -> Option<f64> {
    if !streaming || status != "success" {
        return None;
    }
    let (tokens, total, first) = (output_tokens?, total_ms?, first_content_ms?);
    if tokens < 0 || first < 0 || total <= first {
        return None;
    }
    Some(tokens as f64 * 1000.0 / (total - first) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn event(data: serde_json::Value) -> Vec<u8> {
        format!("data: {data}\n\n").into_bytes()
    }

    #[test]
    fn first_content_ignores_initialization_and_keeps_first_observation() {
        let start = Instant::now();
        let mut timing = StreamTiming::new(start);
        for data in [
            serde_json::json!({"type":"message_start","message":{"content":[]}}),
            serde_json::json!({"type":"ping"}),
            serde_json::json!({"type":"content_block_start","content_block":{"type":"text","text":""}}),
            serde_json::json!({"type":"content_block_delta","delta":{"type":"text_delta","text":""}}),
            serde_json::json!({"type":"content_block_delta","delta":{"type":"signature_delta","signature":"sig"}}),
        ] {
            timing.observe_sse(&event(data), start + Duration::from_millis(100));
        }
        assert_eq!(timing.first_content_ms(), None);
        let text = event(
            serde_json::json!({"type":"content_block_delta","delta":{"type":"text_delta","text":"你好"}}),
        );
        timing.observe_sse(&text, start + Duration::from_millis(850));
        timing.observe_sse(&text, start + Duration::from_millis(1200));
        assert_eq!(timing.first_content_ms(), Some(850));
    }

    #[test]
    fn first_content_counts_thoughts_tools_and_initial_text() {
        for data in [
            serde_json::json!({"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hmm"}}),
            serde_json::json!({"type":"content_block_start","content_block":{"type":"tool_use","name":"search","input":{}}}),
            serde_json::json!({"type":"content_block_start","content_block":{"type":"server_tool_use","name":"web_search"}}),
            serde_json::json!({"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{}"}}),
            serde_json::json!({"type":"content_block_start","content_block":{"type":"text","text":"hi"}}),
        ] {
            let start = Instant::now();
            let mut timing = StreamTiming::new(start);
            timing.observe_sse(&event(data), start + Duration::from_millis(300));
            assert_eq!(timing.first_content_ms(), Some(300));
        }
    }

    #[test]
    fn first_content_is_relative_to_each_attempt_and_can_be_zero() {
        let first_start = Instant::now();
        let second_start = first_start + Duration::from_secs(10);
        let data = event(
            serde_json::json!({"type":"content_block_delta","delta":{"type":"text_delta","text":"x"}}),
        );
        let mut timing = StreamTiming::new(second_start);
        timing.observe_sse(&data, second_start);
        assert_eq!(timing.first_content_ms(), Some(0));
    }

    #[test]
    fn first_content_survives_a_later_stream_error() {
        let start = Instant::now();
        let mut timing = StreamTiming::new(start);
        timing.observe_sse(&event(serde_json::json!({"type":"content_block_delta","delta":{"type":"text_delta","text":"hi"}})), start + Duration::from_millis(200));
        timing.observe_sse(
            &event(serde_json::json!({"type":"error","error":{"message":"broken stream"}})),
            start + Duration::from_millis(300),
        );
        assert_eq!(timing.first_content_ms(), Some(200));
        assert_eq!(timing.error_message(), Some("broken stream"));
    }

    #[test]
    fn gemini_thought_usage_does_not_invent_output_tokens() {
        let mut timing = StreamTiming::new(Instant::now());
        timing.observe_gemini_frame(&serde_json::json!({
            "candidates":[{"finishReason":"STOP"}], "usageMetadata":{"thoughtsTokenCount":20}
        }));
        timing.finish_stream();
        assert_eq!(timing.error_message(), None);
        assert!(!timing.has_output_usage());
    }

    #[test]
    fn interactions_partial_usage_and_eof_are_not_successful_completion() {
        let start = Instant::now();
        let mut timing = StreamTiming::new(start);
        timing.observe_sse(&event(serde_json::json!({"type":"content_block_delta","delta":{"type":"text_delta","text":"partial"}})), start + Duration::from_millis(200));
        timing.observe_interactions_frame(
            &serde_json::json!({"event_type":"step.stop","usage":{"total_output_tokens":10}}),
        );
        timing.finish_stream();
        assert!(timing.error_message().is_some());
        assert!(!timing.has_output_usage());
        assert_eq!(timing.first_content_ms(), Some(200));
    }

    #[test]
    fn gemini_and_chat_partial_usage_need_terminal_evidence() {
        let mut gemini = StreamTiming::new(Instant::now());
        gemini.observe_gemini_frame(
            &serde_json::json!({"usageMetadata":{"candidatesTokenCount":10}}),
        );
        gemini.finish_stream();
        assert!(gemini.error_message().is_some());
        assert!(!gemini.has_output_usage());
        let mut chat = StreamTiming::new(Instant::now());
        chat.observe_chat_frame("data: {\"choices\":[{\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}],\"usage\":{\"completion_tokens\":10}}");
        chat.finish_stream();
        assert!(chat.error_message().is_some());
        assert!(!chat.has_output_usage());
    }

    #[test]
    fn final_usage_after_chat_finish_and_gemini_finish_is_valid() {
        let mut chat = StreamTiming::new(Instant::now());
        chat.observe_chat_frame("data: {\"choices\":[{\"finish_reason\":\"stop\"}]}");
        chat.observe_chat_frame("data: {\"choices\":[],\"usage\":{\"completion_tokens\":10}}");
        chat.observe_chat_frame("data: [DONE]");
        chat.finish_stream();
        assert!(chat.has_output_usage());
        assert_eq!(chat.error_message(), None);
        let mut gemini = StreamTiming::new(Instant::now());
        gemini.observe_gemini_frame(&serde_json::json!({"candidates":[{"finishReason":"STOP"}]}));
        gemini
            .observe_gemini_frame(&serde_json::json!({"usageMetadata":{"candidatesTokenCount":0}}));
        gemini.finish_stream();
        assert!(gemini.has_output_usage());
        assert_eq!(gemini.error_message(), None);
    }

    #[test]
    fn synthetic_interactions_tail_cannot_turn_partial_stream_into_success() {
        use crate::proxy::transform::gemini_interactions::InteractionsSseConverter;
        let start = Instant::now();
        let mut timing = StreamTiming::new(start);
        let mut converter = InteractionsSseConverter::new_with_extras("gemini", true);
        for frame in [
            serde_json::json!({"event_type":"interaction.created","interaction":{"model":"gemini"}}),
            serde_json::json!({"event_type":"step.start","index":0,"step":{"type":"text","text":""}}),
            serde_json::json!({"event_type":"step.delta","index":0,"delta":{"type":"text","text":"partial"}}),
            serde_json::json!({"event_type":"step.stop","index":0,"usage":{"total_output_tokens":10}}),
        ] {
            timing.observe_interactions_frame(&frame);
            for event in converter.feed(&frame) {
                timing.observe_sse(&event.to_sse_bytes(), start + Duration::from_millis(200));
            }
        }
        timing.finish_stream();
        for event in converter.finalize() {
            timing.observe_sse(&event.to_sse_bytes(), start + Duration::from_millis(500));
        }
        assert_eq!(timing.first_content_ms(), Some(200));
        assert!(timing.error_message().is_some());
        assert!(!timing.has_output_usage());
    }

    #[test]
    fn gemini_converter_default_zero_remains_unknown_without_candidate_usage() {
        use crate::proxy::transform::gemini::GeminiSseConverter;
        let start = Instant::now();
        let mut timing = StreamTiming::new(start);
        let mut converter = GeminiSseConverter::new_with_extras("gemini", true);
        let frame = serde_json::json!({"candidates":[{"content":{"parts":[{"text":"thought","thought":true}]},"finishReason":"STOP"}],"usageMetadata":{"thoughtsTokenCount":20}});
        timing.observe_gemini_frame(&frame);
        for event in converter.feed(&frame) {
            timing.observe_sse(&event.to_sse_bytes(), start + Duration::from_millis(200));
        }
        timing.finish_stream();
        assert_eq!(timing.first_content_ms(), Some(200));
        assert_eq!(converter.usage()["output_tokens"], 0);
        assert!(!timing.has_output_usage());
        assert_eq!(timing.error_message(), None);
    }

    #[test]
    fn gemini_last_usage_snapshot_must_still_contain_output_count() {
        let mut timing = StreamTiming::new(Instant::now());
        timing.observe_gemini_frame(&serde_json::json!({"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"candidatesTokenCount":10}}));
        assert!(timing.has_output_usage());
        timing.observe_gemini_frame(&serde_json::json!({"usageMetadata":{"promptTokenCount":20}}));
        assert!(!timing.has_output_usage());
    }

    #[tokio::test]
    async fn delayed_split_http_stream_counts_header_wait_but_not_initial_events_as_content() {
        use crate::proxy::sse_framing::find_sse_frame_boundary;
        use bytes::BytesMut;
        use futures::StreamExt;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            socket.read(&mut request).await.unwrap();
            tokio::time::sleep(Duration::from_millis(25)).await;
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n").await.unwrap();
            socket.write_all(b"event: message_start\r\ndata: {\"type\":\"message_start\",\"message\":{\"content\":[]}}\r\n\r\n").await.unwrap();
            tokio::time::sleep(Duration::from_millis(40)).await;
            socket.write_all(b"event: content_block_delta\r\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"").await.unwrap();
            tokio::time::sleep(Duration::from_millis(40)).await;
            socket
                .write_all("你好\"}}\r\n\r\n".as_bytes())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(25)).await;
            socket
                .write_all(b"event: message_stop\r\ndata: {\"type\":\"message_stop\"}\r\n\r\n")
                .await
                .unwrap();
        });
        let start = Instant::now();
        let mut timing = StreamTiming::new(start);
        let response = reqwest::Client::new()
            .get(format!("http://{addr}/"))
            .send()
            .await
            .unwrap();
        let mut stream = response.bytes_stream();
        let mut buffer = BytesMut::new();
        let mut initial_seen = false;
        while let Some(chunk) = stream.next().await {
            buffer.extend_from_slice(&chunk.unwrap());
            while let Some((idx, sep)) = find_sse_frame_boundary(&buffer) {
                let frame = buffer.split_to(idx + sep);
                timing.observe_sse(&frame, Instant::now());
                if !initial_seen {
                    assert_eq!(timing.first_content_ms(), None);
                    initial_seen = true;
                }
            }
        }
        timing.finish_stream();
        server.await.unwrap();
        assert!(initial_seen);
        assert!(timing.first_content_ms().unwrap() >= 100);
        assert!(start.elapsed().as_millis() as u64 >= timing.first_content_ms().unwrap() + 20);
        assert_eq!(timing.error_message(), None);
    }

    #[test]
    fn speed_excludes_first_content_wait_and_rejects_unknown_or_invalid_data() {
        assert_eq!(
            output_tokens_per_second(true, "success", Some(200), Some(5200), Some(1200)),
            Some(50.0)
        );
        assert_eq!(
            output_tokens_per_second(true, "success", Some(0), Some(5200), Some(1200)),
            Some(0.0)
        );
        for (stream, status, tokens, total, first) in [
            (false, "success", Some(200), Some(5200), Some(1200)),
            (true, "error", Some(200), Some(5200), Some(1200)),
            (true, "timeout", Some(200), Some(5200), Some(1200)),
            (true, "success", None, Some(5200), Some(1200)),
            (true, "success", Some(200), None, Some(1200)),
            (true, "success", Some(200), Some(5200), None),
            (true, "success", Some(200), Some(1200), Some(1200)),
            (true, "success", Some(200), Some(1100), Some(1200)),
            (true, "success", Some(-1), Some(5200), Some(1200)),
            (true, "success", Some(200), Some(5200), Some(-1)),
        ] {
            assert_eq!(
                output_tokens_per_second(stream, status, tokens, total, first),
                None
            );
        }
    }
}
