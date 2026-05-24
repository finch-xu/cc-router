//! ChatGPT OAuth 订阅的请求 dispatch.
//!
//! 与默认 Anthropic 透传分离, 避免污染主路径. 由 [`pipeline::dispatch`] 在检测到
//! `row.auth_type == ChatgptOauth` 时调用.
//!
//! 流程:
//! 1. 用 `chatgpt_oauth.get_valid_access_token` 拿 access_token (内部内存缓存 + 60s 提前 refresh)
//! 2. `transform::openai_responses::anthropic_to_responses` 翻译 body
//! 3. 注入 `ChatGPT-Account-Id` header + `Authorization: Bearer ...`
//! 4. **强制** `stream: true` 发上游 (ChatGPT 后端拒绝非流式)
//! 5. 上游 SSE 用 `ResponsesSseConverter` 翻译成 Anthropic SSE
//! 6. 客户端要非流式 → 用 `NonStreamingCollector` 拼出 Anthropic Messages JSON
//!
//! 注意: 这一路完全不走 `proxy::sse::stream_response`, 因为后者做的是 Anthropic
//! 透传 + model 改写, 与翻译路径冲突.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
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

use crate::oauth::chatgpt::{
    build_codex_ua, ChatGptOAuthManager, OAuthError, CHATGPT_ACCOUNT_ID_HEADER, CODEX_ORIGINATOR,
};
use crate::oauth::kiro::{
    KiroOAuthManager, KIRO_AGENT_MODE_HEADER, KIRO_AMZ_INVOCATION_HEADER, KIRO_AMZ_UA_HEADER,
    KIRO_OPTOUT_HEADER,
};
use crate::observability::events::{self, EventEntry, Severity};
use crate::observability::request_log::{RequestLogEntry, RequestStatus};
use crate::proxy::client_fingerprint::ClientContext;
use crate::proxy::handler::error_body;
use crate::proxy::sse_framing::find_sse_frame_boundary;
use crate::proxy::transform::aws_event_stream::EventStreamDecoder;
use crate::proxy::transform::kiro_codewhisperer::{
    anthropic_to_codewhisperer, KiroSseConverter, NonStreamingCollector as KiroCollector,
};
use crate::proxy::transform::openai_responses::{
    anthropic_to_responses, parse_sse_frame, CodexExtras, NonStreamingCollector,
    ResponsesSseConverter,
};
use crate::proxy::transform::responses_common::ResponsesTransformConfig;
use crate::proxy::upstream;
use crate::subscription::model::{OAuthMetadata, SubscriptionRuntime};
use crate::subscription::state_machine;
use crate::virtual_model::VirtualModelName;

/// 外部 OAuth 错误的内部分类, 决定 pipeline 是否 retry / 写错误状态.
#[derive(Debug)]
pub enum OAuthDispatchError {
    /// access_token 拿不到 (refresh 也失败), 标订阅 auth_failed.
    Auth(String),
    /// 上游返回 4xx/5xx 或网络错误. 携带 status (可选) 与 message.
    Upstream {
        status: Option<u16>,
        message: String,
    },
}

