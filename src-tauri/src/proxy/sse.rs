//! SSE 流式代理（设计稿 §5.1 步骤 8 + §5.4）。
//!
//! 维护 buffer，按 `\n\n` 切事件。
//! - 第一个 `event: message_start`：解析 JSON → 改 `message.model` → 重序列化 → 写出
//! - `event: message_delta`：解析抽取 `output_tokens`，**原字节透传**
//! - 其他事件：原字节透传
//! - 解析失败：warning + 原字节透传（§9.7）

use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use bytes::{Bytes, BytesMut};
use futures::stream::BoxStream;
use futures::{Stream, StreamExt};
use reqwest::header::HeaderMap as ReqHeaderMap;
use sqlx::SqlitePool;
use tauri::AppHandle;
use tokio::sync::{mpsc, RwLock};
use tracing::warn;
use uuid::Uuid;

/// 单个 SSE 事件最多读 16KB; 防止恶意/异常上游让 peek_first_event 一直累积.
const PEEK_LIMIT_BYTES: usize = 16 * 1024;

/// 智谱业务错误码: 5h/月度配额耗尽, 走 30min 长冷却.
pub const ZHIPU_ERR_QUOTA_EXHAUSTED: &str = "1308";
/// 智谱业务错误码: 短期 RPM/TPM 速率限制, 走 60s 短冷却.
pub const ZHIPU_ERR_RATE_LIMITED: &str = "1302";

