use std::net::SocketAddr;

use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::observability::body_dump::{BodyDumpEntry, BodyDumpKind};
use crate::proxy::client_fingerprint::{self, ClientContext, RequestEntryKind};
use crate::proxy::extractors::{format_http_version, HttpVersion};
use crate::proxy::pipeline;
use crate::proxy::session_key;
use crate::proxy::transform::chat_completions_inbound::{self as chat_inbound, AnthropicToChatSseConverter};
use crate::proxy::transform::responses_inbound::{
    request_to_anthropic, response_to_responses_json, AnthropicToResponsesSseConverter,
};
use crate::state::AppState;

pub async fn health() -> &'static str {
    "ok"
}

/// POST /v1/messages
/// 入口：把 Claude Code 的请求解析成 UnifiedRequest 后交给 pipeline。
pub async fn messages(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    HttpVersion(version): HttpVersion,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                &format!("JSON 解析失败: {e}"),
            );
        }
    };

    let model = match parsed.get("model").and_then(|v| v.as_str()) {
        Some(m) => m.to_string(),
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "缺少 model 字段",
            );
        }
    };

    let is_streaming = parsed
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // 识别一次客户端 (UA + stainless headers) + 记录 TCP 对端 IP + 入口端点 + 下游 HTTP 版本,
    // 沿 dispatch 链透传给所有 RequestLogEntry.
    let ctx = ClientContext {
        info: client_fingerprint::identify(&headers),
        ip: Some(peer.ip().to_string()),
        entry_kind: RequestEntryKind::Messages,
        http_version: Some(format_http_version(version)),
        session_key: session_key::extract(&headers, &parsed, RequestEntryKind::Messages),
    };

    info!(
        %model,
        is_streaming,
        client_tool = ?ctx.info.tool,
        client_ip = ?ctx.ip,
        http_version = ?ctx.http_version,
        session_key_source = ?ctx.session_key.as_deref().map(|k| &k[..k.find(':').unwrap_or(0)]),
        "proxy received request"
    );

    match pipeline::dispatch(&state, &model, parsed, headers, is_streaming, &ctx).await {
        Ok(resp) => resp,
        Err(e) => {
            error!(?e, "pipeline dispatch failed");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "api_error", &e.to_string())
        }
    }
}

pub fn error_body(kind: &str, message: &str) -> serde_json::Value {
    json!({
        "type": "error",
        "error": {
            "type": kind,
            "message": message,
        }
    })
}

pub fn error_response(status: StatusCode, kind: &str, message: &str) -> Response {
    (status, Json(error_body(kind, message))).into_response()
}

// ============================================================
// POST /v1/responses — OpenAI Responses 兼容入口 (v2.3+)
// ============================================================

/// OpenAI Responses 风格的错误响应:
/// `{"error": {"message": ..., "type": ..., "code": null}}`.
fn responses_error_response(status: StatusCode, type_: &str, message: &str) -> Response {
    let body = json!({
        "error": {
            "message": message,
            "type": type_,
            "code": Value::Null,
        }
    });
    (status, Json(body)).into_response()
}

