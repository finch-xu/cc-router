//! Google Gemini **Interactions API** 订阅的请求 dispatch.
//!
//! 镜像 [`super::gemini_dispatch`] (旧 generateContent), 但走 Interactions 协议:
//! 1. URL = 订阅 row 的 messages URL (`{base_url}/v1beta/interactions`), **不含 `{model}` 占位符**
//!    (model 在 body 里), 直接拼 `?alt=sse`
//! 2. [`transform::gemini_interactions::anthropic_to_interactions`] 翻译 body, model 写进 body
//! 3. 注入 `x-goog-api-key: <api_key>` header
//! 4. **强制** streaming 上游 (`?alt=sse`)
//! 5. 上游标准 SSE (`event: X\ndata: {...}`) 用 [`InteractionsSseConverter`] 翻成 Anthropic SSE
//! 6. 客户端要非流式 → [`InteractionsNonStreamingCollector`] 拼出 Anthropic Messages JSON
//!
//! 与 [`super::gemini_dispatch`] 唯一结构差异: 无 `{model}` 替换 + 用 Interactions 翻译/收集器。

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
use crate::proxy::client_fingerprint::ClientContext;
use crate::proxy::oauth_dispatch::{OAuthDispatchError, OAuthDispatchOk};
use crate::proxy::sse_framing::find_sse_frame_boundary;
use crate::proxy::transform::gemini_interactions::{
    anthropic_to_interactions, parse_interactions_sse_frame, InteractionsExtras,
    InteractionsNonStreamingCollector, InteractionsSseConverter,
};
use crate::proxy::upstream;
use crate::subscription::model::SubscriptionRuntime;
use crate::subscription::state_machine;
use crate::virtual_model::VirtualModelName;

/// 准备并发送 Gemini Interactions 请求, 把响应翻译成 Anthropic 形态返回。不做 retry。
///
/// `url` 是订阅 row 的完整 messages URL (`{base_url}/v1beta/interactions`), **无 `{model}` 占位符**。
/// `real_model` 写进翻译后 body 的 `model` 字段。
#[allow(clippy::too_many_arguments)]
pub async fn dispatch_gemini_interactions_attempt(
    http_client: &reqwest::Client,
    api_key: String,
    api_key_header_name: String,
    url: String,
    real_model: String,
    request_body: &Value,
    client_wants_streaming: bool,
    forward_headers: Vec<String>,
    client_headers: HeaderMap,
    required_headers: std::collections::BTreeMap<String, String>,
    extras: InteractionsExtras,
) -> Result<OAuthDispatchOk, OAuthDispatchError> {
    // Interactions 的 model 在 body, URL 固定 — 只需拼 ?alt=sse 强制流式。
    let url = if url.contains('?') {
        format!("{}&alt=sse", url)
    } else {
        format!("{}?alt=sse", url)
    };

    let emit_thoughts = extras.include_thoughts;
    let translated_body = anthropic_to_interactions(request_body, &real_model, &extras).map_err(
        |e| OAuthDispatchError::Upstream {
            status: None,
            message: format!("body 翻译失败: {e}"),
        },
    )?;
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

/// 把 Interactions 上游 SSE 流翻译成给客户端的最终响应 (流式或非流式)。
#[allow(clippy::too_many_arguments)]
pub fn finalize_gemini_interactions_response(
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
    ctx: ClientContext,
) -> Response {
    let emit_thoughts = ok.gemini_emit_thoughts;
    if ok.client_wants_streaming {
        finalize_streaming(
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
            ctx,
        )
    } else {
        let fut = collect_to_json_response(
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
            ctx,
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
fn finalize_streaming(
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
    ctx: ClientContext,
) -> Response {
    let (client_tx, client_rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(64);

    tokio::spawn(async move {
        let start = std::time::Instant::now();
        let mut converter = InteractionsSseConverter::new_with_extras(&real_model, emit_thoughts);
        let mut buffer = BytesMut::with_capacity(8 * 1024);

        let mut stream = upstream_stream;
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    warn!(?e, "Gemini Interactions upstream stream error");
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
                let Some(frame_json) = parse_interactions_sse_frame(frame_str) else {
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
            client_tool: ctx.info.tool,
            client_user_agent: ctx.info.user_agent.clone(),
            client_version: ctx.info.version.clone(),
            client_ip: ctx.ip.clone(),
            entry_kind: Some(ctx.entry_kind.as_str()),
            downstream_http_version: ctx.http_version.clone(),
        };
        let _ = log_tx.try_send(entry);
        events::record_request(
            &event_log_tx,
            attempt_id,
            sub_id,
            Severity::Info,
            format!("{} · {} · Gemini Interactions · {}", vm_name.as_str(), display_name, real_model),
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
async fn collect_to_json_response(
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
    ctx: ClientContext,
) -> Response {
    let start = std::time::Instant::now();
    let mut collector = InteractionsNonStreamingCollector::new_with_extras(&real_model, emit_thoughts);
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
            if let Some(frame_json) = parse_interactions_sse_frame(frame_str) {
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
        client_tool: ctx.info.tool,
        client_user_agent: ctx.info.user_agent.clone(),
        client_version: ctx.info.version.clone(),
        client_ip: ctx.ip.clone(),
        entry_kind: Some(ctx.entry_kind.as_str()),
        downstream_http_version: ctx.http_version.clone(),
    };
    let _ = log_tx.try_send(entry);
    events::record_request(
        &event_log_tx,
        attempt_id,
        sub_id,
        Severity::Info,
        format!("{} · {} · Gemini Interactions · {}", vm_name.as_str(), display_name, real_model),
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