use crate::observability::body_dump::{BodyDumpEntry, BodyDumpKind};
use crate::observability::events::{self, EventEntry, Severity};
use crate::observability::request_log::{RequestLogEntry, RequestStatus};
use crate::proxy::client_fingerprint::ClientContext;
use crate::subscription::model::SubscriptionRuntime;
use crate::subscription::state_machine;
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
    display_name: String,
    retry_count: u32,
    start: Instant,
    log_tx: mpsc::Sender<RequestLogEntry>,
    event_log_tx: mpsc::Sender<EventEntry>,
    pool: SqlitePool,
    app: AppHandle,
    sub_rt: Arc<RwLock<SubscriptionRuntime>>,
    // 调试模式下的 body dump channel. None 表示 debug_mode=false, 跳过累积与投递.
    body_dump_tx: Option<mpsc::Sender<BodyDumpEntry>>,
    // 流首 lookahead 已捕获的真实首字节时刻; 不传则按本函数收到第一个 chunk 的时刻计.
    // 单独传是为了避免 peek + state_machine apply 的耗时被错算到 TTFT 里.
    peek_first_byte_at: Option<Instant>,
    ctx: ClientContext,
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
        // 调试模式累积上游 raw SSE 字节流, 流结束时 try_send. None 时全程零成本.
        let mut raw_dump_buf: Option<BytesMut> = body_dump_tx
            .as_ref()
            .map(|_| BytesMut::with_capacity(8 * 1024));
        let mut wrote_any_event = false;
        let mut first_byte_at: Option<Instant> = peek_first_byte_at;
        let mut input_tokens: Option<u32> = None;
        let mut output_tokens: Option<u32> = None;
        let mut cache_creation: Option<u32> = None;
        let mut cache_read: Option<u32> = None;
        let mut response_model: Option<String> = None;
        let mut had_error = false;
        let mut error_text: Option<String> = None;

        while let Some(chunk) = upstream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    warn!(?e, "upstream stream error");
                    had_error = true;
                    error_text = Some(e.to_string());
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
            if let Some(dump) = raw_dump_buf.as_mut() {
                dump.extend_from_slice(&chunk);
            }
            buffer.extend_from_slice(&chunk);

            // 尝试按 "\n\n" 切出完整事件
            while let Some(pos) = find_sequence(&buffer, b"\n\n") {
                let event_bytes = buffer.split_to(pos + 2);
                let (processed, parsed_meta) =
                    process_event(&event_bytes, virtual_name_override.as_deref());

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
                    if let Some(v) = meta.response_model {
                        response_model = Some(v);
                    }
                }

                if let Err(e) = client_tx.send(Ok(processed)).await {
                    warn!(?e, "client 接收方已关闭, 终止流式任务以释放上游连接");
                    return;
                }
                wrote_any_event = true;
            }
        }

        // 缓冲区残余
        if !buffer.is_empty() {
            let _ = client_tx.send(Ok(buffer.freeze())).await;
        }

        // 调试模式: 流结束时(Ok 或 Err 都来到这里之后)投递累积的 raw SSE 字节流.
        if let (Some(tx), Some(buf)) = (body_dump_tx.as_ref(), raw_dump_buf.take()) {
            let _ = tx.try_send(BodyDumpEntry::new(
                request_id,
                BodyDumpKind::UpstreamResponse,
                buf.to_vec(),
            ));
        }

        // 客户端断开走 client_tx send error 提前 return, 不会到此, 避免 ctrl+c 触发 transient
        if had_error {
            let _ = state_machine::apply(
                &pool,
                &app,
                &event_log_tx,
                sub_rt.clone(),
                state_machine::Event::NetworkError,
            )
            .await;
        }

        // 日志
        let total_ms = start.elapsed().as_millis() as u64;
        let ttft_ms = first_byte_at.map(|t| t.duration_since(start).as_millis() as u64);
        let error_message = if had_error {
            Some(format!(
                "流式中断: {}",
                error_text.as_deref().unwrap_or("upstream stream error")
            ))
        } else {
            None
        };
        let entry = RequestLogEntry {
            id: request_id,
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            virtual_model_name: vm_name,
            subscription_id,
            provider_id: provider_id.clone(),
            endpoint_id,
            real_model_name: real_model.clone(),
            response_model_name: response_model.clone(),
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
            error_message: error_message.clone(),
            upstream_response_body: None,
            client_tool: ctx.info.tool,
            client_user_agent: ctx.info.user_agent.clone(),
            client_version: ctx.info.version.clone(),
            client_ip: ctx.ip.clone(),
        };
        let _ = log_tx.try_send(entry);

        // emit kind=request event 用于事件流时间线
        let event_severity = if had_error {
            Severity::Error
        } else {
            Severity::Info
        };
        let event_summary = if had_error {
            format!(
                "{} · {} · {} {}",
                vm_name.as_str(),
                display_name,
                real_model,
                error_message.as_deref().unwrap_or("流式中断")
            )
        } else {
            format!("{} · {} · {} (SSE)", vm_name.as_str(), display_name, real_model)
        };
        events::record_request(
            &event_log_tx,
            request_id,
            subscription_id,
            event_severity,
            event_summary,
        );
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
    /// message_start 事件里 message.model 的原值(改写前)
    response_model: Option<String>,
}

/// 对单个 SSE 事件（以 `\n\n` 结尾）做改写并提取 tokens。
/// - `virtual_name_override` 为 None 时不改写 message.model（fallback 透传模式）。
fn process_event(
    raw: &[u8],
    virtual_name_override: Option<&str>,
) -> (Bytes, Option<ParsedMeta>) {
    let text = match std::str::from_utf8(raw) {
        Ok(s) => s,
        Err(_) => return (Bytes::copy_from_slice(raw), None),
    };

    // SSE 规范允许 "event:" 冒号后空格可有可无。Anthropic 原生带空格，
    // 阿里云百炼等翻译层可能不带。按行解析名字更稳。
    let event_name = sse_event_name(text);
    let is_message_start = event_name == Some("message_start");
    let is_message_delta = event_name == Some("message_delta");

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
        response_model: None,
    };

    if is_message_start {
        // 提取 usage 与上游 model 原值(无论是否改写 model 都要记录日志)
        if let Some(message) = parsed.get("message") {
            if let Some(model) = message.get("model").and_then(|v| v.as_str()) {
                meta.response_model = Some(model.to_string());
            }
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

        (rebuild_event_with_data(text, start, end, &parsed, raw), Some(meta))
    } else {
        // message_delta: 提取 usage，原字节透传。
        // 阿里云百炼把最终的 input_tokens / cache_* 放在 message_delta.usage，
        // 而 Anthropic 原生只给 output_tokens——都读，读不到保持 None 不覆盖。
        if let Some(usage) = parsed.get("usage") {
            meta.output_tokens = usage
                .get("output_tokens")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);
            meta.input_tokens = usage
                .get("input_tokens")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);
            meta.cache_creation = usage
                .get("cache_creation_input_tokens")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);
            meta.cache_read = usage
                .get("cache_read_input_tokens")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);
        }
        (Bytes::copy_from_slice(raw), Some(meta))
    }
}