/// 准备并发送 OAuth 请求, 同时把响应翻译成 Anthropic 形态返回给客户端.
/// 不做 retry, 由调用方根据返回的 Result 决定.
///
/// `client_wants_streaming`: 客户端原始请求里 `stream` 字段的解析结果; 决定我们如何把上游 SSE 还回去.
#[allow(clippy::too_many_arguments)]
pub async fn dispatch_oauth_attempt(
    http_client: &reqwest::Client,
    oauth_manager: Arc<ChatGptOAuthManager>,
    sub_id: Uuid,
    refresh_token: String,
    account_id: String,
    url: String,
    request_body: &Value,
    client_wants_streaming: bool,
    forward_headers: Vec<String>,
    client_headers: HeaderMap,
    required_headers: std::collections::BTreeMap<String, String>,
    extras: CodexExtras,
) -> Result<OAuthDispatchOk, OAuthDispatchError> {
    // 1. 拿 access_token
    let access_token = match oauth_manager.get_valid_access_token(sub_id, &refresh_token).await {
        Ok(t) => t,
        Err(OAuthError::RefreshTokenInvalid) => {
            return Err(OAuthDispatchError::Auth(
                "refresh_token 已失效, 需要重新登录 ChatGPT".into(),
            ))
        }
        Err(e) => {
            return Err(OAuthDispatchError::Auth(format!("OAuth 刷新失败: {e}")))
        }
    };

    // 2. 翻译 body
    let transform_config = ResponsesTransformConfig::codex_chatgpt(extras.expose_reasoning);
    let translated_body =
        anthropic_to_responses(request_body, &extras).map_err(|e| OAuthDispatchError::Upstream {
            status: None,
            message: format!("body 翻译失败: {e}"),
        })?;
    let body_bytes = serde_json::to_vec(&translated_body).map_err(|e| {
        OAuthDispatchError::Upstream {
            status: None,
            message: format!("body 序列化失败: {e}"),
        }
    })?;

    // 3. headers
    let mut headers = ReqHeaderMap::new();
    if let Ok(value) = ReqHeaderValue::from_str(&format!("Bearer {access_token}")) {
        headers.insert(reqwest::header::AUTHORIZATION, value);
    }
    if let Ok(value) = ReqHeaderValue::from_str(&account_id) {
        if let Ok(name) = ReqHeaderName::try_from(CHATGPT_ACCOUNT_ID_HEADER) {
            headers.insert(name, value);
        }
    }
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
    // 强制覆盖全局 http_client 的 cc-router UA, 用 codex_cli_rs/<ver> (... ) cc-router/<ver>.
    // OpenAI 风控查 UA 会校验是否为已知客户端, 否则更易触发手机验证 / 二次确认.
    if let Ok(ua) = ReqHeaderValue::from_str(build_codex_ua()) {
        headers.insert(reqwest::header::USER_AGENT, ua);
    }
    // 同步 originator header. yaml required_headers 已带, 这里幂等覆盖以防老 snapshot 漏带.
    headers.insert(
        ReqHeaderName::from_static("originator"),
        ReqHeaderValue::from_static(CODEX_ORIGINATOR),
    );

    // 4. 发送 (强制 streaming 上游, 因为 ChatGPT 后端拒绝非流式)
    let send_result = upstream::send(http_client, &url, body_bytes, headers, true).await;

    let upstream_response = match send_result {
        Ok(r) => r,
        Err(upstream::UpstreamError::Reqwest(e)) => {
            return Err(OAuthDispatchError::Upstream {
                status: None,
                message: format!("网络错误: {e}"),
            })
        }
    };

    let (status, resp_headers, stream) = match upstream_response {
        upstream::UpstreamResponse::Streaming { status, headers, stream } => {
            (status, headers, stream)
        }
        upstream::UpstreamResponse::NonStreaming { status, body_text, .. } => {
            // 上游强制流式, 走到这里说明 status 不是 2xx (upstream::send 把非 2xx 当 NonStreaming 处理).
            return Err(OAuthDispatchError::Upstream {
                status: Some(status.as_u16()),
                message: body_text.unwrap_or_else(|| format!("HTTP {}", status.as_u16())),
            });
        }
    };

    if !status.is_success() {
        // 把上游的 SSE 流读完作为错误信息
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
        transform_config: Some(transform_config),
        gemini_emit_thoughts: false,
    })
}

/// dispatch_oauth_attempt 成功时返回的「待消费的上游流」+ 客户端意图.
///
/// 翻译配置按 provider 分别填入:
/// - codex / openai responses: `transform_config = Some(...)`, `gemini_emit_thoughts` 不读
/// - gemini: `transform_config = None`, `gemini_emit_thoughts = yaml.expose_reasoning`
/// - kiro: 两者都不读 (走自家协议翻译)
pub struct OAuthDispatchOk {
    pub upstream_status: reqwest::StatusCode,
    #[allow(dead_code)]
    pub upstream_headers: ReqHeaderMap,
    pub upstream_stream: BoxStream<'static, Result<Bytes, reqwest::Error>>,
    pub client_wants_streaming: bool,
    pub transform_config: Option<ResponsesTransformConfig>,
    /// Gemini 专用: SSE converter 是否暴露 thought parts。dispatch_gemini_attempt 填,
    /// finalize_gemini_response 消费。其它 provider 留 false 默认即可。
    pub gemini_emit_thoughts: bool,
}