/// POST /v1/responses
/// 入口翻译模式: 接收外部 agent 的 OpenAI Responses 请求, 内部翻译成 Anthropic Messages
/// 走现有 pipeline, 再把响应翻译回 OpenAI Responses 给客户端。pipeline 零改动, 所有
/// 上游 provider 路径 (9 家 Anthropic 透传 + codex/openai/gemini/kiro) 全部复用。
pub async fn responses(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    HttpVersion(version): HttpVersion,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let parsed: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return responses_error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                &format!("JSON 解析失败: {e}"),
            );
        }
    };

    let model = match parsed.get("model").and_then(|v| v.as_str()) {
        Some(m) => m.to_string(),
        None => {
            return responses_error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "缺少 model 字段",
            );
        }
    };

    let is_streaming = parsed
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let body_size = body.len();

    // OpenAI Responses → Anthropic Messages
    let anthropic_body = match request_to_anthropic(&parsed) {
        Ok(b) => b,
        Err(e) => {
            return responses_error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                &format!("请求翻译失败: {e}"),
            );
        }
    };

    let ctx = ClientContext {
        info: client_fingerprint::identify(&headers),
        ip: Some(peer.ip().to_string()),
        entry_kind: RequestEntryKind::Responses,
        http_version: Some(format_http_version(version)),
        // 用翻译前的原始 parsed: request_to_anthropic 会丢 prompt_cache_key.
        session_key: session_key::extract(&headers, &parsed, RequestEntryKind::Responses),
    };

    info!(
        %model,
        is_streaming,
        body_size,
        client_tool = ?ctx.info.tool,
        client_ip = ?ctx.ip,
        http_version = ?ctx.http_version,
        session_key_source = ?ctx.session_key.as_deref().map(|k| &k[..k.find(':').unwrap_or(0)]),
        "proxy received /v1/responses request"
    );

    // pipeline 内部 stream 字段已经是 Anthropic Messages 的, 但 is_streaming 入参用客户端原始意图.
    let upstream = match pipeline::dispatch(
        &state,
        &model,
        anthropic_body,
        headers,
        is_streaming,
        &ctx,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            error!(?e, "pipeline dispatch failed (responses)");
            return responses_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                &e.to_string(),
            );
        }
    };

    // pipeline 返回的 Response 已经是给客户端的 Anthropic 形式 (SSE 或 JSON).
    // 我们拆开拦截并翻译回 OpenAI Responses 形式.
    let (parts, axum_body) = upstream.into_parts();
    let status = parts.status;
    let upstream_content_type = parts
        .headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    let is_event_stream = upstream_content_type
        .as_deref()
        .map(|s| s.contains("text/event-stream"))
        .unwrap_or(false);

    info!(
        upstream_status = %status,
        upstream_content_type = ?upstream_content_type,
        is_event_stream,
        "pipeline 返回, 准备翻译响应给客户端 (/v1/responses)"
    );

    if is_event_stream {
        translate_sse(status, axum_body, AnthropicToResponsesSseConverter::new(), "/v1/responses")
    } else {
        translate_json_to_responses(status, axum_body).await
    }
}

/// 两个入站 converter (Responses / Chat Completions) 的公共接口, 让 SSE 中转代码只写一份.
trait InboundSseConverter: Send + 'static {
    fn feed(&mut self, event_name: &str, data: &Value) -> Vec<String>;
    fn finalize_if_needed(&mut self) -> Vec<String>;
}

impl InboundSseConverter for AnthropicToResponsesSseConverter {
    fn feed(&mut self, event_name: &str, data: &Value) -> Vec<String> {
        AnthropicToResponsesSseConverter::feed(self, event_name, data)
    }
    fn finalize_if_needed(&mut self) -> Vec<String> {
        AnthropicToResponsesSseConverter::finalize_if_needed(self)
    }
}

impl InboundSseConverter for AnthropicToChatSseConverter {
    fn feed(&mut self, event_name: &str, data: &Value) -> Vec<String> {
        AnthropicToChatSseConverter::feed(self, event_name, data)
    }
    fn finalize_if_needed(&mut self) -> Vec<String> {
        AnthropicToChatSseConverter::finalize_if_needed(self)
    }
}

/// 把 pipeline 返回的 Anthropic SSE body 经 `converter` 翻译成客户端协议的 SSE 流, 重新拼成 Response.
/// 仿 [`crate::proxy::sse::stream_response`] 的 mpsc + spawn 模式.
///
/// 响应头策略: 与 sse::stream_response 对齐 — 只设 content-type=text/event-stream,
/// **不设 cache-control / transfer-encoding** (让 axum 自动管 chunked encoding, 避免在
/// HTTPS+rustls 路径上跟底层冲突触发 IncompleteMessage)。
///
/// `label` 只用于日志 (如 "/v1/responses").
fn translate_sse<C: InboundSseConverter>(
    status: StatusCode,
    body: Body,
    mut converter: C,
    label: &'static str,
) -> Response {
    let (client_tx, client_rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(64);
    let mut data_stream = body.into_data_stream();

    tokio::spawn(async move {
        let mut buffer: Vec<u8> = Vec::with_capacity(8 * 1024);
        let mut frames_emitted: u64 = 0;
        let mut events_parsed: u64 = 0;
        let mut upstream_chunks: u64 = 0;
        let mut early_break = false;
        while let Some(chunk) = data_stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    warn!(?e, frames_emitted, events_parsed, label, "pipeline SSE 流错误");
                    early_break = true;
                    break;
                }
            };
            upstream_chunks += 1;
            buffer.extend_from_slice(&chunk);

            // 按 "\n\n" 切完整事件 (兼容 LF / CRLF)
            while let Some(pos) = find_double_newline(&buffer) {
                let drain_end = pos + double_newline_len(&buffer, pos);
                let event_bytes: Vec<u8> = buffer.drain(..drain_end).collect();
                events_parsed += 1;
                let frames = match parse_anthropic_sse_event(&event_bytes) {
                    Some((name, json)) => converter.feed(&name, &json),
                    None => Vec::new(),
                };
                for frame in frames {
                    frames_emitted += 1;
                    if client_tx.send(Ok(Bytes::from(frame))).await.is_err() {
                        info!(frames_emitted, events_parsed, upstream_chunks, label, "客户端断开, SSE 翻译任务退出");
                        return;
                    }
                }
            }
        }
        // 流结束兜底: 上游没发 message_stop 时, 至少补终结帧让客户端能收到流终结信号.
        let extra = converter.finalize_if_needed();
        let finalized = !extra.is_empty();
        for frame in extra {
            frames_emitted += 1;
            let _ = client_tx.send(Ok(Bytes::from(frame))).await;
        }
        info!(
            frames_emitted,
            events_parsed,
            upstream_chunks,
            buffer_residue = buffer.len(),
            early_break,
            finalized,
            label,
            "SSE 翻译任务结束"
        );
    });

    let body_stream = stream_from_receiver(client_rx);
    let mut response = Response::new(Body::from_stream(body_stream));
    *response.status_mut() = status;
    // 只设 content-type, 不动 cache-control / transfer-encoding — axum 自己会写 chunked.
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    response
}

