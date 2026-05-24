//! OpenAI `/v1/chat/completions` API key 订阅的请求 dispatch.
//!
//! 与 [`crate::proxy::openai_responses_dispatch`] 并列, 都属 OpenAI 协议家族但 SSE 状态机不同
//! (chat completions 是 `delta.{content,tool_calls[i]}` 增量, responses 是 `output_item.added` 事件).
//! 由 [`crate::proxy::pipeline::dispatch`] 在检测到 `row.auth_type == OpenaiChatCompletionsApiKey`
//! 时调用. 覆盖 DeepSeek/Together/Groq/Ollama/各类 one-api 中转 / OpenAI 官方早期模型生态.
//!
//! 流程:
//! 1. 解析客户端 `stream` 字段决定上游 stream 模式 (跟随客户端, 不强制)
//! 2. [`crate::proxy::transform::openai::resolve_reasoning_effort`] 解析 reasoning_effort
//! 3. [`crate::proxy::transform::openai_chat_completions::anthropic_to_openai_chat`] 翻译 body
//!    (含 system 合并 / tool_use 提升 tool_calls / tool_result 拆 role:tool / image_url 转换)
//! 4. 注入 `Authorization: Bearer <api_key>` (header_name/format 由订阅 row 决定)
//! 5. 双路径 finalize:
//!    - client stream=true → 上游 SSE → [`ChatCompletionsSseConverter`] → client SSE
//!    - client stream=false → 上游 JSON → [`chat_json_to_anthropic`] → client JSON
//!
//! 与 [`crate::proxy::openai_responses_dispatch`] 关键差异:
//! - transform 配置类型不同 ([`ChatCompletionsTransformConfig`] vs `ResponsesTransformConfig`)
//! - SSE converter 接口不同 (我们的 `ingest(frame)` 直接返回 `Vec<String>` SSE 帧文本,
//!   而 `ResponsesSseConverter::feed` 返回 `AnthropicEvent` 再 `to_sse_frame`)
//! - response id 前缀不同 (`chatcmpl-` vs `resp-`), msg id 由翻译层 / converter 自动加 `msg_` 前缀
//! - chat completions 上游普遍不发 reasoning_tokens / cache_read_tokens 字段, dispatch 仅记录
//!   prompt/completion tokens

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
use crate::proxy::client_fingerprint::ClientContext;
use crate::proxy::handler::error_response;
use crate::proxy::oauth_dispatch::OAuthDispatchError;
use crate::proxy::sse_framing::find_sse_frame_boundary;
use crate::proxy::transform::openai_chat_completions::{
    anthropic_to_openai_chat, chat_json_to_anthropic, parse_token_count, ChatCompletionsExtras,
    ChatCompletionsSseConverter, ChatCompletionsTransformConfig,
};
use crate::proxy::upstream;
use crate::subscription::model::SubscriptionRuntime;
use crate::subscription::state_machine;
use crate::virtual_model::VirtualModelName;

/// 上游错误 body 最大保留长度。超过则截断 + 标注总字节数。
/// 与 openai_responses_dispatch 共线 (4KB), 避免中转 HTML 错误页撑大 sqlite requests.error_message.
const MAX_UPSTREAM_ERROR_BODY: usize = 4 * 1024;

/// 修 #9: SSE buffer 上限. 上游 (恶意 / 中转吞 `\n\n` 分隔符 / 单帧巨型 payload) 永远不
/// 发分隔符时, `buffer.extend_from_slice` 无限增长会拖垮整个 app (CLAUDE.md 明确:
/// 代理 panic / OOM 会拖垮主 app). 超限即视为上游协议异常, 走 UpstreamError 路径.
const MAX_SSE_BUFFER_BYTES: usize = 16 * 1024 * 1024;

/// 流式上游处理结果. 用于 spawn 内统一的 outcome → 状态机/日志映射, 避免 mid-stream
/// 错误后仍走 RequestSucceeded 路径 (修 #3).
enum StreamOutcome {
    Success,
    /// 上游中途错误 (网络断 / converter panic / buffer 超限 / 自定义).
    UpstreamError {
        message: String,
        /// 状态机用的 HTTP 等价 status. 上游 stream 返回 200 后中断: 视为 502.
        http_status: u16,
    },
}

