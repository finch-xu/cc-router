//! SSE 流式代理（设计稿 §5.1 步骤 8 + §5.4）。
//!
//! 维护 buffer，按 `\n\n` 切事件。
//! - 第一个 `event: message_start`：解析 JSON → 改 `message.model` → 重序列化 → 写出
//! - `event: message_delta`：解析抽取 `output_tokens`，**原字节透传**
//! - 其他事件：原字节透传
//! - 解析失败：warning + 原字节透传（§9.7）

use std::time::Instant;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use bytes::{Bytes, BytesMut};
use futures::stream::BoxStream;
use futures::{Stream, StreamExt};
use reqwest::header::HeaderMap as ReqHeaderMap;
use tokio::sync::mpsc;
use tracing::warn;
use uuid::Uuid;

use crate::observability::request_log::{RequestLogEntry, RequestStatus};
use crate::virtual_model::VirtualModelName;

#[allow(clippy::too_many_arguments)]
pub fn stream_response(
    upstream_headers: ReqHeaderMap,
    upstream_stream: BoxStream<'static, Result<Bytes, reqwest::Error>>,
    vm_name: VirtualModelName,
    request_id: Uuid,
    subscription_id: Uuid,
    provider_id: String,
    endpoint_id: String,
    real_model: String,
    retry_count: u32,
    start: Instant,
    log_tx: mpsc::Sender<RequestLogEntry>,
) -> Response {
    let (client_tx, client_rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(64);

    // fallback 模式下不改写 message.model；传 None 表示透传
    let virtual_name_override: Option<String> = if vm_name.is_fallback() {
        None
    } else {
        Some(vm_name.as_str().to_string())
    };

    tokio::spawn(async move {
        let mut upstream = upstream_stream;
        let mut buffer = BytesMut::with_capacity(8 * 1024);
        let mut wrote_any_event = false;
        let mut first_byte_at: Option<Instant> = None;
        let mut input_tokens: Option<u32> = None;
        let mut output_tokens: Option<u32> = None;
        let mut cache_creation: Option<u32> = None;
        let mut cache_read: Option<u32> = None;
        let mut had_error = false;

        while let Some(chunk) = upstream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    warn!(?e, "upstream stream error");
                    had_error = true;
                    if !wrote_any_event {
                        let _ = client_tx
                            .send(Ok(Bytes::from(format_error_event(&e.to_string()))))
                            .await;
                    } else {
                        let _ = client_tx
                            .send(Ok(Bytes::from(format_error_event(&e.to_string()))))
                            .await;
                        let _ = client_tx.send(Ok(Bytes::from_static(b"data: [DONE]\n\n"))).await;
                    }
                    break;
                }
            };
            if first_byte_at.is_none() {
                first_byte_at = Some(Instant::now());
            }
            buffer.extend_from_slice(&chunk);

            // 尝试按 "\n\n" 切出完整事件
            while let Some(pos) = find_sequence(&buffer, b"\n\n") {
                let event_bytes = buffer.split_to(pos + 2);
                let (processed, parsed_meta) = process_event(
                    &event_bytes,
                    virtual_name_override.as_deref(),
                    &real_model,
                );

                if let Some(meta) = parsed_meta {
                    if let Some(v) = meta.input_tokens {
                        input_tokens = Some(v);
                    }
                    if let Some(v) = meta.output_tokens {
                        output_tokens = Some(v);
                    }
                    if let Some(v) = meta.cache_creation {
                        cache_creation = Some(v);
                    }
                    if let Some(v) = meta.cache_read {
                        cache_read = Some(v);
                    }
                }

                if let Err(e) = client_tx.send(Ok(processed)).await {
                    warn!(?e, "client 接收方已关闭");
                    break;
                }
                wrote_any_event = true;
            }
        }

        // 缓冲区残余
        if !buffer.is_empty() {
            let _ = client_tx.send(Ok(buffer.freeze())).await;
        }

        // 日志
        let total_ms = start.elapsed().as_millis() as u64;
        let ttft_ms = first_byte_at.map(|t| t.duration_since(start).as_millis() as u64);
        let entry = RequestLogEntry {
            id: request_id,
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            virtual_model_name: vm_name,
            subscription_id,
            provider_id,
            endpoint_id,
            real_model_name: real_model,
            is_streaming: true,
            status: if had_error {
                RequestStatus::Error
            } else {
                RequestStatus::Success
            },
            http_status: Some(200),
            ttft_ms,
            total_latency_ms: Some(total_ms),
            upstream_input_tokens: input_tokens,
            upstream_output_tokens: output_tokens,
            upstream_cache_creation: cache_creation,
            upstream_cache_read: cache_read,
            retry_count,
            error_message: None,
        };
        let _ = log_tx.try_send(entry);
    });

    let body_stream = stream_from_receiver(client_rx);
    let mut response = Response::new(Body::from_stream(body_stream));
    *response.status_mut() = StatusCode::OK;

    let headers = response.headers_mut();
    copy_safe_headers(&upstream_headers, headers);
    if !headers.contains_key(axum::http::header::CONTENT_TYPE) {
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
    }

    response
}