/// 解析单个 Anthropic SSE 事件 (`event: <name>\ndata: <json>\n\n`) 为 (事件名, data JSON).
/// 非 UTF-8 / 缺 event 或 data 行 / JSON 非法 → None (JSON 非法时 warn).
fn parse_anthropic_sse_event(raw: &[u8]) -> Option<(String, Value)> {
    let text = std::str::from_utf8(raw).ok()?;
    let mut event_name: Option<&str> = None;
    let mut data_str: Option<&str> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("event:") {
            event_name = Some(rest.trim());
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_str = Some(rest.trim());
        }
    }
    let (name, data) = (event_name?, data_str?);
    match serde_json::from_str::<Value>(data) {
        Ok(json) => Some((name.to_string(), json)),
        Err(e) => {
            warn!(?e, %name, "Anthropic SSE data JSON 解析失败 (入站翻译)");
            None
        }
    }
}

/// 旧接口保留给 responses 入口的既有测试: 解析 + 喂 converter 一步到位.
/// handler 本身已改走 `translate_sse` 泛型路径, 不再直接调用它.
#[cfg(test)]
fn process_anthropic_sse_event(
    raw: &[u8],
    converter: &mut AnthropicToResponsesSseConverter,
) -> Vec<String> {
    match parse_anthropic_sse_event(raw) {
        Some((name, json)) => converter.feed(&name, &json),
        None => Vec::new(),
    }
}

/// 从 pipeline 返回的 Anthropic 错误体 `{"type":"error","error":{"type","message"}}` 取 (type, message);
/// 缺失时分别缺省 "api_error" / "upstream error". 两个入站入口共用.
fn upstream_error_parts(parsed: &Value) -> (String, String) {
    match parsed.get("error") {
        Some(e) => (
            e.get("type").and_then(|v| v.as_str()).unwrap_or("api_error").to_string(),
            e.get("message").and_then(|v| v.as_str()).unwrap_or("upstream error").to_string(),
        ),
        None => ("api_error".to_string(), "upstream error".to_string()),
    }
}

/// 把 pipeline 返回的 Anthropic JSON body 翻译成 OpenAI Responses JSON.
async fn translate_json_to_responses(status: StatusCode, body: Body) -> Response {
    let bytes = match axum::body::to_bytes(body, 16 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            error!(?e, "读取 pipeline JSON body 失败 (responses)");
            return responses_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                "读取上游响应失败",
            );
        }
    };
    let parsed: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => {
            error!(?e, "上游 JSON 解析失败 (responses)");
            return responses_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                "上游响应解析失败",
            );
        }
    };

    // 错误响应翻译: pipeline 可能返回 Anthropic error 形式 `{"type":"error","error":{"type","message"}}`.
    if !status.is_success() {
        let (etype, msg) = upstream_error_parts(&parsed);
        return responses_error_response(status, &etype, &msg);
    }

    let translated = response_to_responses_json(&parsed);
    (status, Json(translated)).into_response()
}

