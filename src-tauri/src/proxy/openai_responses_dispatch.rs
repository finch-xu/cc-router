//! OpenAI `/v1/responses` API key 订阅的请求 dispatch.
//!
//! 与默认 Anthropic 透传分离, 避免污染主路径. 由 [`crate::proxy::pipeline::dispatch`] 在
//! 检测到 `row.auth_type == OpenaiResponsesApiKey` 时调用.
//!
//! 流程:
//! 1. 解析客户端 `stream` 字段决定上游 stream 模式 (跟随客户端, 不强制)
//! 2. [`crate::proxy::transform::openai::resolve_reasoning_effort`] 解析 reasoning_effort 优先级链
//! 3. [`crate::proxy::transform::openai::anthropic_to_openai_responses`] 翻译 body
//!    (Anthropic Messages → OpenAI Responses), 自动处理 max_tokens 映射 + reasoning include
//! 4. 注入 `Authorization: Bearer <api_key>` (header_name/format 由 yaml 决定; openai.yaml 锁 Bearer)
//! 5. 双路径 finalize:
//!    - client stream=true → 上游 SSE → [`ResponsesSseConverter`] (config.emit_reasoning=true) → client SSE
//!    - client stream=false → 上游 JSON → [`responses_json_to_anthropic`] → client JSON
//!
//! 与 codex (oauth_dispatch.rs) 关键差异:
//! - 用 sk-... API key 而非 OAuth refresh_token
//! - 无 chatgpt 风控 headers (无 build_codex_ua / originator / ChatGPT-Account-Id)
//! - reasoning 默认暴露 (codex 默认 skip)
//! - 真正的 stream=false 上游 JSON 路径 (codex 永远 stream=true 上游)

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
use crate::provider::model::AuthHeaderFormat;
use crate::proxy::handler::error_response;
use crate::proxy::oauth_dispatch::OAuthDispatchError;
use crate::proxy::sse_framing::find_sse_frame_boundary;
use crate::proxy::transform::openai::{
    anthropic_to_openai_responses, responses_json_to_anthropic, OpenAiResponsesExtras,
};
use crate::proxy::transform::responses_common::{
    parse_sse_frame, ResponsesSseConverter, ResponsesTransformConfig,
};
use crate::proxy::upstream;
use crate::subscription::model::SubscriptionRuntime;
use crate::subscription::state_machine;
use crate::virtual_model::VirtualModelName;

/// 上游错误 body 最大保留长度。超过则截断 + 标注总字节数。
/// 避免中转站抛 HTML 错误页 (KB-MB 级) 撑大 SQLite requests.error_message 列。
const MAX_UPSTREAM_ERROR_BODY: usize = 4 * 1024;

fn truncate_error_body(s: String) -> String {
    if s.len() > MAX_UPSTREAM_ERROR_BODY {
        let total = s.len();
        let mut head = s;
        head.truncate(MAX_UPSTREAM_ERROR_BODY);
        format!("{head} ...(truncated, total {total} bytes)")
    } else {
        s
    }
}

/// dispatch_openai_responses_attempt 的成功返回, transform_config 在外层统一保管。
pub struct OpenaiResponsesDispatchOk {
    pub transform_config: ResponsesTransformConfig,
    pub payload: OpenaiResponsesPayload,
}