struct ParsedMeta {
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    cache_creation: Option<u32>,
    cache_read: Option<u32>,
}

/// 对单个 SSE 事件（以 `\n\n` 结尾）做改写并提取 tokens。
/// `virtual_name_override` 为 None 时不改写 message.model（fallback 透传模式）。
fn process_event(
    raw: &[u8],
    virtual_name_override: Option<&str>,
    _real_model: &str,
) -> (Bytes, Option<ParsedMeta>) {
    let text = match std::str::from_utf8(raw) {
        Ok(s) => s,
        Err(_) => return (Bytes::copy_from_slice(raw), None),
    };

    let is_message_start = text.contains("event: message_start");
    let is_message_delta = text.contains("event: message_delta");
    if !is_message_start && !is_message_delta {
        return (Bytes::copy_from_slice(raw), None);
    }

    // 寻找 data: 行
    let mut data_line_start: Option<usize> = None;
    let mut data_line_end: Option<usize> = None;
    let mut cursor = 0usize;
    for line in text.split_inclusive('\n') {
        if line.starts_with("data: ") || line.starts_with("data:") {
            data_line_start = Some(cursor);
            data_line_end = Some(cursor + line.len());
            break;
        }
        cursor += line.len();
    }

    let Some((start, end)) = data_line_start.zip(data_line_end) else {
        return (Bytes::copy_from_slice(raw), None);
    };

    let data_line = &text[start..end];
    let json_str = data_line
        .trim_start_matches("data:")
        .trim_start()
        .trim_end_matches('\n')
        .trim_end_matches('\r');

    let mut parsed: serde_json::Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(e) => {
            warn!(?e, "SSE data JSON 解析失败, 原字节透传");
            return (Bytes::copy_from_slice(raw), None);
        }
    };

    let mut meta = ParsedMeta {
        input_tokens: None,
        output_tokens: None,
        cache_creation: None,
        cache_read: None,
    };

    if is_message_start {
        // 提取 usage（无论是否改写 model 都需要记录日志）
        if let Some(message) = parsed.get("message") {
            if let Some(usage) = message.get("usage") {
                meta.input_tokens = usage.get("input_tokens").and_then(|v| v.as_u64()).map(|v| v as u32);
                meta.cache_creation = usage
                    .get("cache_creation_input_tokens")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32);
                meta.cache_read = usage
                    .get("cache_read_input_tokens")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32);
            }
        }

        // fallback 模式：不改写 model，原字节透传
        let Some(virtual_name) = virtual_name_override else {
            return (Bytes::copy_from_slice(raw), Some(meta));
        };

        if let Some(message) = parsed.get_mut("message") {
            if message.get("model").is_some() {
                message["model"] = serde_json::Value::String(virtual_name.to_string());
            }
        }

        let new_json = match serde_json::to_string(&parsed) {
            Ok(s) => s,
            Err(e) => {
                warn!(?e, "SSE 事件重序列化失败, 原字节透传");
                return (Bytes::copy_from_slice(raw), None);
            }
        };
        let mut rebuilt = String::with_capacity(raw.len());
        rebuilt.push_str(&text[..start]);
        rebuilt.push_str("data: ");
        rebuilt.push_str(&new_json);
        rebuilt.push('\n');
        rebuilt.push_str(&text[end..]);
        (Bytes::from(rebuilt), Some(meta))
    } else {
        // message_delta: 只提取 usage，原字节透传
        if let Some(usage) = parsed.get("usage") {
            meta.output_tokens = usage
                .get("output_tokens")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);
        }
        (Bytes::copy_from_slice(raw), Some(meta))
    }
}

fn find_sequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

fn format_error_event(msg: &str) -> String {
    let payload = serde_json::json!({
        "type": "error",
        "error": { "type": "upstream_error", "message": msg }
    });
    format!("event: error\ndata: {}\n\n", payload)
}

fn copy_safe_headers(from: &ReqHeaderMap, to: &mut HeaderMap) {
    const FORWARD: &[&str] = &[
        "content-type",
        "cache-control",
        "transfer-encoding",
    ];
    for name in FORWARD {
        if let Some(v) = from.get(*name) {
            if let (Ok(n), Ok(val)) = (
                HeaderName::try_from(*name),
                HeaderValue::from_bytes(v.as_bytes()),
            ) {
                to.insert(n, val);
            }
        }
    }
}

fn stream_from_receiver(
    rx: mpsc::Receiver<Result<Bytes, std::io::Error>>,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    tokio_stream_adapter(rx)
}

fn tokio_stream_adapter(
    rx: mpsc::Receiver<Result<Bytes, std::io::Error>>,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    futures::stream::unfold(rx, |mut rx| async move {
        match rx.recv().await {
            Some(item) => Some((item, rx)),
            None => None,
        }
    })
}