// ============================================================
// POST /v1/chat/completions — OpenAI Chat Completions 兼容入口 (v4.9+)
// ============================================================

fn chat_error_response(status: StatusCode, anthropic_type: &str, message: &str) -> Response {
    (status, Json(chat_inbound::chat_error_body(anthropic_type, message))).into_response()
}

/// POST /v1/chat/completions
/// 入口翻译模式: 接收 Open WebUI / Cherry Studio 等工具的 Chat Completions 请求, 内部翻译成
/// Anthropic Messages 走现有 pipeline, 再把响应翻译回 Chat Completions。pipeline 零改动。
pub async fn chat_completions(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    HttpVersion(version): HttpVersion,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let parsed: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return chat_error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                &format!("JSON 解析失败: {e}"),
            );
        }
    };

    let model = match parsed.get("model").and_then(|v| v.as_str()) {
        Some(m) => m.to_string(),
        None => {
            return chat_error_response(StatusCode::BAD_REQUEST, "invalid_request_error", "缺少 model 字段");
        }
    };

    let is_streaming = parsed.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);
    let body_size = body.len();

    // 调试模式: 翻译前的原始 Chat 请求也留一份, 便于排查翻译层问题 (spec §6).
    if state.settings.read().await.debug_mode {
        let _ = state.body_dump_tx.try_send(BodyDumpEntry::new(
            Uuid::new_v4(),
            BodyDumpKind::ClientChatCompletions,
            body.to_vec(),
        ));
    }

    // OpenAI Chat Completions → Anthropic Messages
    let anthropic_body = match chat_inbound::request_to_anthropic(&parsed) {
        Ok(b) => b,
        Err(e) => {
            return chat_error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                &format!("请求翻译失败: {e}"),
            );
        }
    };

    let ctx = ClientContext {
        info: client_fingerprint::identify(&headers),
        ip: Some(peer.ip().to_string()),
        entry_kind: RequestEntryKind::ChatCompletions,
        http_version: Some(format_http_version(version)),
        // 用翻译前的原始 parsed: 会话键分支读原始 `user` 字段.
        session_key: session_key::extract(&headers, &parsed, RequestEntryKind::ChatCompletions),
    };

    info!(
        %model,
        is_streaming,
        body_size,
        client_tool = ?ctx.info.tool,
        client_ip = ?ctx.ip,
        http_version = ?ctx.http_version,
        session_key_source = ?ctx.session_key.as_deref().map(|k| &k[..k.find(':').unwrap_or(0)]),
        "proxy received /v1/chat/completions request"
    );

    let upstream = match pipeline::dispatch(&state, &model, anthropic_body, headers, is_streaming, &ctx).await {
        Ok(r) => r,
        Err(e) => {
            error!(?e, "pipeline dispatch failed (chat_completions)");
            return chat_error_response(StatusCode::INTERNAL_SERVER_ERROR, "api_error", &e.to_string());
        }
    };

    let (parts, axum_body) = upstream.into_parts();
    let status = parts.status;
    let is_event_stream = parts
        .headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.contains("text/event-stream"))
        .unwrap_or(false);

    info!(upstream_status = %status, is_event_stream, "pipeline 返回, 准备翻译响应给客户端 (/v1/chat/completions)");

    if is_event_stream {
        translate_sse(status, axum_body, AnthropicToChatSseConverter::new(model), "/v1/chat/completions")
    } else {
        translate_json_to_chat(status, axum_body, &model).await
    }
}

/// 把 pipeline 返回的 Anthropic JSON body 翻译成 chat.completion JSON; 非 2xx 翻译成 OpenAI 错误体.
async fn translate_json_to_chat(status: StatusCode, body: Body, requested_model: &str) -> Response {
    let bytes = match axum::body::to_bytes(body, 16 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            error!(?e, "读取 pipeline JSON body 失败 (chat_completions)");
            return chat_error_response(StatusCode::INTERNAL_SERVER_ERROR, "api_error", "读取上游响应失败");
        }
    };
    let parsed: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => {
            error!(?e, "上游 JSON 解析失败 (chat_completions)");
            return chat_error_response(StatusCode::INTERNAL_SERVER_ERROR, "api_error", "上游响应解析失败");
        }
    };

    if !status.is_success() {
        let (etype, msg) = upstream_error_parts(&parsed);
        return chat_error_response(status, &etype, &msg);
    }

    (status, Json(chat_inbound::response_to_chat_json(&parsed, requested_model))).into_response()
}