/// 用改写后的 JSON 替换原 SSE 事件里的 data 行, 保留其他行 (event: 等) 不动。
/// 失败时回退到原字节透传。
fn rebuild_event_with_data(
    text: &str,
    data_start: usize,
    data_end: usize,
    parsed: &serde_json::Value,
    raw_fallback: &[u8],
) -> Bytes {
    let new_json = match serde_json::to_string(parsed) {
        Ok(s) => s,
        Err(e) => {
            warn!(?e, "SSE 事件重序列化失败, 原字节透传");
            return Bytes::copy_from_slice(raw_fallback);
        }
    };
    let mut rebuilt = String::with_capacity(text.len() + new_json.len());
    rebuilt.push_str(&text[..data_start]);
    rebuilt.push_str("data: ");
    rebuilt.push_str(&new_json);
    rebuilt.push('\n');
    rebuilt.push_str(&text[data_end..]);
    Bytes::from(rebuilt)
}

/// 从 SSE 事件文本中按行找 `event:` 前缀，返回事件名。冒号后允许 0/1/多个空格。
fn sse_event_name(text: &str) -> Option<&str> {
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("event:") {
            let name = rest.trim();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
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

/// 流首 lookahead 的结果. 让 dispatcher 在把 200 返客户端前先看一眼第一个 SSE 事件,
/// 是 `event: error` 就触发 retry 切下家, 避免上游 200 + 业务错误的"假成功"被透传.
pub enum PeekResult {
    /// 首事件是 SSE error (如智谱 1302/1308). 调用方按 provider_id 判定冷却时长后触发状态机 + retry.
    UpstreamError {
        code: Option<String>,
        message: Option<String>,
        /// 已读到的原始字节, 给 debug_mode body_dump 用; 调用方决定是否落盘.
        raw_bytes: Bytes,
    },
    /// 首事件正常 (message_start 等). `stream` 已把首事件字节拼回流头部, 调用方直接当全流处理即可.
    Ok {
        stream: BoxStream<'static, Result<Bytes, reqwest::Error>>,
        /// 真实上游首字节到达时刻, 用于精准 TTFT 统计.
        first_byte_at: Instant,
    },
    /// 上游传输层错误, 一个完整事件都没读完.
    Network(reqwest::Error),
    /// 累积 PEEK_LIMIT_BYTES 仍没出现 \n\n, 或流提前结束 / UTF-8 解析失败.
    /// 调用方应当当成 NetworkError 处理 (retry).
    Malformed(Bytes),
}

impl std::fmt::Debug for PeekResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UpstreamError {
                code,
                message,
                raw_bytes,
            } => f
                .debug_struct("UpstreamError")
                .field("code", code)
                .field("message", message)
                .field("raw_bytes_len", &raw_bytes.len())
                .finish(),
            Self::Ok { .. } => f.debug_struct("Ok").finish_non_exhaustive(),
            Self::Network(e) => f.debug_tuple("Network").field(e).finish(),
            Self::Malformed(b) => f.debug_struct("Malformed").field("len", &b.len()).finish(),
        }
    }
}