/// 修 #10: error SSE 帧必须符合 Anthropic 规范 `{type:"error", error:{type, message}}`,
/// 否则 Claude Code 等 SDK 会按 schema 严格校验 silent drop 该事件, 用户拿到 truncated 响应
/// 但完全无错误提示.
fn anthropic_error_sse_frame(error_type: &str, message: &str) -> String {
    let payload = serde_json::json!({
        "type": "error",
        "error": {
            "type": error_type,
            "message": message,
        },
    });
    format!(
        "event: error\ndata: {}\n\n",
        serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into())
    )
}

fn truncate_error_body(s: String) -> String {
    if s.len() <= MAX_UPSTREAM_ERROR_BODY {
        return s;
    }
    let total = s.len();
    // Walk back to the nearest UTF-8 char boundary <= MAX. String::truncate panics
    // if the cut lands mid-codepoint, which is common for upstream HTML error pages
    // containing Chinese / emoji (single char = 3-4 bytes).
    let mut cutoff = MAX_UPSTREAM_ERROR_BODY;
    while cutoff > 0 && !s.is_char_boundary(cutoff) {
        cutoff -= 1;
    }
    let mut head = s;
    head.truncate(cutoff);
    format!("{head} ...(truncated, total {total} bytes)")
}

pub struct ChatCompletionsDispatchOk {
    pub transform_config: ChatCompletionsTransformConfig,
    /// 用于 SSE converter / msg_id 拼装 (流式) 和 chat_json_to_anthropic 兜底 (非流).
    pub model: String,
    pub payload: ChatCompletionsPayload,
}

