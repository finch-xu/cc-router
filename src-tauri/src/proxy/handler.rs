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

use crate::proxy::client_fingerprint::{self, ClientContext, RequestEntryKind};
use crate::proxy::extractors::{format_http_version, HttpVersion};
use crate::proxy::pipeline;
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
    };

    info!(
        %model,
        is_streaming,
        client_tool = ?ctx.info.tool,
        client_ip = ?ctx.ip,
        http_version = ?ctx.http_version,
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
    };

    info!(
        %model,
        is_streaming,
        body_size,
        client_tool = ?ctx.info.tool,
        client_ip = ?ctx.ip,
        http_version = ?ctx.http_version,
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
        translate_sse_to_responses(status, axum_body)
    } else {
        translate_json_to_responses(status, axum_body).await
    }
}

/// 把 pipeline 返回的 Anthropic SSE body 翻译成 OpenAI Responses SSE 流, 重新拼成 Response.
/// 仿 [`crate::proxy::sse::stream_response`] 的 mpsc + spawn 模式, 但翻译方向是 Anthropic → OpenAI.
///
/// 响应头策略: 与 sse::stream_response 对齐 — 只设 content-type=text/event-stream,
/// **不设 cache-control / transfer-encoding** (让 axum 自动管 chunked encoding, 避免在
/// HTTPS+rustls 路径上跟底层冲突触发 IncompleteMessage)。
fn translate_sse_to_responses(status: StatusCode, body: Body) -> Response {
    let (client_tx, client_rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(64);
    let mut data_stream = body.into_data_stream();

    tokio::spawn(async move {
        let mut converter = AnthropicToResponsesSseConverter::new();
        let mut buffer: Vec<u8> = Vec::with_capacity(8 * 1024);
        let mut frames_emitted: u64 = 0;
        let mut events_parsed: u64 = 0;
        let mut upstream_chunks: u64 = 0;
        let mut early_break = false;
        while let Some(chunk) = data_stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    warn!(?e, frames_emitted, events_parsed, "pipeline SSE 流错误 (responses)");
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
                let frames = process_anthropic_sse_event(&event_bytes, &mut converter);
                events_parsed += 1;
                for frame in frames {
                    frames_emitted += 1;
                    if client_tx.send(Ok(Bytes::from(frame))).await.is_err() {
                        info!(
                            frames_emitted,
                            events_parsed,
                            upstream_chunks,
                            "客户端断开, SSE 翻译任务退出 (/v1/responses)"
                        );
                        return;
                    }
                }
            }
        }
        // 流结束兜底: 上游没发 message_stop 时, 至少补一个 response.completed 让客户端能收到流终结信号.
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
            "SSE 翻译任务结束 (/v1/responses)"
        );
        // OpenAI Responses SSE 末尾不发 [DONE], 客户端按 response.completed 终止.
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

/// 解析单个 Anthropic SSE 事件 (`event: <name>\ndata: <json>\n\n`), 喂转换器, 返回 OpenAI Responses 帧.
fn process_anthropic_sse_event(
    raw: &[u8],
    converter: &mut AnthropicToResponsesSseConverter,
) -> Vec<String> {
    let text = match std::str::from_utf8(raw) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut event_name: Option<&str> = None;
    let mut data_str: Option<&str> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("event:") {
            event_name = Some(rest.trim());
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_str = Some(rest.trim());
        }
    }
    let (Some(name), Some(data)) = (event_name, data_str) else {
        return Vec::new();
    };
    let json: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(e) => {
            warn!(?e, %name, "Anthropic SSE data JSON 解析失败 (responses 入口)");
            return Vec::new();
        }
    };
    converter.feed(name, &json)
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
        let (etype, msg) = match parsed.get("error") {
            Some(e) => {
                let t = e.get("type").and_then(|v| v.as_str()).unwrap_or("api_error").to_string();
                let m = e
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("upstream error")
                    .to_string();
                (t, m)
            }
            None => ("api_error".to_string(), "upstream error".to_string()),
        };
        return responses_error_response(status, &etype, &msg);
    }

    let translated = response_to_responses_json(&parsed);
    (status, Json(translated)).into_response()
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
        // OpenAI Responses 兼容入口别名 (v2.3+): 映射到 fable/opus/sonnet/haiku
        "gpt-5.6",
        "gpt-5.5",
        "gpt-5.4",
        "gpt-5.4-mini",
        "openai/gpt-5.6",
        "openai/gpt-5.5",
        "openai/gpt-5.4",
        "openai/gpt-5.4-mini",
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
