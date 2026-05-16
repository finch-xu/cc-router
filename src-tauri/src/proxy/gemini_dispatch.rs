//! Google AI Studio Gemini 订阅的请求 dispatch.
//!
//! 与默认 Anthropic 透传分离, 避免污染主路径. 由 [`pipeline::dispatch`] 在检测到
//! `row.auth_type == GeminiApiKey` 时调用.
//!
//! 流程:
//! 1. 把订阅 row 的 URL 模板里的 `{model}` 替换为 slot 真实模型名, 拼上 `?alt=sse`
//! 2. [`transform::gemini::anthropic_to_gemini`] 翻译 body (Anthropic → Gemini)
//! 3. 注入 `x-goog-api-key: <api_key>` header (从订阅 row 读取, header_name 由 yaml 决定)
//! 4. **强制** streaming 上游 (端点是 `:streamGenerateContent` + `alt=sse`)
//! 5. 上游 SSE 帧 (无 event 名, 每帧 `data: {json}`) 用 [`GeminiSseConverter`] 翻译成 Anthropic SSE
//! 6. 客户端要非流式 → [`NonStreamingCollector`] 拼出 Anthropic Messages JSON
//!
//! 注意: 这一路完全不走 [`proxy::sse::stream_response`] (Anthropic 透传 + model 改写,
//! 与翻译路径冲突).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use bytes::{Bytes, BytesMut};
use chrono::Utc;
use futures::stream::BoxStream;
use futures::StreamExt;
use reqwest::header::{
    HeaderMap as ReqHeaderMap, HeaderName as ReqHeaderName, HeaderValue as ReqHeaderValue,
};
use serde_json::Value;
use sqlx::SqlitePool;
use tauri::AppHandle;
use tokio::sync::{mpsc, RwLock};
use tracing::warn;
use uuid::Uuid;

use crate::observability::events::{self, EventEntry, Severity};
use crate::observability::request_log::{RequestLogEntry, RequestStatus};
use crate::proxy::oauth_dispatch::{OAuthDispatchError, OAuthDispatchOk};
use crate::proxy::sse_framing::find_sse_frame_boundary;
use crate::proxy::transform::gemini::{
    anthropic_to_gemini, parse_gemini_sse_frame, GeminiExtras, GeminiSseConverter,
    NonStreamingCollector,
};
use crate::proxy::upstream;
use crate::subscription::model::SubscriptionRuntime;
use crate::subscription::state_machine;
use crate::virtual_model::VirtualModelName;