pub enum ChatCompletionsPayload {
    Streaming(BoxStream<'static, Result<Bytes, reqwest::Error>>),
    NonStreaming(Value),
}

/// 准备并发送 OpenAI Chat Completions 请求, 同时把响应翻译成 Anthropic 形态返回给客户端.
/// 不做 retry, 由调用方按返回结果决定。
///
/// 错误类型沿用 [`OAuthDispatchError`] (与 gemini/codex/openai_responses dispatch 共线)。
#[allow(clippy::too_many_arguments)]
pub async fn dispatch_openai_chat_completions_attempt(
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
    extras: ChatCompletionsExtras,
) -> Result<ChatCompletionsDispatchOk, OAuthDispatchError> {
    let transform_config = ChatCompletionsTransformConfig::permissive();
    // 应用 yaml 兜底 (Phase 1 仅 expose_reasoning); 其他 quirks 留给 Phase 2 暴露到 yaml/订阅级
    let mut effective_config = transform_config.clone();
    effective_config.expose_reasoning = extras.expose_reasoning;

    // 1. 翻译 body (跟随客户端 stream, 自动注入 stream_options.include_usage)
    let mut translated_body =
        anthropic_to_openai_chat(request_body, &effective_config, &extras).map_err(|e| {
            OAuthDispatchError::Upstream {
                status: None,
                message: format!("body 翻译失败: {e}"),
            }
        })?;
    // 显式覆盖 stream 字段以匹配 client 意图
    translated_body["stream"] = Value::Bool(client_wants_streaming);

    let real_model = translated_body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

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
            message: "无法构造 Authorization header (API key 含不允许的字符: 控制符 / 非可见 ASCII)"
                .into(),
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

    // 3. 发送请求
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

    // 4. 按 client_wants_streaming 分支处理
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
            Ok(ChatCompletionsDispatchOk {
                transform_config: effective_config,
                model: real_model,
                payload: ChatCompletionsPayload::Streaming(stream),
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
            Ok(ChatCompletionsDispatchOk {
                transform_config: effective_config,
                model: real_model,
                payload: ChatCompletionsPayload::NonStreaming(body),
            })
        }
    }
}

/// 把 OpenAI Chat Completions 上游响应翻译成给客户端的最终响应 (流式或非流式).
/// 同时投递 RequestLogEntry / events / 状态机事件。
#[allow(clippy::too_many_arguments)]
pub fn finalize_openai_chat_completions(
    ok: ChatCompletionsDispatchOk,
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
    let ChatCompletionsDispatchOk {
        transform_config,
        model,
        payload,
    } = ok;
    match payload {
        ChatCompletionsPayload::Streaming(upstream_stream) => finalize_streaming(
            upstream_stream,
            transform_config,
            model,
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
        ),
        ChatCompletionsPayload::NonStreaming(body) => finalize_non_streaming(
            body,
            transform_config,
            attempt_id,
            vm_name,
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
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn finalize_streaming(
    upstream_stream: BoxStream<'static, Result<Bytes, reqwest::Error>>,
    transform_config: ChatCompletionsTransformConfig,
    model: String,
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
        // response_id 占位用 attempt_id (上游 chunk.id 出现时由 converter 自动覆盖)
        let mut converter = ChatCompletionsSseConverter::new(
            transform_config,
            attempt_id.to_string(),
            model.clone(),
        );
        let mut buffer = BytesMut::with_capacity(8 * 1024);

        let mut stream = upstream_stream;
        let mut outcome = StreamOutcome::Success;
        'outer: while let Some(chunk_result) = stream.next().await {
            let chunk = match chunk_result {
                Ok(c) => c,
                Err(e) => {
                    // 修 #3: mid-stream 网络错误改走 UpstreamError outcome, 不再误标 RequestSucceeded
                    warn!(?e, "OpenAI Chat upstream stream error");
                    outcome = StreamOutcome::UpstreamError {
                        message: format!("上游 stream 中断: {e}"),
                        http_status: 502,
                    };
                    break 'outer;
                }
            };

            // 修 #9: buffer 上限保护. 上游不发 `\n\n` 分隔符时累积爆掉 → OOM 拖垮主 app.
            if buffer.len().saturating_add(chunk.len()) > MAX_SSE_BUFFER_BYTES {
                warn!(
                    buf_len = buffer.len(),
                    chunk_len = chunk.len(),
                    cap_mb = MAX_SSE_BUFFER_BYTES / 1024 / 1024,
                    "OpenAI Chat SSE buffer 超上限, 视为上游协议异常"
                );
                outcome = StreamOutcome::UpstreamError {
                    message: format!(
                        "上游 SSE 单帧超过 {}MB (未见 \\n\\n 分隔符), 视为协议异常",
                        MAX_SSE_BUFFER_BYTES / 1024 / 1024
                    ),
                    http_status: 502,
                };
                break 'outer;
            }
            buffer.extend_from_slice(&chunk);

            loop {
                let Some((idx, sep_len)) = find_sse_frame_boundary(&buffer) else {
                    break;
                };
                let frame_bytes = buffer.split_to(idx + sep_len);
                let frame_str = std::str::from_utf8(
                    &frame_bytes[..frame_bytes.len() - sep_len],
                )
                .unwrap_or("");
                // 修 #9: converter.ingest 是 sync, 用 catch_unwind 兜底 panic.
                // panic 时走 UpstreamError 让客户端拿到合法 error 帧而非 silent close.
                let ingest_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || converter.ingest(frame_str),
                ));
                let frames_out = match ingest_result {
                    Ok(f) => f,
                    Err(_) => {
                        warn!("OpenAI Chat converter.ingest panicked, aborting stream");
                        outcome = StreamOutcome::UpstreamError {
                            message: "翻译层 panic (上游 SSE 帧格式可能异常)".into(),
                            http_status: 502,
                        };
                        break 'outer;
                    }
                };
                if frames_out.is_empty() {
                    continue;
                }
                let mut buf = Vec::with_capacity(256);
                for s in frames_out {
                    buf.extend_from_slice(s.as_bytes());
                }
                if client_tx.send(Ok(Bytes::from(buf))).await.is_err() {
                    return;
                }
            }
        }

        // 修 #11: 统一收尾 — 不管 success 还是 error 都让客户端拿到合法的 SSE 结尾
        match &outcome {
            StreamOutcome::Success => {
                // [DONE] 没收到时兜底关 block + 发 message_stop
                let tail = converter.finish();
                if !tail.is_empty() {
                    let mut buf = Vec::new();
                    for s in tail {
                        buf.extend_from_slice(s.as_bytes());
                    }
                    let _ = client_tx.send(Ok(Bytes::from(buf))).await;
                }
            }
            StreamOutcome::UpstreamError { message, .. } => {
                // 若已 started, 关掉所有 open content_block (避免客户端解析器留悬挂状态);
                // 未 started 直接发 error (Anthropic 协议下 error 是终结事件, 不需要 message_start)
                let close_frames = converter.close_open_blocks();
                if !close_frames.is_empty() {
                    let mut buf = Vec::new();
                    for s in close_frames {
                        buf.extend_from_slice(s.as_bytes());
                    }
                    let _ = client_tx.send(Ok(Bytes::from(buf))).await;
                }
                // 修 #10: error 帧用 Anthropic 规范 {type:error, error:{type, message}} 形态
                let err_frame = anthropic_error_sse_frame("overloaded_error", message);
                let _ = client_tx.send(Ok(Bytes::from(err_frame))).await;
            }
        }

        // 修 #14: saturating cast 避免极端大 token 数 (理论 u64 > u32::MAX) 静默截断成低位.
        let input_tokens = converter
            .final_input_tokens()
            .map(|v| u32::try_from(v).unwrap_or(u32::MAX));
        let output_tokens = converter
            .final_output_tokens()
            .map(|v| u32::try_from(v).unwrap_or(u32::MAX));
        let response_model = {
            let m = converter.response_model();
            if m.is_empty() {
                None
            } else {
                Some(m.to_string())
            }
        };

        // 修 #3: 状态机事件 / RequestLogEntry 按 outcome 区分, 不再把所有结果都标 Success
        let (sm_event, status, http_status, error_message) = match &outcome {
            StreamOutcome::Success => (
                state_machine::Event::RequestSucceeded,
                RequestStatus::Success,
                Some(200u16),
                None::<String>,
            ),
            StreamOutcome::UpstreamError {
                message,
                http_status: hs,
            } => (
                state_machine::Event::HttpStatus(*hs),
                RequestStatus::Error,
                Some(*hs),
                Some(message.clone()),
            ),
        };

        let _ = state_machine::apply(&pool, &app, &event_log_tx, sub_rt.clone(), sm_event).await;

        let entry = RequestLogEntry {
            id: attempt_id,
            timestamp_ms: Utc::now().timestamp_millis(),
            virtual_model_name: vm_name,
            subscription_id: sub_id,
            provider_id,
            endpoint_id,
            real_model_name: real_model.clone(),
            response_model_name: response_model,
            is_streaming: true,
            status,
            http_status,
            ttft_ms: None,
            total_latency_ms: Some(start.elapsed().as_millis() as u64),
            upstream_input_tokens: input_tokens,
            upstream_output_tokens: output_tokens,
            upstream_cache_creation: None,
            upstream_cache_read: None,
            retry_count,
            error_message: error_message.clone(),
            upstream_response_body: None,
            client_tool: ctx.info.tool,
            client_user_agent: ctx.info.user_agent.clone(),
            client_version: ctx.info.version.clone(),
            client_ip: ctx.ip.clone(),
        };
        let _ = log_tx.try_send(entry);
        let severity = match &outcome {
            StreamOutcome::Success => Severity::Info,
            StreamOutcome::UpstreamError { .. } => Severity::Error,
        };
        let event_msg = match &outcome {
            StreamOutcome::Success => format!(
                "{} · {} · OpenAI Chat · {}",
                vm_name.as_str(),
                display_name,
                real_model
            ),
            StreamOutcome::UpstreamError { message, .. } => format!(
                "{} · {} · OpenAI Chat · {}",
                vm_name.as_str(),
                display_name,
                message
            ),
        };
        events::record_request(&event_log_tx, attempt_id, sub_id, severity, event_msg);
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
    transform_config: ChatCompletionsTransformConfig,
    attempt_id: Uuid,
    vm_name: VirtualModelName,
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

    // 提取 usage (用于日志, 翻译前提). 修 #14: parse_token_count 容错 float/string +
    // saturating cast 防止 u64 截断.
    let usage = upstream_body.get("usage").cloned();
    let input_tokens = usage
        .as_ref()
        .and_then(|u| u.get("prompt_tokens"))
        .and_then(parse_token_count)
        .map(|v| u32::try_from(v).unwrap_or(u32::MAX));
    let output_tokens = usage
        .as_ref()
        .and_then(|u| u.get("completion_tokens"))
        .and_then(parse_token_count)
        .map(|v| u32::try_from(v).unwrap_or(u32::MAX));
    let response_model = upstream_body
        .get("model")
        .and_then(|v| v.as_str())
        .map(String::from);

    let final_msg =
        match chat_json_to_anthropic(&upstream_body, &transform_config, &attempt_id.to_string()) {
            Ok(m) => m,
            Err(e) => {
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    "upstream_translation_error",
                    &format!("OpenAI Chat JSON 翻译失败: {e}"),
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
            upstream_cache_read: None,
            retry_count,
            error_message: None,
            upstream_response_body: None,
            client_tool: ctx.info.tool,
            client_user_agent: ctx.info.user_agent.clone(),
            client_version: ctx.info.version.clone(),
            client_ip: ctx.ip.clone(),
        };
        let _ = log_tx.try_send(entry);
        events::record_request(
            &log_event_log_tx,
            attempt_id,
            sub_id,
            Severity::Info,
            format!(
                "{} · {} · OpenAI Chat · {}",
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

// ============================================================
// dispatch 层集成测试 (wiremock 模拟上游)
//
// finalize_* 部分需要 axum + tauri::AppHandle, 测起来太重, 此处只验 dispatch_attempt:
// - 请求 body 正确翻译 (model 改写 / stream_options / messages 转换)
// - headers 注入 (Authorization Bearer / Accept)
// - 成功路径返回 ChatCompletionsDispatchOk (含 transform_config / model / payload)
// - 错误路径 (4xx/5xx) 返回 OAuthDispatchError::Upstream 含 status
// ============================================================
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn mk_extras() -> ChatCompletionsExtras {
        ChatCompletionsExtras {
            reasoning_effort: None,
            expose_reasoning: true,
        }
    }

    fn mk_request_body() -> Value {
        json!({
            "model": "deepseek-chat",
            "messages": [{"role": "user", "content": "hello"}],
            "max_tokens": 1024,
        })
    }

    /// 修 #10: error 帧必须是 Anthropic 规范形态 `{type:error, error:{type, message}}`,
    /// 否则 Claude Code SDK 会按 schema 严格校验 silent drop.
    #[test]
    fn anthropic_error_sse_frame_matches_anthropic_spec() {
        let f = anthropic_error_sse_frame("overloaded_error", "上游中断 (timeout)");
        assert!(f.starts_with("event: error\ndata: "));
        assert!(f.ends_with("\n\n"));
        let data_line = f
            .lines()
            .find(|l| l.starts_with("data: "))
            .expect("应有 data: 行");
        let payload: Value =
            serde_json::from_str(data_line.strip_prefix("data: ").unwrap()).unwrap();
        assert_eq!(payload["type"], "error");
        assert_eq!(payload["error"]["type"], "overloaded_error");
        assert_eq!(payload["error"]["message"], "上游中断 (timeout)");
    }

    #[test]
    fn truncate_error_body_handles_utf8_boundary_inside_multibyte_char() {
        // 构造 4096 字节正好落在汉字中间的字符串: 4093 个 'a' + 一个三字节汉字 '中'
        // 总长 4096 字节, MAX 阈值正好截到 '中' 的中间字节, 旧实现会 panic.
        let prefix = "a".repeat(MAX_UPSTREAM_ERROR_BODY - 3);
        let s = format!("{prefix}中{}", "尾".repeat(10));
        assert!(s.len() > MAX_UPSTREAM_ERROR_BODY);
        // 旧实现 head.truncate(MAX) 会在 '中' 三字节序列中间 panic.
        let out = truncate_error_body(s);
        assert!(out.starts_with(&prefix), "前缀必须保留");
        assert!(out.contains("...(truncated, total"));
    }

    #[tokio::test]
    async fn dispatch_non_streaming_success_returns_translated_payload() {
        let server = MockServer::start().await;
        let upstream_resp = json!({
            "id": "chatcmpl-test",
            "model": "deepseek-chat",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "Hi!"},
                "finish_reason": "stop",
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8},
        });

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("Authorization", "Bearer sk-test"))
            .and(header("Content-Type", "application/json"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(upstream_resp.clone())
                    .insert_header("content-type", "application/json"),
            )
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/v1/chat/completions", server.uri());

        let result = dispatch_openai_chat_completions_attempt(
            &client,
            "sk-test".into(),
            "Authorization".into(),
            AuthHeaderFormat::Bearer,
            url,
            &mk_request_body(),
            false, // non-streaming
            Vec::new(),
            HeaderMap::new(),
            std::collections::BTreeMap::new(),
            mk_extras(),
        )
        .await;

        let ok = result.expect("dispatch should succeed");
        assert_eq!(ok.model, "deepseek-chat");
        match ok.payload {
            ChatCompletionsPayload::NonStreaming(body) => {
                assert_eq!(body["id"], "chatcmpl-test");
                assert_eq!(body["choices"][0]["message"]["content"], "Hi!");
            }
            ChatCompletionsPayload::Streaming(_) => panic!("expected NonStreaming"),
        }
    }

    #[tokio::test]
    async fn dispatch_streaming_success_returns_streaming_payload() {
        let server = MockServer::start().await;
        let sse_body = "data: {\"id\":\"chatcmpl-x\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\ndata: [DONE]\n\n";

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(sse_body)
                    .insert_header("content-type", "text/event-stream"),
            )
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/v1/chat/completions", server.uri());
        let mut body = mk_request_body();
        body["stream"] = json!(true);

        let result = dispatch_openai_chat_completions_attempt(
            &client,
            "sk-test".into(),
            "Authorization".into(),
            AuthHeaderFormat::Bearer,
            url,
            &body,
            true,
            Vec::new(),
            HeaderMap::new(),
            std::collections::BTreeMap::new(),
            mk_extras(),
        )
        .await;

        let ok = result.expect("dispatch should succeed");
        assert!(matches!(ok.payload, ChatCompletionsPayload::Streaming(_)));
        assert_eq!(ok.transform_config.expose_reasoning, true);
    }

    #[tokio::test]
    async fn dispatch_upstream_429_returns_upstream_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(429).set_body_json(json!({
                    "error": {"message": "rate limit"}
                })),
            )
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/v1/chat/completions", server.uri());
        let result = dispatch_openai_chat_completions_attempt(
            &client,
            "sk-test".into(),
            "Authorization".into(),
            AuthHeaderFormat::Bearer,
            url,
            &mk_request_body(),
            false,
            Vec::new(),
            HeaderMap::new(),
            std::collections::BTreeMap::new(),
            mk_extras(),
        )
        .await;