pub enum OpenaiResponsesPayload {
    Streaming(BoxStream<'static, Result<Bytes, reqwest::Error>>),
    NonStreaming(Value),
}

/// 准备并发送 OpenAI Responses 请求, 同时把响应翻译成 Anthropic 形态返回给客户端.
/// 不做 retry, 由调用方按返回结果决定。
///
/// 错误类型沿用 [`OAuthDispatchError`] (与 gemini/codex dispatch 共线)。
#[allow(clippy::too_many_arguments)]
pub async fn dispatch_openai_responses_attempt(
    http_client: &reqwest::Client,
    api_key: String,
    api_key_header_name: String,
    api_key_header_format: AuthHeaderFormat,
    url: String,
    request_body: &Value,
    client_wants_streaming: bool,
    forward_headers: Vec<String>,
    client_headers: HeaderMap,
    required_headers: std::collections::BTreeMap<String, String>,
    extras: OpenAiResponsesExtras,
) -> Result<OpenaiResponsesDispatchOk, OAuthDispatchError> {
    // 1. 翻译 body (跟随客户端 stream + 注入 reasoning_effort)
    let mut translated_body = anthropic_to_openai_responses(request_body, &extras).map_err(|e| {
        OAuthDispatchError::Upstream {
            status: None,
            message: format!("body 翻译失败: {e}"),
        }
    })?;
    // 显式覆盖 stream 字段以匹配 client 意图 (build_responses_body 已设置, 这里 double-check)
    translated_body["stream"] = Value::Bool(client_wants_streaming);

    let body_bytes = serde_json::to_vec(&translated_body).map_err(|e| {
        OAuthDispatchError::Upstream {
            status: None,
            message: format!("body 序列化失败: {e}"),
        }
    })?;

    // 2. 组装 headers
    let mut headers = ReqHeaderMap::new();
    let header_name = if api_key_header_name.is_empty() {
        "Authorization".to_string()
    } else {
        api_key_header_name
    };
    let auth_name = ReqHeaderName::try_from(header_name.as_str()).map_err(|_| {
        OAuthDispatchError::Upstream {
            status: None,
            message: format!("auth header name {:?} 含非法字符", header_name),
        }
    })?;
    let auth_value_str = api_key_header_format.apply(&api_key);
    let auth_value = ReqHeaderValue::from_str(&auth_value_str).map_err(|_| {
        OAuthDispatchError::Upstream {
            status: None,
            message: "无法构造 Authorization header (API key 含不允许的字符: 控制符 / 非可见 ASCII)".into(),
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
    if client_wants_streaming {
        headers.insert(
            reqwest::header::ACCEPT,
            ReqHeaderValue::from_static("text/event-stream"),
        );
    } else {
        headers.insert(
            reqwest::header::ACCEPT,
            ReqHeaderValue::from_static("application/json"),
        );
    }

    // 3. transform config (与 anthropic_to_openai_responses 内部用的同一份, 为响应翻译复用)
    let mut transform_config = ResponsesTransformConfig::openai_official();
    if extras.expose_reasoning {
        transform_config.emit_reasoning = true;
        transform_config.roundtrip_reasoning = true;
    }

    // 4. 发送请求
    let send_result =
        upstream::send(http_client, &url, body_bytes, headers, client_wants_streaming).await;
    let upstream_response = match send_result {
        Ok(r) => r,
        Err(upstream::UpstreamError::Reqwest(e)) => {
            return Err(OAuthDispatchError::Upstream {
                status: None,
                message: format!("网络错误: {e}"),
            });
        }
    };

    // 5. 按 client_wants_streaming 分支处理
    match upstream_response {
        upstream::UpstreamResponse::Streaming { status, stream, .. } => {
            if !status.is_success() {
                let mut buf = String::new();
                let mut s = stream;
                while let Some(c) = s.next().await {
                    if let Ok(b) = c {
                        if let Ok(t) = std::str::from_utf8(&b) {
                            buf.push_str(t);
                            if buf.len() >= MAX_UPSTREAM_ERROR_BODY * 2 {
                                break;
                            }
                        }
                    }
                }
                return Err(OAuthDispatchError::Upstream {
                    status: Some(status.as_u16()),
                    message: truncate_error_body(buf),
                });
            }
            Ok(OpenaiResponsesDispatchOk {
                transform_config,
                payload: OpenaiResponsesPayload::Streaming(stream),
            })
        }
        upstream::UpstreamResponse::NonStreaming {
            status,
            body,
            body_text,
            ..
        } => {
            if !status.is_success() {
                return Err(OAuthDispatchError::Upstream {
                    status: Some(status.as_u16()),
                    message: truncate_error_body(body_text.unwrap_or_else(|| body.to_string())),
                });
            }
            Ok(OpenaiResponsesDispatchOk {
                transform_config,
                payload: OpenaiResponsesPayload::NonStreaming(body),
            })
        }
    }
}

/// 把 OpenAI Responses 上游响应翻译成给客户端的最终响应 (流式或非流式).
/// 同时投递 RequestLogEntry / events / 状态机事件。
#[allow(clippy::too_many_arguments)]
pub fn finalize_openai_responses(
    ok: OpenaiResponsesDispatchOk,
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
    let OpenaiResponsesDispatchOk { transform_config, payload } = ok;
    match payload {
        OpenaiResponsesPayload::Streaming(upstream_stream) => finalize_streaming(
            upstream_stream,
            transform_config,
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
        ),
        OpenaiResponsesPayload::NonStreaming(body) => finalize_non_streaming(
            body,
            transform_config,
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
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn finalize_streaming(
    upstream_stream: BoxStream<'static, Result<Bytes, reqwest::Error>>,
    transform_config: ResponsesTransformConfig,
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
        let mut converter = ResponsesSseConverter::new_with_config(transform_config);
        let mut buffer = BytesMut::with_capacity(8 * 1024);
        let mut input_tokens: Option<u32> = None;
        let mut output_tokens: Option<u32> = None;
        let mut cache_read: Option<u32> = None;
        let mut response_model_observed: Option<String> = None;

        let mut stream = upstream_stream;
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    warn!(?e, "OpenAI Responses upstream stream error");
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
                let Some((event_name, data)) = parse_sse_frame(frame_str) else {
                    continue;
                };
                let anth_events = converter.feed(&event_name, &data);
                if anth_events.is_empty() {
                    continue;
                }
                let mut buf = Vec::with_capacity(256);
                for evt in anth_events {
                    buf.extend_from_slice(evt.to_sse_frame().as_bytes());
                }
                if client_tx.send(Ok(Bytes::from(buf))).await.is_err() {
                    return;
                }
            }
        }

        // 兜底 message_stop (若 response.completed 没收到)
        let tail = converter.finalize_if_needed();
        if !tail.is_empty() {
            let mut buf = Vec::new();
            for evt in tail {
                buf.extend_from_slice(evt.to_sse_frame().as_bytes());
            }
            let _ = client_tx.send(Ok(Bytes::from(buf))).await;
        }

        // 提取 usage / model (final_usage 是 pub(crate))
        if let Some(usage) = converter.final_usage.as_ref() {
            input_tokens = usage
                .get("input_tokens")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);
            output_tokens = usage
                .get("output_tokens")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);
            cache_read = usage
                .get("input_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(|v| v.as_u64())
                .map(|v| v as u32);
        }
        let response_model = converter.response_model();
        if !response_model.is_empty() {
            response_model_observed = Some(response_model.to_string());
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

        let entry = RequestLogEntry {
            id: attempt_id,
            timestamp_ms: Utc::now().timestamp_millis(),
            virtual_model_name: vm_name,
            subscription_id: sub_id,
            provider_id,
            endpoint_id,
            real_model_name: real_model.clone(),
            response_model_name: response_model_observed,
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
            format!(
                "{} · {} · OpenAI · {}",
                vm_name.as_str(),
                display_name,
                real_model
            ),
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
fn finalize_non_streaming(
    upstream_body: Value,
    transform_config: ResponsesTransformConfig,
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

    // 提取 usage 字段 (用于日志, 翻译前提)
    let usage = upstream_body.get("usage").cloned();
    let input_tokens = usage
        .as_ref()
        .and_then(|u| u.get("input_tokens"))
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let output_tokens = usage
        .as_ref()
        .and_then(|u| u.get("output_tokens"))
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let cache_read = usage
        .as_ref()
        .and_then(|u| u.get("input_tokens_details"))
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let response_model = upstream_body
        .get("model")
        .and_then(|v| v.as_str())
        .map(String::from);

    let final_msg = match responses_json_to_anthropic(&upstream_body, &transform_config) {
        Ok(m) => m,
        Err(e) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "upstream_translation_error",
                &format!("OpenAI Responses JSON 翻译失败: {e}"),
            );
        }
    };


    // 状态机 + 日志 (在 spawn 里跑, 不阻塞响应)
    let log_app = app.clone();
    let log_pool = pool.clone();
    let log_sub_rt = sub_rt.clone();
    let log_event_log_tx = event_log_tx.clone();
    let log_real_model = real_model.clone();
    let log_provider_id = provider_id.clone();
    let log_endpoint_id = endpoint_id.clone();
    let log_display_name = display_name.clone();
    tokio::spawn(async move {
        let _ = state_machine::apply(
            &log_pool,
            &log_app,
            &log_event_log_tx,
            log_sub_rt,
            state_machine::Event::RequestSucceeded,
        )
        .await;

        let entry = RequestLogEntry {
            id: attempt_id,
            timestamp_ms: Utc::now().timestamp_millis(),
            virtual_model_name: vm_name,
            subscription_id: sub_id,
            provider_id: log_provider_id,
            endpoint_id: log_endpoint_id,
            real_model_name: log_real_model.clone(),
            response_model_name: response_model,
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
            &log_event_log_tx,
            attempt_id,
            sub_id,
            Severity::Info,
            format!(
                "{} · {} · OpenAI · {}",
                vm_name.as_str(),
                log_display_name,
                log_real_model
            ),
        );
    });

    let bytes = serde_json::to_vec(&final_msg).unwrap_or_default();
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}