/// 准备并发送 Gemini 请求, 同时把响应翻译成 Anthropic 形态返回给客户端.
/// 不做 retry, 由调用方根据返回的 Result 决定.
///
/// `url_template` 是订阅 row 的完整 messages URL, 含 `{model}` 占位符. 例如
/// `https://generativelanguage.googleapis.com/v1beta/models/{model}:streamGenerateContent`.
/// 本函数会替换占位符 + 拼接 `?alt=sse`.
#[allow(clippy::too_many_arguments)]
pub async fn dispatch_gemini_attempt(
    http_client: &reqwest::Client,
    api_key: String,
    api_key_header_name: String,
    url_template: String,
    real_model: String,
    request_body: &Value,
    client_wants_streaming: bool,
    forward_headers: Vec<String>,
    client_headers: HeaderMap,
    required_headers: std::collections::BTreeMap<String, String>,
    extras: GeminiExtras,
) -> Result<OAuthDispatchOk, OAuthDispatchError> {
    if !url_template.contains("{model}") {
        return Err(OAuthDispatchError::Upstream {
            status: None,
            message: "Gemini messages_path 缺少 {model} 占位符".into(),
        });
    }
    let url_with_model = url_template.replace("{model}", &real_model);
    let url = if url_with_model.contains('?') {
        format!("{}&alt=sse", url_with_model)
    } else {
        format!("{}?alt=sse", url_with_model)
    };

    let emit_thoughts = extras.include_thoughts;
    let translated_body =
        anthropic_to_gemini(request_body, &extras).map_err(|e| OAuthDispatchError::Upstream {
            status: None,
            message: format!("body 翻译失败: {e}"),
        })?;
    let body_bytes = serde_json::to_vec(&translated_body).map_err(|e| {
        OAuthDispatchError::Upstream {
            status: None,
            message: format!("body 序列化失败: {e}"),
        }
    })?;

    let mut headers = ReqHeaderMap::new();
    let header_name = if api_key_header_name.is_empty() {
        "x-goog-api-key".to_string()
    } else {
        api_key_header_name
    };
    // api_key header 缺失会直接 401 且上游不留线索, 必须 fail-fast 而非 silent skip.
    let auth_name = ReqHeaderName::try_from(header_name.as_str()).map_err(|_| {
        OAuthDispatchError::Upstream {
            status: None,
            message: format!("auth header name {:?} 含非法字符", header_name),
        }
    })?;
    let auth_value = ReqHeaderValue::from_str(&api_key).map_err(|_| {
        OAuthDispatchError::Upstream {
            status: None,
            message: "API key 含非 ASCII 可见字符, 无法构造 header".into(),
        }
    })?;
    headers.insert(auth_name, auth_value);
    for (k, v) in required_headers {
        if let (Ok(name), Ok(value)) =
            (ReqHeaderName::try_from(k.as_str()), ReqHeaderValue::from_str(&v))
        {
            headers.insert(name, value);
        }
    }
    for fwd in forward_headers {
        if let Some(val) = client_headers.get(fwd.as_str()) {
            if let (Ok(name), Ok(value)) = (
                ReqHeaderName::try_from(fwd.as_str()),
                ReqHeaderValue::from_bytes(val.as_bytes()),
            ) {
                headers.insert(name, value);
            }
        }
    }
    headers.insert(
        reqwest::header::CONTENT_TYPE,
        ReqHeaderValue::from_static("application/json"),
    );
    headers.insert(
        reqwest::header::ACCEPT,
        ReqHeaderValue::from_static("text/event-stream"),
    );

    // 强制 streaming 上游, 因为 URL 已是 :streamGenerateContent?alt=sse.
    let send_result = upstream::send(http_client, &url, body_bytes, headers, true).await;
    let upstream_response = match send_result {
        Ok(r) => r,
        Err(upstream::UpstreamError::Reqwest(e)) => {
            return Err(OAuthDispatchError::Upstream {
                status: None,
                message: format!("网络错误: {e}"),
            });
        }
    };

    let (status, resp_headers, stream) = match upstream_response {
        upstream::UpstreamResponse::Streaming { status, headers, stream } => {
            (status, headers, stream)
        }
        upstream::UpstreamResponse::NonStreaming { status, body_text, .. } => {
            // Gemini 错误响应可能是 JSON {error: {...}}, body_text 包含完整错误
            return Err(OAuthDispatchError::Upstream {
                status: Some(status.as_u16()),
                message: body_text.unwrap_or_else(|| format!("HTTP {}", status.as_u16())),
            });
        }
    };

    if !status.is_success() {
        let mut buf = String::new();
        let mut s = stream;
        while let Some(c) = s.next().await {
            if let Ok(b) = c {
                if let Ok(t) = std::str::from_utf8(&b) {
                    buf.push_str(t);
                }
            }
        }
        return Err(OAuthDispatchError::Upstream {
            status: Some(status.as_u16()),
            message: buf,
        });
    }

    Ok(OAuthDispatchOk {
        upstream_status: status,
        upstream_headers: resp_headers,
        upstream_stream: stream,
        client_wants_streaming,
        transform_config: None,
        gemini_emit_thoughts: emit_thoughts,
    })
}