fn find_double_newline(buf: &[u8]) -> Option<usize> {
    // 优先匹配 "\n\n", 再回退到 "\r\n\r\n"
    for i in 0..buf.len().saturating_sub(1) {
        if buf[i] == b'\n' && buf[i + 1] == b'\n' {
            return Some(i);
        }
    }
    for i in 0..buf.len().saturating_sub(3) {
        if &buf[i..i + 4] == b"\r\n\r\n" {
            return Some(i);
        }
    }
    None
}

fn double_newline_len(buf: &[u8], pos: usize) -> usize {
    if buf.len() >= pos + 4 && &buf[pos..pos + 4] == b"\r\n\r\n" {
        4
    } else {
        2
    }
}

fn stream_from_receiver(
    rx: mpsc::Receiver<Result<Bytes, std::io::Error>>,
) -> impl futures::Stream<Item = Result<Bytes, std::io::Error>> {
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
    fn find_double_newline_lf() {
        let buf = b"event: x\ndata: y\n\nrest".to_vec();
        let pos = find_double_newline(&buf).unwrap();
        // 起点应落在第一个 '\n' (索引 16, "data: y" 后面那个), pos+2 = 18 是 "rest" 的起点
        assert_eq!(pos, 16);
        assert_eq!(double_newline_len(&buf, pos), 2);
        assert_eq!(&buf[pos + 2..], b"rest");
    }

    #[test]
    fn find_double_newline_crlf() {
        let buf = b"event: x\r\ndata: y\r\n\r\nrest".to_vec();
        let pos = find_double_newline(&buf).unwrap();
        // 注意: 此 buf 不含 "\n\n", 应回退到 "\r\n\r\n"
        assert!(pos > 0);
        // 不必硬编码具体位置, 但 len 必须是 4
        assert_eq!(double_newline_len(&buf, pos), 4);
    }

    #[test]
    fn process_anthropic_sse_event_emits_openai_frames() {
        let mut conv = AnthropicToResponsesSseConverter::new();
        // 喂一个 message_start
        let frames = process_anthropic_sse_event(
            b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude-sonnet-4-6\"}}\n\n",
            &mut conv,
        );
        assert!(!frames.is_empty());
        assert!(frames.iter().any(|f| f.starts_with("event: response.created")));
        assert!(frames.iter().any(|f| f.starts_with("event: response.in_progress")));
    }

    #[test]
    fn process_anthropic_sse_event_full_flow() {
        let mut conv = AnthropicToResponsesSseConverter::new();
        let mut all_out: Vec<String> = Vec::new();
        let events: &[&[u8]] = &[
            b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude\"}}\n\n",
            b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
            b"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            b"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":3,\"output_tokens\":1}}\n\n",
            b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ];
        for raw in events {
            all_out.extend(process_anthropic_sse_event(raw, &mut conv));
        }
        // 应当包含完整的事件链
        let names: Vec<String> = all_out
            .iter()
            .filter_map(|f| f.lines().next().map(|l| l.trim_start_matches("event: ").to_string()))
            .collect();
        assert!(names.contains(&"response.created".to_string()));
        assert!(names.contains(&"response.output_item.added".to_string()));
        assert!(names.contains(&"response.output_text.delta".to_string()));
        assert!(names.contains(&"response.output_item.done".to_string()));
        assert!(names.contains(&"response.completed".to_string()));
    }

    #[test]
    fn process_anthropic_sse_event_handles_malformed_json() {
        let mut conv = AnthropicToResponsesSseConverter::new();
        // data 不是合法 JSON, 应 warn + 返回空 (不 panic)
        let frames =
            process_anthropic_sse_event(b"event: ping\ndata: not_json\n\n", &mut conv);
        assert!(frames.is_empty());
    }

    #[test]
    fn parse_anthropic_sse_event_extracts_name_and_json() {
        let (name, json) = parse_anthropic_sse_event(b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n").unwrap();
        assert_eq!(name, "message_stop");
        assert_eq!(json["type"], "message_stop");
        assert!(parse_anthropic_sse_event(b"data: {}\n\n").is_none(), "缺 event 行");
        assert!(parse_anthropic_sse_event(b"event: ping\ndata: not_json\n\n").is_none());
        assert!(parse_anthropic_sse_event(b"\xff\xfe").is_none(), "非 UTF-8");
    }

    #[test]
    fn chat_converter_full_flow_through_sse_parser() {
        use crate::proxy::transform::chat_completions_inbound::AnthropicToChatSseConverter;
        let mut conv = AnthropicToChatSseConverter::new("gpt-5.5".into());
        let events: &[&[u8]] = &[
            b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude\",\"usage\":{\"input_tokens\":3}}}\n\n",
            b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
            b"event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            b"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
            b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ];
        let mut all_out: Vec<String> = Vec::new();
        for raw in events {
            if let Some((name, json)) = parse_anthropic_sse_event(raw) {
                all_out.extend(conv.feed(&name, &json));
            }
        }
        assert_eq!(all_out.len(), 5, "role + content + finish + usage + DONE");
        assert!(all_out[1].contains("\"content\":\"Hello\""));
        assert!(all_out[2].contains("\"finish_reason\":\"stop\""));
        assert!(all_out[3].contains("\"total_tokens\":4"));
        assert_eq!(all_out[4], "data: [DONE]\n\n");
    }

    #[test]
    fn body_dump_kind_chat_suffix_distinct() {
        use crate::observability::body_dump::BodyDumpKind;
        // suffix 是私有的, 这里只确认 variant 存在且可比较; 文件名格式由 body_dump 自己的测试锁
        assert_ne!(BodyDumpKind::ClientChatCompletions, BodyDumpKind::Client);
    }
}

/// GET /v1/models
/// 返回 cc-router 对外暴露的固定模型清单, 无鉴权 (与 /health 同级在 auth_layer 直通).
///
/// schema 是 **Anthropic /v1/models + OpenAI /v1/models 超集**: 同时填两边的字段
/// (`type`+`object`, `display_name`+`owned_by`, `created_at`+`created`),
/// 两边 SDK 都按 `extra: allow` 忽略未知字段, 共用同一路径对客户端透明。
pub async fn models() -> Response {
    const MODEL_IDS: &[&str] = &[
        // Anthropic 风格虚拟模型名 + 版本别名 + anthropic/ 前缀变种
        "model-fable",
        "model-opus",
        "model-sonnet",
        "model-haiku",
        "claude-fable-5",
        "claude-opus-4-7",
        "claude-sonnet-4-6",
        "claude-haiku-4-5",
        "anthropic/claude-fable-5",
        "anthropic/claude-opus-4-7",
        "anthropic/claude-sonnet-4-6",
        "anthropic/claude-haiku-4-5",
        "anthropic/model-fable",
        "anthropic/model-opus",
        "anthropic/model-sonnet",
        "anthropic/model-haiku",
        // OpenAI Responses 兼容入口别名 (v2.3+): 映射到 fable/opus/sonnet/haiku;
        // sol/terra/luna 是 ChatGPT 新一代档位命名 (gpt-*-sol/terra/luna/mini 模糊匹配)
        "gpt-5.6",
        "gpt-5.5",
        "gpt-5.4",
        "gpt-5.4-mini",
        "gpt-5.6-sol",
        "gpt-5.6-terra",
        "gpt-5.6-luna",
        "openai/gpt-5.6",
        "openai/gpt-5.5",
        "openai/gpt-5.4",
        "openai/gpt-5.4-mini",
        "openai/gpt-5.6-sol",
        "openai/gpt-5.6-terra",
        "openai/gpt-5.6-luna",
    ];
    const CREATED_AT_ISO: &str = "2026-01-01T00:00:00Z"; // Anthropic 字段 (ISO 字符串)
    const CREATED_UNIX: i64 = 1_767_225_600; // OpenAI 字段 (Unix 秒, 同一时刻)

    let data: Vec<serde_json::Value> = MODEL_IDS
        .iter()
        .map(|id| {
            json!({
                // Anthropic 字段
                "type": "model",
                "id": id,
                "display_name": id,
                "created_at": CREATED_AT_ISO,
                // OpenAI 字段 (extra: allow → Anthropic SDK 忽略)
                "object": "model",
                "created": CREATED_UNIX,
                "owned_by": "cc-router",
            })
        })
        .collect();

    Json(json!({
        // OpenAI list wrapper (extra: allow → Anthropic SDK 忽略)
        "object": "list",
        // 通用字段
        "data": data,
        // Anthropic page wrapper
        "has_more": false,
        "first_id": MODEL_IDS.first(),
        "last_id": MODEL_IDS.last(),
    }))
    .into_response()
}