        match result {
            Err(OAuthDispatchError::Upstream { status, .. }) => {
                assert_eq!(status, Some(429));
            }
            _ => panic!("expected Upstream(429), got {:?}", result.err()),
        }
    }

    #[tokio::test]
    async fn dispatch_upstream_401_returns_upstream_error_with_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(401).set_body_json(json!({
                    "error": {"message": "invalid api key"}
                })),
            )
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/v1/chat/completions", server.uri());
        let result = dispatch_openai_chat_completions_attempt(
            &client,
            "sk-bad".into(),
            "Authorization".into(),
            AuthHeaderFormat::Bearer,
            url,
            &mk_request_body(),
            false,
            Vec::new(),
            HeaderMap::new(),
            std::collections::BTreeMap::new(),
            mk_extras(),
        )
        .await;

        match result {
            Err(OAuthDispatchError::Upstream { status, message }) => {
                assert_eq!(status, Some(401));
                assert!(message.contains("invalid api key"));
            }
            _ => panic!("expected Upstream(401)"),
        }
    }

    #[tokio::test]
    async fn dispatch_streaming_request_body_injects_stream_options_include_usage() {
        let server = MockServer::start().await;
        let sse_body = "data: [DONE]\n\n";

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            // 验证 body 含 stream_options.include_usage
            .and(wiremock::matchers::body_partial_json(json!({
                "stream": true,
                "stream_options": {"include_usage": true},
            })))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(sse_body)
                    .insert_header("content-type", "text/event-stream"),
            )
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/v1/chat/completions", server.uri());
        let mut body = mk_request_body();
        body["stream"] = json!(true);

        let result = dispatch_openai_chat_completions_attempt(
            &client,
            "sk-test".into(),
            "Authorization".into(),
            AuthHeaderFormat::Bearer,
            url,
            &body,
            true,
            Vec::new(),
            HeaderMap::new(),
            std::collections::BTreeMap::new(),
            mk_extras(),
        )
        .await;

        // wiremock 没匹配上的话 ResponseTemplate 会返 404; 这里 200 表示 body assertion 通过
        assert!(result.is_ok(), "body assertion failed: {:?}", result.err());
    }
}