/// 从上游 SSE 流中消费第一个完整事件 (以 `\n\n` 结尾), 解析后返回 PeekResult.
/// 限制单事件最大 16KB (`PEEK_LIMIT_BYTES`), 防止异常上游让 dispatcher 一直阻塞.
pub async fn peek_first_event(
    mut stream: BoxStream<'static, Result<Bytes, reqwest::Error>>,
) -> PeekResult {
    let mut buffer = BytesMut::with_capacity(8 * 1024);
    let mut first_byte_at: Option<Instant> = None;

    loop {
        if let Some(pos) = find_sequence(&buffer, b"\n\n") {
            let event_end = pos + 2;
            let event_bytes = &buffer[..event_end];
            let text = match std::str::from_utf8(event_bytes) {
                Ok(s) => s,
                Err(_) => return PeekResult::Malformed(buffer.freeze()),
            };
            if sse_event_name(text) == Some("error") {
                let (code, message) = parse_sse_error_data(text);
                return PeekResult::UpstreamError {
                    code,
                    message,
                    raw_bytes: buffer.freeze(),
                };
            }
            let head = futures::stream::once(async move {
                Ok::<_, reqwest::Error>(buffer.freeze())
            });
            return PeekResult::Ok {
                stream: Box::pin(head.chain(stream)),
                // first_byte_at must be Some here: 走到此必经过下面的 stream.next() 至少一次成功
                first_byte_at: first_byte_at.expect("first_byte_at set after first chunk"),
            };
        }

        if buffer.len() >= PEEK_LIMIT_BYTES {
            return PeekResult::Malformed(buffer.freeze());
        }

        match stream.next().await {
            Some(Ok(chunk)) => {
                if first_byte_at.is_none() {
                    first_byte_at = Some(Instant::now());
                }
                buffer.extend_from_slice(&chunk);
            }
            Some(Err(e)) => return PeekResult::Network(e),
            None => return PeekResult::Malformed(buffer.freeze()),
        }
    }
}

/// 从 SSE 事件文本里抠出 `data:` 行并解析 JSON, 提取 `error.code` / `error.message`.
fn parse_sse_error_data(text: &str) -> (Option<String>, Option<String>) {
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("data:") else {
            continue;
        };
        let json_str = rest.trim();
        let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) else {
            continue;
        };
        let err = v.get("error");
        let code = err
            .and_then(|e| e.get("code"))
            .and_then(|c| {
                c.as_str()
                    .map(String::from)
                    .or_else(|| c.as_u64().map(|n| n.to_string()))
            });
        let message = err
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .map(String::from);
        return (code, message);
    }
    (None, None)
}