/// 把 Gemini 上游 SSE 流翻译成给客户端的最终响应 (流式或非流式).
/// 同时投递 RequestLogEntry / events / 状态机事件.
#[allow(clippy::too_many_arguments)]
pub fn finalize_gemini_response(
    ok: OAuthDispatchOk,
    vm_name: VirtualModelName,
    attempt_id: Uuid,
    sub_id: Uuid,
    provider_id: String,
    endpoint_id: String,
    real_model: String,
    display_name: String,
    retry_count: u32,
    log_tx: mpsc::Sender<RequestLogEntry>,
    event_log_tx: mpsc::Sender<EventEntry>,
    pool: SqlitePool,
    app: AppHandle,
    sub_rt: Arc<RwLock<SubscriptionRuntime>>,
) -> Response {
    let emit_thoughts = ok.gemini_emit_thoughts;
    if ok.client_wants_streaming {
        finalize_gemini_streaming(
            ok.upstream_stream,
            emit_thoughts,
            vm_name,
            attempt_id,
            sub_id,
            provider_id,
            endpoint_id,
            real_model,
            display_name,
            retry_count,
            log_tx,
            event_log_tx,
            pool,
            app,
            sub_rt,
        )
    } else {
        let fut = collect_gemini_to_json_response(
            ok.upstream_stream,
            emit_thoughts,
            vm_name,
            attempt_id,
            sub_id,
            provider_id,
            endpoint_id,
            real_model,
            display_name,
            retry_count,
            log_tx,
            event_log_tx,
            pool,
            app,
            sub_rt,
        );
        let stream = futures::stream::once(async move {
            let resp = fut.await;
            let (_parts, body) = resp.into_parts();
            let bytes = match axum::body::to_bytes(body, usize::MAX).await {
                Ok(b) => b,
                Err(e) => Bytes::from(format!("internal: {e}")),
            };
            Ok::<_, std::io::Error>(bytes)
        });
        let body = Body::from_stream(stream);
        let mut response = Response::new(body);
        *response.status_mut() = StatusCode::OK;
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        response
    }
}