/// 把 OAuth 上游的 SSE 流翻译成给客户端的最终响应 (流式或非流式).
/// 同时投递 RequestLogEntry / events / 状态机事件.
#[allow(clippy::too_many_arguments)]
pub fn finalize_oauth_response(
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
    let transform_config = ok
        .transform_config
        .unwrap_or_else(|| ResponsesTransformConfig::codex_chatgpt(false));
    if ok.client_wants_streaming {
        finalize_streaming(
            ok.upstream_stream,
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
            ctx,
        )
    } else {
        // 非流式: tokio task 收完, 然后返回 oneshot. 但 axum 需要同步返回 Response,
        // 我们把整个 collect 过程跑在异步 body 里.
        let fut = collect_to_json_response(
            ok.upstream_stream,
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
            ctx,
        );
        // axum::response::Response 不能 hold a future. 需要 spawn 然后用 oneshot 返回.
        // 简单做法: block_on 是错的. 改用 streamed body 包一个一次性 chunk.
        // 这里偷个巧: 把 collect 跑成一个返回 single chunk 的 stream.
        let stream = futures::stream::once(async move {
            let resp = fut.await;
            // 把 Response body 转成 Bytes
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
    ctx: ClientContext,
) -> Response {
    let (client_tx, client_rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(64);

    tokio::spawn(async move {
        let start = std::time::Instant::now();
        let mut converter = ResponsesSseConverter::new_with_config(transform_config);
        let mut buffer = BytesMut::with_capacity(8 * 1024);
        let mut input_tokens: Option<u32> = None;
        let mut output_tokens: Option<u32> = None;

        let mut stream = upstream_stream;
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    warn!(?e, "OAuth upstream stream error");
                    let _ = client_tx
                        .send(Ok(Bytes::from(format!(
                            "event: error\ndata: {{\"error\":\"{}\"}}\n\n",
                            e
                        ))))
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

                // 抽 usage 用于日志, 即使 converter 不返回事件
                if event_name == "response.completed" {
                    if let Some(usage) = data.get("response").and_then(|r| r.get("usage")) {
                        input_tokens = usage
                            .get("input_tokens")
                            .and_then(|v| v.as_u64())
                            .map(|v| v as u32);
                        output_tokens = usage
                            .get("output_tokens")
                            .and_then(|v| v.as_u64())
                            .map(|v| v as u32);
                    }
                }

                let anth_events = converter.feed(&event_name, &data);
                for evt in anth_events {
                    let frame = evt.to_sse_frame();
                    if client_tx.send(Ok(Bytes::from(frame))).await.is_err() {
                        return;
                    }
                }
            }
        }

        // 兜底 message_stop
        for evt in converter.finalize_if_needed() {
            let _ = client_tx.send(Ok(Bytes::from(evt.to_sse_frame()))).await;
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
            upstream_cache_read: None,
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
            format!("{} · {} · {}", vm_name.as_str(), display_name, real_model),
        );
    });

    let stream = futures::stream::unfold(client_rx, |mut rx| async move {
        match rx.recv().await {
            Some(item) => Some((item, rx)),
            None => None,
        }
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
    ctx: ClientContext,
) -> Response {
    let start = std::time::Instant::now();
    let mut collector = NonStreamingCollector::new_with_config(transform_config);
    let mut buffer = BytesMut::with_capacity(8 * 1024);
    let mut input_tokens: Option<u32> = None;
    let mut output_tokens: Option<u32> = None;

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
            if let Some((event_name, data)) = parse_sse_frame(frame_str) {
                if event_name == "response.completed" {
                    if let Some(usage) = data.get("response").and_then(|r| r.get("usage")) {
                        input_tokens = usage
                            .get("input_tokens")
                            .and_then(|v| v.as_u64())
                            .map(|v| v as u32);
                        output_tokens = usage
                            .get("output_tokens")
                            .and_then(|v| v.as_u64())
                            .map(|v| v as u32);
                    }
                }
                collector.feed(&event_name, &data);
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
        upstream_cache_read: None,
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
        format!("{} · {} · {}", vm_name.as_str(), display_name, real_model),
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

// ============================================================================
// Kiro 分支 (与 ChatgptOauth 完全独立, 共用 OAuthDispatchOk / OAuthDispatchError / build_error_response)
// ============================================================================

/// Kiro OAuth dispatch. 上游 AWS CodeWhisperer 返回 Event Stream 二进制流.
#[allow(clippy::too_many_arguments)]
pub async fn dispatch_kiro_attempt(
    http_client: &reqwest::Client,
    oauth_manager: Arc<KiroOAuthManager>,
    sub_id: Uuid,
    metadata: OAuthMetadata,
    url: String,
    request_body: &Value,
    client_wants_streaming: bool,
    forward_headers: Vec<String>,
    client_headers: HeaderMap,
    required_headers: std::collections::BTreeMap<String, String>,
) -> Result<OAuthDispatchOk, OAuthDispatchError> {
    let kiro_extras = metadata
        .kiro
        .as_ref()
        .ok_or_else(|| OAuthDispatchError::Auth("Kiro 订阅缺少 oauth_metadata.kiro".into()))?;

    // 1. 拿 access_token
    let access_token = match oauth_manager.get_valid_access_token(sub_id, &metadata).await {
        Ok(t) => t,
        Err(OAuthError::RefreshTokenInvalid) => {
            return Err(OAuthDispatchError::Auth(
                "Kiro refresh_token 已失效, 需要重新登录".into(),
            ));
        }
        Err(e) => return Err(OAuthDispatchError::Auth(format!("Kiro OAuth 刷新失败: {e}"))),
    };

    // 2. 翻译 body (Anthropic Messages → CodeWhisperer JSON)
    let translated_body = anthropic_to_codewhisperer(
        request_body,
        kiro_extras.profile_arn.as_deref(),
        None,
    )
    .map_err(|e| OAuthDispatchError::Upstream {
        status: None,
        message: format!("body 翻译失败: {e}"),
    })?;
    let body_bytes = serde_json::to_vec(&translated_body).map_err(|e| OAuthDispatchError::Upstream {
        status: None,
        message: format!("body 序列化失败: {e}"),
    })?;

    // 3. headers (5 个 Kiro 风控伪装 header + Authorization)
    let mut headers = ReqHeaderMap::new();
    let disguise = &kiro_extras.disguise;
    let auth_value = ReqHeaderValue::from_str(&format!("Bearer {access_token}"))
        .map_err(|e| OAuthDispatchError::Upstream {
            status: None,
            message: format!("Authorization header 构造失败: {e}"),
        })?;
    headers.insert(reqwest::header::AUTHORIZATION, auth_value);

    let x_amz_user_agent = format!(
        "aws-sdk-js/1.0.34 KiroIDE-{}-{}",
        disguise.kiro_version, disguise.machine_id
    );
    let kiro_user_agent = format!(
        "aws-sdk-js/1.0.34 ua/2.1 os/{} lang/js md/nodejs#{} api/codewhispererstreaming#1.0.34 m/E KiroIDE-{}-{}",
        disguise.system_version, disguise.node_version, disguise.kiro_version, disguise.machine_id
    );
    let amz_invocation_id = Uuid::new_v4().to_string();
    let kiro_headers = [
        (KIRO_OPTOUT_HEADER, "true".to_string()),
        (KIRO_AGENT_MODE_HEADER, "vibe".to_string()),
        (KIRO_AMZ_UA_HEADER, x_amz_user_agent),
        (KIRO_AMZ_INVOCATION_HEADER, amz_invocation_id),
    ];
    for (name, value) in &kiro_headers {
        if let (Ok(n), Ok(v)) = (ReqHeaderName::try_from(*name), ReqHeaderValue::from_str(value)) {
            headers.insert(n, v);
        }
    }
    if let Ok(ua) = ReqHeaderValue::from_str(&kiro_user_agent) {
        headers.insert(reqwest::header::USER_AGENT, ua);
    }

    // yaml `required_headers` 兜底; 与上面已注入的 4 个 kiro headers 冲突的跳过.
    for (k, v) in required_headers {
        if !k.eq_ignore_ascii_case("User-Agent")
            && !k.eq_ignore_ascii_case(KIRO_AMZ_UA_HEADER)
            && !k.eq_ignore_ascii_case(KIRO_OPTOUT_HEADER)
            && !k.eq_ignore_ascii_case(KIRO_AGENT_MODE_HEADER)
        {
            if let (Ok(name), Ok(value)) =
                (ReqHeaderName::try_from(k.as_str()), ReqHeaderValue::from_str(&v))
            {
                headers.insert(name, value);
            }
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
        ReqHeaderValue::from_static("application/vnd.amazon.eventstream"),
    );

    // 4. 发送 (Kiro 后端响应总是 Event Stream)
    let send_result = upstream::send(http_client, &url, body_bytes, headers, true).await;
    let upstream_response = match send_result {
        Ok(r) => r,
        Err(upstream::UpstreamError::Reqwest(e)) => {
            return Err(OAuthDispatchError::Upstream {
                status: None,
                message: format!("Kiro 网络错误: {e}"),
            });
        }
    };
    let (status, resp_headers, stream) = match upstream_response {
        upstream::UpstreamResponse::Streaming { status, headers, stream } => (status, headers, stream),
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
        gemini_emit_thoughts: false,
    })
}

/// 把 Kiro Event Stream 流翻译成 Anthropic SSE / JSON 给客户端.
#[allow(clippy::too_many_arguments)]
pub fn finalize_kiro_response(
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
    if ok.client_wants_streaming {
        finalize_kiro_streaming(
            ok.upstream_stream, vm_name, attempt_id, sub_id, provider_id, endpoint_id,
            real_model, display_name, retry_count, log_tx, event_log_tx, pool, app, sub_rt, ctx,
        )
    } else {
        let fut = collect_kiro_to_json_response(
            ok.upstream_stream, vm_name, attempt_id, sub_id, provider_id, endpoint_id,
            real_model, display_name, retry_count, log_tx, event_log_tx, pool, app, sub_rt, ctx,
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
fn finalize_kiro_streaming(
    upstream_stream: BoxStream<'static, Result<Bytes, reqwest::Error>>,
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
        let mut decoder = EventStreamDecoder::new();
        let mut converter = KiroSseConverter::new(&real_model);

        let mut stream = upstream_stream;
        'outer: while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    warn!(?e, "Kiro upstream stream error");
                    let _ = client_tx
                        .send(Ok(Bytes::from(format!(
                            "event: error\ndata: {{\"error\":\"{}\"}}\n\n",
                            e
                        ))))
                        .await;
                    break;
                }
            };
            let frames = match decoder.feed_and_drain(&chunk) {
                Ok(f) => f,
                Err(e) => {
                    warn!(?e, "Kiro Event Stream 解码错误");
                    let _ = client_tx
                        .send(Ok(Bytes::from(format!(
                            "event: error\ndata: {{\"error\":\"event_stream_decode: {}\"}}\n\n",
                            e
                        ))))
                        .await;
                    break 'outer;
                }
            };
            for frame in frames {
                let anth_events = converter.feed(&frame);
                if anth_events.is_empty() {
                    continue;
                }
                let mut buf = Vec::new();
                for evt in anth_events {
                    buf.extend_from_slice(&evt.to_sse_bytes());
                }
                if client_tx.send(Ok(Bytes::from(buf))).await.is_err() {
                    return;
                }
            }
        }
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

        let entry = RequestLogEntry {
            id: attempt_id,
            timestamp_ms: Utc::now().timestamp_millis(),
            virtual_model_name: vm_name,
            subscription_id: sub_id,
            provider_id,
            endpoint_id,
            real_model_name: real_model.clone(),
            response_model_name: None,
            is_streaming: true,
            status: RequestStatus::Success,
            http_status: Some(200),
            ttft_ms: None,
            total_latency_ms: Some(start.elapsed().as_millis() as u64),
            upstream_input_tokens: input_tokens,
            upstream_output_tokens: output_tokens,
            upstream_cache_creation: None,
            upstream_cache_read: None,
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
            format!("{} · {} · Kiro · {}", vm_name.as_str(), display_name, real_model),
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
async fn collect_kiro_to_json_response(
    upstream_stream: BoxStream<'static, Result<Bytes, reqwest::Error>>,
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
    let mut decoder = EventStreamDecoder::new();
    let mut collector = KiroCollector::new(&real_model);

    let mut stream = upstream_stream;
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else { break };
        let frames = match decoder.feed_and_drain(&chunk) {
            Ok(f) => f,
            Err(e) => {
                warn!(?e, "Kiro Event Stream 解码错误");
                break;
            }
        };
        for frame in frames {
            collector.feed(&frame);
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
        upstream_cache_read: None,
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
        format!("{} · {} · Kiro · {}", vm_name.as_str(), display_name, real_model),
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

/// 把 OAuth 错误转成给客户端的 axum Response.
pub fn build_error_response(err: &OAuthDispatchError) -> Response {
    match err {
        OAuthDispatchError::Auth(msg) => (
            StatusCode::UNAUTHORIZED,
            Json(error_body("oauth_auth_failed", msg)),
        )
            .into_response(),
        OAuthDispatchError::Upstream { status, message } => {
            let code = status
                .and_then(|s| StatusCode::from_u16(s).ok())
                .unwrap_or(StatusCode::BAD_GATEWAY);
            (code, Json(error_body("oauth_upstream_error", message))).into_response()
        }
    }
}