/// 智谱 SSE error 是否为长期配额耗尽 (vs 短期速率限制).
/// 调用前必须确认 `provider_id == "zhipu"`, 否则其他 provider 的同名错误码会被误伤.
/// 后续接更多 provider 时再抽到 yaml `error_mapping` 通用 schema.
pub fn classify_zhipu_sse_error(code: Option<&str>, msg: Option<&str>) -> bool {
    if matches!(code, Some(c) if c == ZHIPU_ERR_QUOTA_EXHAUSTED) {
        return true;
    }
    if let Some(m) = msg {
        if m.contains("使用上限") || m.contains("5 小时") || m.contains("额度") {
            return true;
        }
    }
    false
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_event_name_handles_various_spacing() {
        assert_eq!(sse_event_name("event: message_start\ndata: {}\n"), Some("message_start"));
        assert_eq!(sse_event_name("event:message_start\ndata:{}\n"), Some("message_start"));
        assert_eq!(sse_event_name("event:   message_delta\ndata:{}\n"), Some("message_delta"));
        assert_eq!(sse_event_name("event: message_start\r\ndata: {}\r\n"), Some("message_start"));
        assert_eq!(sse_event_name("data: {}\n"), None);
    }

    /// 百炼风格：event 行冒号后无空格，message_delta.usage 携带全套 token。
    #[test]
    fn process_event_alibaba_style_message_start_no_space() {
        let raw = b"event:message_start\ndata:{\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"model\":\"qwen-flash\",\"role\":\"assistant\",\"type\":\"message\",\"content\":[],\"usage\":{\"input_tokens\":7,\"output_tokens\":0}}}\n\n";
        let (_bytes, meta) = process_event(raw, Some("model-sonnet"));
        let meta = meta.expect("alibaba message_start 应被识别");
        assert_eq!(meta.input_tokens, Some(7));
        // message_start 没有 cache_* 字段 → None
        assert_eq!(meta.cache_creation, None);
        assert_eq!(meta.cache_read, None);
        assert_eq!(meta.output_tokens, None);
    }

    #[test]
    fn process_event_alibaba_style_message_delta_full_usage() {
        let raw = b"event:message_delta\ndata:{\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":4,\"input_tokens\":15,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0}}\n\n";
        let (_bytes, meta) = process_event(raw, Some("model-sonnet"));
        let meta = meta.expect("alibaba message_delta 应被识别");
        assert_eq!(meta.input_tokens, Some(15));
        assert_eq!(meta.output_tokens, Some(4));
        assert_eq!(meta.cache_creation, Some(0));
        assert_eq!(meta.cache_read, Some(0));
    }

    /// Anthropic 原生风格：event 行带空格，message_delta 只有 output_tokens。
    #[test]
    fn process_event_anthropic_style_message_delta_output_only() {
        let raw = b"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":42}}\n\n";
        let (_bytes, meta) = process_event(raw, Some("model-sonnet"));
        let meta = meta.expect("anthropic message_delta 应被识别");
        assert_eq!(meta.output_tokens, Some(42));
        assert_eq!(meta.input_tokens, None);
        assert_eq!(meta.cache_creation, None);
        assert_eq!(meta.cache_read, None);
    }

    /// 非 message_start / message_delta 的事件直接透传，不产生 meta。
    #[test]
    fn process_event_other_events_passthrough() {
        let raw = b"event: ping\ndata: {\"type\":\"ping\"}\n\n";
        let (bytes, meta) = process_event(raw, Some("model-sonnet"));
        assert!(meta.is_none());
        assert_eq!(&bytes[..], raw);

        let raw2 = b"event:content_block_delta\ndata:{\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"},\"index\":0}\n\n";
        let (bytes2, meta2) = process_event(raw2, Some("model-sonnet"));
        assert!(meta2.is_none());
        assert_eq!(&bytes2[..], raw2);
    }

    /// content_block_start (含 thinking 块) 应原字节透传, 现已无方言重命名逻辑。
    #[test]
    fn process_event_thinking_block_start_passes_through() {
        let raw = b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"x\",\"signature\":\"\"}}\n\n";
        let (bytes, meta) = process_event(raw, Some("model-sonnet"));
        assert!(meta.is_none());
        assert_eq!(&bytes[..], raw, "content_block_start 应原字节透传");
    }

    fn into_box_stream(
        chunks: Vec<&'static [u8]>,
    ) -> BoxStream<'static, Result<Bytes, reqwest::Error>> {
        let iter = chunks.into_iter().map(|c| Ok(Bytes::from_static(c)));
        Box::pin(futures::stream::iter(iter))
    }

    #[tokio::test]
    async fn peek_zhipu_1308_returns_upstream_error() {
        let stream = into_box_stream(vec![
            b"event: error\ndata: {\"error\":{\"code\":\"1308\",\"message\":\"\xe5\xb7\xb2\xe8\xbe\xbe\xe5\x88\xb0 5 \xe5\xb0\x8f\xe6\x97\xb6\xe7\x9a\x84\xe4\xbd\xbf\xe7\x94\xa8\xe4\xb8\x8a\xe9\x99\x90\xe3\x80\x82\"}}\n\n",
            b"data: [DONE]\n\n",
        ]);
        let result = peek_first_event(stream).await;
        match result {
            PeekResult::UpstreamError { code, message, .. } => {
                assert_eq!(code.as_deref(), Some(ZHIPU_ERR_QUOTA_EXHAUSTED));
                assert!(classify_zhipu_sse_error(code.as_deref(), message.as_deref()));
            }
            other => panic!("expected UpstreamError, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn peek_zhipu_1302_returns_upstream_error_not_quota() {
        let stream = into_box_stream(vec![
            b"event: error\ndata: {\"error\":{\"code\":\"1302\",\"message\":\"rate limit\"}}\n\n",
        ]);
        let result = peek_first_event(stream).await;
        match result {
            PeekResult::UpstreamError { code, message, .. } => {
                assert_eq!(code.as_deref(), Some(ZHIPU_ERR_RATE_LIMITED));
                assert!(!classify_zhipu_sse_error(code.as_deref(), message.as_deref()));
            }
            other => panic!("expected UpstreamError, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn peek_normal_message_start_returns_ok() {
        let first_event: &[u8] = b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"model\":\"glm-4.6\",\"role\":\"assistant\",\"type\":\"message\",\"content\":[],\"usage\":{\"input_tokens\":7}}}\n\n";
        let second_event: &[u8] = b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"},\"index\":0}\n\n";
        let stream = into_box_stream(vec![first_event, second_event]);
        let result = peek_first_event(stream).await;
        match result {
            PeekResult::Ok { mut stream, first_byte_at: _ } => {
                // 拼回的流里应能依次读出首事件 + 后续事件全部字节
                let mut all = Vec::new();
                while let Some(chunk) = stream.next().await {
                    all.extend_from_slice(&chunk.expect("no transport error"));
                }
                let text = std::str::from_utf8(&all).unwrap();
                assert!(text.contains("message_start"));
                assert!(text.contains("content_block_delta"));
            }
            other => panic!("expected Ok, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn peek_chunked_first_event_accumulates_until_delim() {
        let stream = into_box_stream(vec![
            b"event: error\ndata: {\"error\":",
            b"{\"code\":\"1308\",",
            b"\"message\":\"x\"}}",
            b"\n\n",
        ]);
        let result = peek_first_event(stream).await;
        match result {
            PeekResult::UpstreamError { code, .. } => {
                assert_eq!(code.as_deref(), Some(ZHIPU_ERR_QUOTA_EXHAUSTED));
            }
            other => panic!("expected UpstreamError, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn peek_stream_ends_without_delim_is_malformed() {
        let stream = into_box_stream(vec![b"event: error\ndata: {\"error\":{\"code\":\"x\"}}"]);
        let result = peek_first_event(stream).await;
        assert!(matches!(result, PeekResult::Malformed(_)));
    }

    #[test]
    fn classify_zhipu_sse_error_matches_quota_messages() {
        assert!(classify_zhipu_sse_error(Some(ZHIPU_ERR_QUOTA_EXHAUSTED), None));
        assert!(classify_zhipu_sse_error(None, Some("已达到 5 小时的使用上限")));
        assert!(classify_zhipu_sse_error(None, Some("您的额度已用完")));
        assert!(!classify_zhipu_sse_error(Some(ZHIPU_ERR_RATE_LIMITED), Some("rate limit")));
        assert!(!classify_zhipu_sse_error(None, Some("network glitch")));
    }

    #[test]
    fn parse_sse_error_data_handles_string_and_int_code() {
        let (code, msg) =
            parse_sse_error_data("event: error\ndata: {\"error\":{\"code\":\"1308\",\"message\":\"hi\"}}\n");
        assert_eq!(code.as_deref(), Some("1308"));
        assert_eq!(msg.as_deref(), Some("hi"));

        // 部分 provider 把 code 写成数字
        let (code2, _) = parse_sse_error_data(
            "event: error\ndata: {\"error\":{\"code\":1308,\"message\":\"hi\"}}\n",
        );
        assert_eq!(code2.as_deref(), Some("1308"));
    }
}