#[allow(clippy::too_many_arguments)]
fn finalize_gemini_streaming(
    upstream_stream: BoxStream<'static, Result<Bytes, reqwest::Error>>,
    emit_thoughts: bool,
    vm_name: VirtualModelName,
    attempt_id: Uuid,
    sub_id: Uuid,
    provider_id: String,
    endpoint_id: String,
    real_model: String,
    display_name: String,
    retry_count: u32,
    log_tx: mpsc::Sender<RequestLogEntry>,
    event_log_tx: mpsc::Sender<EventEntry>,
    pool: SqlitePool,
    app: AppHandle,
    sub_rt: Arc<RwLock<SubscriptionRuntime>>,
) -> Response {
    let (client_tx, client_rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(64);

    tokio::spawn(async move {
        let start = std::time::Instant::now();
        let mut converter = GeminiSseConverter::new_with_extras(&real_model, emit_thoughts);
        let mut buffer = BytesMut::with_capacity(8 * 1024);

        let mut stream = upstream_stream;
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    warn!(?e, "Gemini upstream stream error");
                    let payload = serde_json::to_string(&serde_json::json!({"error": e.to_string()}))
                        .unwrap_or_else(|_| r#"{"error":"unknown"}"#.into());
                    let _ = client_tx
                        .send(Ok(Bytes::from(format!("event: error\ndata: {}\n\n", payload))))
                        .await;
                    break;
                }
            };
            buffer.extend_from_slice(&chunk);

            loop {
                let Some((idx, sep_len)) = find_sse_frame_boundary(&buffer) else {
                    break;
                };
                let frame_bytes = buffer.split_to(idx + sep_len);
                let frame_str = std::str::from_utf8(&frame_bytes[..frame_bytes.len() - sep_len])
                    .unwrap_or("");
                let Some(frame_json) = parse_gemini_sse_frame(frame_str) else {
                    continue;
                };
                let anth_events = converter.feed(&frame_json);
                if anth_events.is_empty() {
                    continue;
                }
                let mut buf = Vec::with_capacity(256);
                for evt in anth_events {
                    buf.extend_from_slice(&evt.to_sse_bytes());
                }
                if client_tx.send(Ok(Bytes::from(buf))).await.is_err() {
                    return;
                }
            }
        }

        // 兜底 message_delta + message_stop
        let tail = converter.finalize();
        if !tail.is_empty() {
            let mut buf = Vec::new();
            for evt in tail {
                buf.extend_from_slice(&evt.to_sse_bytes());
            }
            let _ = client_tx.send(Ok(Bytes::from(buf))).await;
        }

        // 状态机 + 日志
        let _ = state_machine::apply(
            &pool,
            &app,
            &event_log_tx,
            sub_rt.clone(),
            state_machine::Event::RequestSucceeded,
        )
        .await;

        let usage = converter.usage();
        let input_tokens = usage.get("input_tokens").and_then(|v| v.as_u64()).map(|v| v as u32);
        let output_tokens = usage.get("output_tokens").and_then(|v| v.as_u64()).map(|v| v as u32);
        let cache_read = usage
            .get("cache_read_input_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);

        let entry = RequestLogEntry {
            id: attempt_id,
            timestamp_ms: Utc::now().timestamp_millis(),
            virtual_model_name: vm_name,
            subscription_id: sub_id,
            provider_id,
            endpoint_id,
            real_model_name: real_model.clone(),
            response_model_name: {
                let m = converter.response_model();
                if m.is_empty() { None } else { Some(m.to_string()) }
            },
            is_streaming: true,
            status: RequestStatus::Success,
            http_status: Some(200),
            ttft_ms: None,
            total_latency_ms: Some(start.elapsed().as_millis() as u64),
            upstream_input_tokens: input_tokens,
            upstream_output_tokens: output_tokens,
            upstream_cache_creation: None,
            upstream_cache_read: cache_read,
            retry_count,
            error_message: None,
            upstream_response_body: None,
        };
        let _ = log_tx.try_send(entry);
        events::record_request(
            &event_log_tx,
            attempt_id,
            sub_id,
            Severity::Info,
            format!("{} · {} · Gemini · {}", vm_name.as_str(), display_name, real_model),
        );
    });

    let stream = futures::stream::unfold(client_rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    let body = Body::from_stream(stream);
    let mut response = Response::new(body);
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    response.headers_mut().insert(
        HeaderName::from_static("cache-control"),
        HeaderValue::from_static("no-cache"),
    );
    response
}

#[allow(clippy::too_many_arguments)]
async fn collect_gemini_to_json_response(
    upstream_stream: BoxStream<'static, Result<Bytes, reqwest::Error>>,
    emit_thoughts: bool,
    vm_name: VirtualModelName,
    attempt_id: Uuid,
    sub_id: Uuid,
    provider_id: String,
    endpoint_id: String,
    real_model: String,
    display_name: String,
    retry_count: u32,
    log_tx: mpsc::Sender<RequestLogEntry>,
    event_log_tx: mpsc::Sender<EventEntry>,
    pool: SqlitePool,
    app: AppHandle,
    sub_rt: Arc<RwLock<SubscriptionRuntime>>,
) -> Response {
    let start = std::time::Instant::now();
    let mut collector = NonStreamingCollector::new_with_extras(&real_model, emit_thoughts);
    let mut buffer = BytesMut::with_capacity(8 * 1024);

    let mut stream = upstream_stream;
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else { break };
        buffer.extend_from_slice(&chunk);
        loop {
            let Some((idx, sep_len)) = find_sse_frame_boundary(&buffer) else {
                break;
            };
            let frame_bytes = buffer.split_to(idx + sep_len);
            let frame_str =
                std::str::from_utf8(&frame_bytes[..frame_bytes.len() - sep_len]).unwrap_or("");
            if let Some(frame_json) = parse_gemini_sse_frame(frame_str) {
                collector.feed(&frame_json);
            }
        }
    }
    let final_msg = collector.finalize();

    let _ = state_machine::apply(
        &pool,
        &app,
        &event_log_tx,
        sub_rt.clone(),
        state_machine::Event::RequestSucceeded,
    )
    .await;

    let input_tokens = final_msg
        .get("usage")
        .and_then(|u| u.get("input_tokens"))
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let output_tokens = final_msg
        .get("usage")
        .and_then(|u| u.get("output_tokens"))
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let cache_read = final_msg
        .get("usage")
        .and_then(|u| u.get("cache_read_input_tokens"))
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);

    let entry = RequestLogEntry {
        id: attempt_id,
        timestamp_ms: Utc::now().timestamp_millis(),
        virtual_model_name: vm_name,
        subscription_id: sub_id,
        provider_id,
        endpoint_id,
        real_model_name: real_model.clone(),
        response_model_name: final_msg
            .get("model")
            .and_then(|v| v.as_str())
            .map(String::from),
        is_streaming: false,
        status: RequestStatus::Success,
        http_status: Some(200),
        ttft_ms: None,
        total_latency_ms: Some(start.elapsed().as_millis() as u64),
        upstream_input_tokens: input_tokens,
        upstream_output_tokens: output_tokens,
        upstream_cache_creation: None,
        upstream_cache_read: cache_read,
        retry_count,
        error_message: None,
        upstream_response_body: None,
    };
    let _ = log_tx.try_send(entry);
    events::record_request(
        &event_log_tx,
        attempt_id,
        sub_id,
        Severity::Info,
        format!("{} · {} · Gemini · {}", vm_name.as_str(), display_name, real_model),
    );

    let bytes = serde_json::to_vec(&final_msg).unwrap_or_default();
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

