//! 请求 pipeline（设计稿 §5.1 步骤 1-6 + 重试）。

use std::time::Instant;

use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use reqwest::header::{HeaderMap as ReqHeaderMap, HeaderName as ReqHeaderName, HeaderValue as ReqHeaderValue};
use serde_json::Value;
use tauri::Emitter;
use tracing::{info, warn};
use uuid::Uuid;

use crate::error::AppResult;
use crate::observability::body_dump::{BodyDumpEntry, BodyDumpKind};
use crate::observability::events::{self, Severity};
use crate::observability::request_log::{RequestLogEntry, RequestStatus};
use crate::provider::model::AuthType;
use crate::proxy::handler::error_body;
use crate::proxy::oauth_dispatch::{
    self, OAuthDispatchError,
};
use crate::proxy::overloaded;
use crate::proxy::retry::{classify_response, ShouldRetry};
use crate::proxy::sse;
use crate::proxy::upstream;
use crate::state::AppState;
use crate::subscription::state_machine;
use crate::virtual_model::scheduler::build_candidate_order;
use crate::virtual_model::VirtualModelName;

const ERROR_BODY_LIMIT: usize = 4096;

/// 按 char_indices 找 UTF-8 边界, 避免切坏多字节字符
fn truncate_body(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let cut = text
        .char_indices()
        .take_while(|(idx, _)| *idx <= limit)
        .last()
        .map(|(idx, _)| idx)
        .unwrap_or(0);
    let mut out = text[..cut].to_string();
    out.push_str("...[truncated]");
    out
}

fn emit_attempt_started(state: &AppState, sub_id: Uuid, vm_name: VirtualModelName) {
    let _ = state.app_handle.emit(
        "route_attempt_started",
        serde_json::json!({
            "subscription_id": sub_id.to_string(),
            "virtual_model": vm_name.as_str(),
        }),
    );
}

fn emit_attempt_finished(state: &AppState, sub_id: Uuid, vm_name: VirtualModelName, success: bool) {
    let _ = state.app_handle.emit(
        "route_attempt_finished",
        serde_json::json!({
            "subscription_id": sub_id.to_string(),
            "virtual_model": vm_name.as_str(),
            "success": success,
        }),
    );
}

pub async fn dispatch(
    state: &AppState,
    model: &str,
    request_body: Value,
    client_headers: HeaderMap,
    is_streaming: bool,
) -> AppResult<Response> {
    // 1. 解析虚拟模型; 非三个虚拟名走 fallback（透传原始 model）
    let vm_name = VirtualModelName::parse(model).unwrap_or(VirtualModelName::Fallback);
    let is_fallback = vm_name.is_fallback();

    // 2. 获取候选订阅顺序
    let vm_config = {
        let guard = state.virtual_models.read().await;
        match guard.get(&vm_name) {
            Some(cfg) => cfg.clone(),
            None => {
                return Ok(overloaded::response(vm_name, &[]));
            }
        }
    };
    if vm_config.subscription_ids.is_empty() {
        return Ok(overloaded::response(vm_name, &[]));
    }

    let subs_map = state.subscriptions.read().await.clone();
    let order = build_candidate_order(&vm_config, &subs_map, Utc::now()).await;
    drop(subs_map);

    if order.candidate_ids.is_empty() {
        let subs_map = state.subscriptions.read().await;
        let mut summary = Vec::new();
        for sub_id in &vm_config.subscription_ids {
            if let Some(rt) = subs_map.get(sub_id) {
                let g = rt.read().await;
                summary.push(format!(
                    "- {}: {:?}",
                    g.row.display_name,
                    g.state,
                ));
            }
        }
        return Ok(overloaded::response_with_summary(vm_name, &summary));
    }

    // 更新轮询索引（round_robin 模式）
    if let Some(idx) = order.chosen_index {
        let mut guard = state.virtual_models.write().await;
        if let Some(cfg) = guard.get_mut(&vm_name) {
            cfg.last_used_index = idx;
        }
    }

    let mut retry_count: u32 = 0;
    let start = Instant::now();
    // 一次 dispatch 可能产生多个 attempt(每次 retry 一行日志)。每个 attempt 用独立 id,
    // 避免 PRIMARY KEY 冲突 + 让用户在 Logs 页能看到每次 retry 的具体错误。
    let dispatch_id = Uuid::new_v4();
    // 读一次 settings.debug_mode 给整个 dispatch 期间所有 attempt 共用,
    // 避免 attempt 循环里多次过 RwLock(单次 dispatch 期间 toggle 切换的窗口无实际价值)。
    let debug_mode = state.settings.read().await.debug_mode;
    // debug_mode 下每次 retry 写盘的 client body 字节序列, 单次 dispatch 内一致, 序列化一次复用。
    let client_bytes_cached: Option<Vec<u8>> = if debug_mode {
        Some(serde_json::to_vec(&request_body).unwrap_or_default())
    } else {
        None
    };

    for sub_id in order.candidate_ids {
        let attempt_id = Uuid::new_v4();
        let rt = {
            let subs_map = state.subscriptions.read().await;
            subs_map.get(&sub_id).cloned()
        };
        let Some(rt) = rt else { continue };

        // 从订阅 row snapshot 读取所有连接信息(snapshot 模型: 不再回查 state.providers)
        let (
            provider_id,
            endpoint_id,
            real_model,
            display_name,
            url,
            auth_header_name,
            auth_header_value,
            required_headers,
            forward_headers,
            auth_type,
            oauth_metadata,
        ) = {
            let guard = rt.read().await;
            // fallback 透传原始 model; 其他三个按 slot 映射
            let real_model = if is_fallback {
                model.to_string()
            } else {
                let slot = vm_name.slot();
                guard.row.model_slots.get(slot).to_string()
            };
            (
                guard.row.provider_id.clone(),
                guard.row.endpoint_id.clone(),
                real_model,
                guard.row.display_name.clone(),
                guard.row.messages_url(),
                guard.row.auth_header_name.clone(),
                guard.row.auth_header_value(),
                guard.row.required_headers.clone(),
                guard.row.forward_headers.clone(),
                guard.row.auth_type,
                guard.row.oauth_metadata.clone(),
            )
        };
        let oauth_refresh_token = oauth_metadata.refresh_token.clone();
        let oauth_account_id = oauth_metadata.account_id.clone();

        // ChatGPT OAuth 分支: 走独立的 dispatch + 翻译层 (proxy::oauth_dispatch).
        // fallback 透传与 OAuth 互斥: 此 provider 不允许做 fallback (model 必须改写到 codex 模型).
        if matches!(auth_type, AuthType::ChatgptOauth) {
            // 改写 model 字段后给翻译层 (类似默认路径里 fallback==false 的处理)
            let mut oauth_body = request_body.clone();
            if !is_fallback {
                oauth_body["model"] = Value::String(real_model.clone());
            }
            emit_attempt_started(state, sub_id, vm_name);
            let dispatch_res = oauth_dispatch::dispatch_oauth_attempt(
                &state.http_client,
                state.chatgpt_oauth.clone(),
                sub_id,
                oauth_refresh_token,
                oauth_account_id,
                url.clone(),
                &oauth_body,
                is_streaming,
                forward_headers.clone(),
                client_headers.clone(),
                required_headers.clone(),
            )
            .await;

            match dispatch_res {
                Ok(ok) => {
                    emit_attempt_finished(state, sub_id, vm_name, true);
                    return Ok(oauth_dispatch::finalize_oauth_response(
                        ok,
                        vm_name,
                        attempt_id,
                        sub_id,
                        provider_id,
                        endpoint_id,
                        real_model,
                        display_name,
                        retry_count,
                        state.request_log_tx.clone(),
                        state.event_log_tx.clone(),
                        state.db.clone(),
                        state.app_handle.clone(),
                        rt.clone(),
                    ));
                }
                Err(err) => {
                    emit_attempt_finished(state, sub_id, vm_name, false);
                    let (event, retryable) = match &err {
                        OAuthDispatchError::Auth(_) => (state_machine::Event::HttpStatus(401), false),
                        OAuthDispatchError::Upstream { status, .. } => {
                            let s = status.unwrap_or(502);
                            let retryable =
                                matches!(classify_response(s, None), ShouldRetry::Yes(_));
                            (state_machine::Event::HttpStatus(s), retryable)
                        }
                    };
                    let _ = state_machine::apply(
                        &state.db,
                        &state.app_handle,
                        &state.event_log_tx,
                        rt.clone(),
                        event,
                    )
                    .await;

                    let err_msg = match &err {
                        OAuthDispatchError::Auth(m) => m.clone(),
                        OAuthDispatchError::Upstream { message, .. } => message.clone(),
                    };
                    let entry = RequestLogEntry {
                        id: attempt_id,
                        timestamp_ms: chrono::Utc::now().timestamp_millis(),
                        virtual_model_name: vm_name,
                        subscription_id: sub_id,
                        provider_id: provider_id.clone(),
                        endpoint_id: endpoint_id.clone(),
                        real_model_name: real_model.clone(),
                        response_model_name: None,
                        is_streaming,
                        status: RequestStatus::Error,
                        http_status: match &err {
                            OAuthDispatchError::Upstream { status, .. } => *status,
                            _ => Some(401),
                        },
                        ttft_ms: None,
                        total_latency_ms: Some(start.elapsed().as_millis() as u64),
                        upstream_input_tokens: None,
                        upstream_output_tokens: None,
                        upstream_cache_creation: None,
                        upstream_cache_read: None,
                        retry_count,
                        error_message: Some(err_msg.clone()),
                        upstream_response_body: Some(truncate_body(&err_msg, ERROR_BODY_LIMIT)),
                    };
                    let _ = state.request_log_tx.try_send(entry);
                    events::record_request(
                        &state.event_log_tx,
                        attempt_id,
                        sub_id,
                        Severity::Error,
                        format!(
                            "{} · {} · OAuth · {}",
                            vm_name.as_str(),
                            display_name,
                            err_msg
                        ),
                    );

                    if retryable {
                        retry_count += 1;
                        continue;
                    }
                    return Ok(oauth_dispatch::build_error_response(&err));
                }
            }
        }

        // Kiro OAuth 分支: 凭据走 AWS Builder ID OIDC 或 Kiro IDE JSON 文件,
        // 上游为 AWS CodeWhisperer (二进制 Event Stream), 协议完全独立, 与 codex 互不污染.
        // fallback 与 OAuth 互斥, 同 codex 规则.
        if matches!(auth_type, AuthType::KiroOauth) {
            if is_fallback {
                // fallback 透传原始 model, Kiro 后端只认 codewhisperer 模型, 必然 400.
                // 直接跳过此候选, 不计入 retry.
                continue;
            }
            let mut kiro_body = request_body.clone();
            kiro_body["model"] = Value::String(real_model.clone());
            emit_attempt_started(state, sub_id, vm_name);
            let dispatch_res = oauth_dispatch::dispatch_kiro_attempt(
                &state.http_client,
                state.kiro_oauth.clone(),
                sub_id,
                oauth_metadata.clone(),
                url.clone(),
                &kiro_body,
                is_streaming,
                forward_headers.clone(),
                client_headers.clone(),
                required_headers.clone(),
            )
            .await;

            match dispatch_res {
                Ok(ok) => {
                    emit_attempt_finished(state, sub_id, vm_name, true);
                    return Ok(oauth_dispatch::finalize_kiro_response(
                        ok,
                        vm_name,
                        attempt_id,
                        sub_id,
                        provider_id,
                        endpoint_id,
                        real_model,
                        display_name,
                        retry_count,
                        state.request_log_tx.clone(),
                        state.event_log_tx.clone(),
                        state.db.clone(),
                        state.app_handle.clone(),
                        rt.clone(),
                    ));
                }
                Err(err) => {
                    emit_attempt_finished(state, sub_id, vm_name, false);
                    let (event, retryable) = match &err {
                        OAuthDispatchError::Auth(_) => (state_machine::Event::HttpStatus(401), false),
                        OAuthDispatchError::Upstream { status, .. } => {
                            let s = status.unwrap_or(502);
                            let retryable =
                                matches!(classify_response(s, None), ShouldRetry::Yes(_));
                            (state_machine::Event::HttpStatus(s), retryable)
                        }
                    };
                    let _ = state_machine::apply(
                        &state.db,
                        &state.app_handle,
                        &state.event_log_tx,
                        rt.clone(),
                        event,
                    )
                    .await;

                    let err_msg = match &err {
                        OAuthDispatchError::Auth(m) => m.clone(),
                        OAuthDispatchError::Upstream { message, .. } => message.clone(),
                    };
                    let entry = RequestLogEntry {
                        id: attempt_id,
                        timestamp_ms: chrono::Utc::now().timestamp_millis(),
                        virtual_model_name: vm_name,
                        subscription_id: sub_id,
                        provider_id: provider_id.clone(),
                        endpoint_id: endpoint_id.clone(),
                        real_model_name: real_model.clone(),
                        response_model_name: None,
                        is_streaming,
                        status: RequestStatus::Error,
                        http_status: match &err {
                            OAuthDispatchError::Upstream { status, .. } => *status,
                            _ => Some(401),
                        },
                        ttft_ms: None,
                        total_latency_ms: Some(start.elapsed().as_millis() as u64),
                        upstream_input_tokens: None,
                        upstream_output_tokens: None,
                        upstream_cache_creation: None,
                        upstream_cache_read: None,
                        retry_count,
                        error_message: Some(err_msg.clone()),
                        upstream_response_body: Some(truncate_body(&err_msg, ERROR_BODY_LIMIT)),
                    };
                    let _ = state.request_log_tx.try_send(entry);
                    events::record_request(
                        &state.event_log_tx,
                        attempt_id,
                        sub_id,
                        Severity::Error,
                        format!(
                            "{} · {} · Kiro · {}",
                            vm_name.as_str(),
                            display_name,
                            err_msg
                        ),
                    );

                    if retryable {
                        retry_count += 1;
                        continue;
                    }
                    return Ok(oauth_dispatch::build_error_response(&err));
                }
            }
        }

        // fallback 透传原始 body, 不必 clone JSON, 直接序列化引用; 其他三个虚拟模型按
        // 订阅 slot 改写 model 字段, 必须在 clone 上改。
        let serialized_body = if is_fallback {
            serde_json::to_vec(&request_body)?
        } else {
            let mut upstream_body = request_body.clone();
            upstream_body["model"] = Value::String(real_model.clone());
            serde_json::to_vec(&upstream_body)?
        };

        // 调试模式: 把客户端原始请求体 + cc-router 改写后的出站请求体写盘.
        // channel 满时 try_send 失败也不影响主路径.
        if debug_mode {
            if let Some(client_bytes) = client_bytes_cached.as_ref() {
                let _ = state.body_dump_tx.try_send(BodyDumpEntry::new(
                    attempt_id,
                    BodyDumpKind::Client,
                    client_bytes.clone(),
                ));
            }
            let _ = state.body_dump_tx.try_send(BodyDumpEntry::new(
                attempt_id,
                BodyDumpKind::UpstreamRequest,
                serialized_body.clone(),
            ));
        }

        let mut upstream_headers = ReqHeaderMap::new();
        if let (Ok(name), Ok(value)) = (
            ReqHeaderName::try_from(auth_header_name.as_str()),
            ReqHeaderValue::from_str(&auth_header_value),
        ) {
            upstream_headers.insert(name, value);
        }
        // required headers (从订阅 snapshot 读)
        for (k, v) in required_headers.iter() {
            if let (Ok(name), Ok(value)) = (
                ReqHeaderName::try_from(k.as_str()),
                ReqHeaderValue::from_str(v),
            ) {
                upstream_headers.insert(name, value);
            }
        }
        // forward headers (白名单, 从订阅 snapshot 读)
        for fwd in forward_headers.iter() {
            if let Some(val) = client_headers.get(fwd.as_str()) {
                if let (Ok(name), Ok(value)) = (
                    ReqHeaderName::try_from(fwd.as_str()),
                    ReqHeaderValue::from_bytes(val.as_bytes()),
                ) {
                    upstream_headers.insert(name, value);
                }
            }
        }
        // content-type: application/json
        upstream_headers.insert(
            reqwest::header::CONTENT_TYPE,
            ReqHeaderValue::from_static("application/json"),
        );

        info!(
            %dispatch_id,
            %attempt_id,
            %sub_id,
            %display_name,
            %real_model,
            url = %url,
            "forwarding to upstream"
        );

        emit_attempt_started(state, sub_id, vm_name);

        let upstream_result = upstream::send(
            &state.http_client,
            &url,
            serialized_body,
            upstream_headers,
            is_streaming,
        )
        .await;

        match upstream_result {
            Ok(upstream::UpstreamResponse::NonStreaming {
                status,
                headers: resp_headers,
                body: resp_body,
                body_text,
            }) => {
                emit_attempt_finished(state, sub_id, vm_name, status.is_success());
                // 调试模式: dump 上游真实响应体. 优先 body_text(原始字节, 错误路径有),
                // 其次 resp_body 序列化(成功路径 JSON Value).
                if debug_mode {
                    let bytes = body_text
                        .as_ref()
                        .map(|s| s.as_bytes().to_vec())
                        .unwrap_or_else(|| serde_json::to_vec(&resp_body).unwrap_or_default());
                    let _ = state.body_dump_tx.try_send(BodyDumpEntry::new(
                        attempt_id,
                        BodyDumpKind::UpstreamResponse,
                        bytes,
                    ));
                }
                let should_retry = classify_response(status.as_u16(), None);
                // 状态机事件
                let event = match should_retry {
                    ShouldRetry::Yes(_) => state_machine::Event::HttpStatus(status.as_u16()),
                    ShouldRetry::No => {
                        if status.is_success() {
                            state_machine::Event::RequestSucceeded
                        } else {
                            state_machine::Event::HttpStatus(status.as_u16())
                        }
                    }
                };
                let _ = state_machine::apply(
                    &state.db,
                    &state.app_handle,
                    &state.event_log_tx,
                    rt.clone(),
                    event,
                )
                .await;

                let is_success = status.is_success();
                let response_model_name = resp_body
                    .get("model")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let (req_status, error_message, upstream_body_log) = if is_success {
                    (RequestStatus::Success, None, None)
                } else {
                    (
                        RequestStatus::Error,
                        Some(format!("HTTP {}", status.as_u16())),
                        body_text.as_deref().map(|s| truncate_body(s, ERROR_BODY_LIMIT)),
                    )
                };

                // 改写响应 model 字段：真实名 → 虚拟名（§5.4）；fallback 透传不改写
                let final_body = if is_success && !is_fallback {
                    rewrite_response_model(resp_body, vm_name.as_str())
                } else {
                    resp_body
                };

                let entry = RequestLogEntry {
                    id: attempt_id,
                    timestamp_ms: chrono::Utc::now().timestamp_millis(),
                    virtual_model_name: vm_name,
                    subscription_id: sub_id,
                    provider_id: provider_id.clone(),
                    endpoint_id: endpoint_id.clone(),
                    real_model_name: real_model.clone(),
                    response_model_name,
                    is_streaming: false,
                    status: req_status,
                    http_status: Some(status.as_u16()),
                    ttft_ms: None,
                    total_latency_ms: Some(start.elapsed().as_millis() as u64),
                    upstream_input_tokens: extract_usage(&final_body, "input_tokens"),
                    upstream_output_tokens: extract_usage(&final_body, "output_tokens"),
                    upstream_cache_creation: extract_usage(&final_body, "cache_creation_input_tokens"),
                    upstream_cache_read: extract_usage(&final_body, "cache_read_input_tokens"),
                    retry_count,
                    error_message: error_message.clone(),
                    upstream_response_body: upstream_body_log,
                };
                let _ = state.request_log_tx.try_send(entry);

                let event_summary = if is_success {
                    format!("{} · {} · {}", vm_name.as_str(), display_name, real_model)
                } else {
                    format!(
                        "{} · {} · {} HTTP {}",
                        vm_name.as_str(),
                        display_name,
                        real_model,
                        status.as_u16()
                    )
                };
                let event_severity = if is_success {
                    Severity::Info
                } else {
                    Severity::Error
                };
                events::record_request(
                    &state.event_log_tx,
                    attempt_id,
                    sub_id,
                    event_severity,
                    event_summary,
                );

                if let ShouldRetry::Yes(_) = should_retry {
                    retry_count += 1;
                    continue;
                }

                return Ok(build_non_streaming_response(status, resp_headers, final_body));
            }
            Ok(upstream::UpstreamResponse::Streaming {
                status,
                headers: resp_headers,
                stream,
            }) => {
                // 流式：若非 2xx 按非流式错误处理
                if !status.is_success() {
                    emit_attempt_finished(state, sub_id, vm_name, false);
                    let event = state_machine::Event::HttpStatus(status.as_u16());
                    let _ = state_machine::apply(
                        &state.db,
                        &state.app_handle,
                        &state.event_log_tx,
                        rt.clone(),
                        event,
                    )
                    .await;
                    if matches!(
                        classify_response(status.as_u16(), None),
                        ShouldRetry::Yes(_)
                    ) {
                        retry_count += 1;
                        continue;
                    } else {
                        return Ok(build_error_status(status));
                    }
                }

                // 智谱等上游用 200 + `event: error` 表达限流; 不 peek 会被透传成假成功.
                let peek = sse::peek_first_event(stream).await;
                match peek {
                    sse::PeekResult::UpstreamError {
                        code,
                        message,
                        raw_bytes,
                    } => {
                        // 仅对智谱按已知 1308 / 5h 文案判长冷却; 其他 provider 一律视为短期速率限制.
                        let is_quota_exhausted = provider_id == "zhipu"
                            && sse::classify_zhipu_sse_error(code.as_deref(), message.as_deref());
                        emit_attempt_finished(state, sub_id, vm_name, false);
                        let _ = state_machine::apply(
                            &state.db,
                            &state.app_handle,
                            &state.event_log_tx,
                            rt.clone(),
                            state_machine::Event::UpstreamSseError { is_quota_exhausted },
                        )
                        .await;

                        let err_summary = format!(
                            "{}: {}",
                            code.as_deref().unwrap_or("?"),
                            message.as_deref().unwrap_or("(no message)")
                        );

                        if debug_mode {
                            let _ = state.body_dump_tx.try_send(BodyDumpEntry::new(
                                attempt_id,
                                BodyDumpKind::UpstreamResponse,
                                raw_bytes.to_vec(),
                            ));
                        }

                        let entry = RequestLogEntry {
                            id: attempt_id,
                            timestamp_ms: chrono::Utc::now().timestamp_millis(),
                            virtual_model_name: vm_name,
                            subscription_id: sub_id,
                            provider_id: provider_id.clone(),
                            endpoint_id: endpoint_id.clone(),
                            real_model_name: real_model.clone(),
                            response_model_name: None,
                            is_streaming: true,
                            status: RequestStatus::Error,
                            // HTTP 仍然是 200, 但语义层是错误; error_message 区分
                            http_status: Some(200),
                            ttft_ms: None,
                            total_latency_ms: Some(start.elapsed().as_millis() as u64),
                            upstream_input_tokens: None,
                            upstream_output_tokens: None,
                            upstream_cache_creation: None,
                            upstream_cache_read: None,
                            retry_count,
                            error_message: Some(format!("SSE error {}", err_summary)),
                            upstream_response_body: Some(truncate_body(
                                &String::from_utf8_lossy(&raw_bytes),
                                ERROR_BODY_LIMIT,
                            )),
                        };
                        let _ = state.request_log_tx.try_send(entry);
                        events::record_request(
                            &state.event_log_tx,
                            attempt_id,
                            sub_id,
                            Severity::Error,
                            format!(
                                "{} · {} · {} SSE {}",
                                vm_name.as_str(),
                                display_name,
                                real_model,
                                err_summary
                            ),
                        );

                        retry_count += 1;
                        continue;
                    }
                    sse::PeekResult::Network(e) => {
                        emit_attempt_finished(state, sub_id, vm_name, false);
                        warn!(?e, "upstream stream error during lookahead");
                        let _ = state_machine::apply(
                            &state.db,
                            &state.app_handle,
                            &state.event_log_tx,
                            rt.clone(),
                            state_machine::Event::NetworkError,
                        )
                        .await;
                        let err_msg = format!("流首 lookahead 网络错误: {}", e);
                        let entry = RequestLogEntry {
                            id: attempt_id,
                            timestamp_ms: chrono::Utc::now().timestamp_millis(),
                            virtual_model_name: vm_name,
                            subscription_id: sub_id,
                            provider_id: provider_id.clone(),
                            endpoint_id: endpoint_id.clone(),
                            real_model_name: real_model.clone(),
                            response_model_name: None,
                            is_streaming: true,
                            status: RequestStatus::Error,
                            http_status: Some(200),
                            ttft_ms: None,
                            total_latency_ms: Some(start.elapsed().as_millis() as u64),
                            upstream_input_tokens: None,
                            upstream_output_tokens: None,
                            upstream_cache_creation: None,
                            upstream_cache_read: None,
                            retry_count,
                            error_message: Some(err_msg.clone()),
                            upstream_response_body: None,
                        };
                        let _ = state.request_log_tx.try_send(entry);
                        events::record_request(
                            &state.event_log_tx,
                            attempt_id,
                            sub_id,
                            Severity::Error,
                            format!(
                                "{} · {} · {} {}",
                                vm_name.as_str(),
                                display_name,
                                real_model,
                                err_msg
                            ),
                        );
                        retry_count += 1;
                        continue;
                    }
                    sse::PeekResult::Malformed(bytes) => {
                        // 罕见: 上游返了 SSE Content-Type 但流首格式不对 / 提前结束.
                        // 保守: 标 NetworkError + retry 切下家, body_dump 留证.
                        emit_attempt_finished(state, sub_id, vm_name, false);
                        warn!(
                            "stream first event malformed (len={}), retrying",
                            bytes.len()
                        );
                        let _ = state_machine::apply(
                            &state.db,
                            &state.app_handle,
                            &state.event_log_tx,
                            rt.clone(),
                            state_machine::Event::NetworkError,
                        )
                        .await;
                        if debug_mode && !bytes.is_empty() {
                            let _ = state.body_dump_tx.try_send(BodyDumpEntry::new(
                                attempt_id,
                                BodyDumpKind::UpstreamResponse,
                                bytes.to_vec(),
                            ));
                        }
                        retry_count += 1;
                        continue;
                    }
                    sse::PeekResult::Ok {
                        stream,
                        first_byte_at,
                    } => {
                        emit_attempt_finished(state, sub_id, vm_name, true);
                        let _ = state_machine::apply(
                            &state.db,
                            &state.app_handle,
                            &state.event_log_tx,
                            rt.clone(),
                            state_machine::Event::RequestSucceeded,
                        )
                        .await;

                        let log_tx = state.request_log_tx.clone();
                        let event_tx = state.event_log_tx.clone();
                        let dump_tx = if debug_mode {
                            Some(state.body_dump_tx.clone())
                        } else {
                            None
                        };
                        let response = sse::stream_response(
                            resp_headers,
                            stream,
                            vm_name,
                            attempt_id,
                            sub_id,
                            provider_id,
                            endpoint_id,
                            real_model,
                            display_name,
                            retry_count,
                            start,
                            log_tx,
                            event_tx,
                            state.db.clone(),
                            state.app_handle.clone(),
                            rt.clone(),
                            dump_tx,
                            Some(first_byte_at),
                        );
                        return Ok(response);
                    }
                }
            }
            Err(upstream::UpstreamError::Reqwest(e)) => {
                emit_attempt_finished(state, sub_id, vm_name, false);
                warn!(?e, "upstream network error");
                let _ = state_machine::apply(
                    &state.db,
                    &state.app_handle,
                    &state.event_log_tx,
                    rt.clone(),
                    state_machine::Event::NetworkError,
                )
                .await;

                let err_msg = format!("网络错误: {}", e);
                let entry = RequestLogEntry {
                    id: attempt_id,
                    timestamp_ms: chrono::Utc::now().timestamp_millis(),
                    virtual_model_name: vm_name,
                    subscription_id: sub_id,
                    provider_id: provider_id.clone(),
                    endpoint_id: endpoint_id.clone(),
                    real_model_name: real_model.clone(),
                    response_model_name: None,
                    is_streaming,
                    status: RequestStatus::Error,
                    http_status: None,
                    ttft_ms: None,
                    total_latency_ms: Some(start.elapsed().as_millis() as u64),
                    upstream_input_tokens: None,
                    upstream_output_tokens: None,
                    upstream_cache_creation: None,
                    upstream_cache_read: None,
                    retry_count,
                    error_message: Some(err_msg.clone()),
                    upstream_response_body: None,
                };
                let _ = state.request_log_tx.try_send(entry);

                events::record_request(
                    &state.event_log_tx,
                    attempt_id,
                    sub_id,
                    Severity::Error,
                    format!(
                        "{} · {} · {} {}",
                        vm_name.as_str(),
                        display_name,
                        real_model,
                        err_msg
                    ),
                );

                retry_count += 1;
                continue;
            }
        }
    }

    // 所有候选都失败
    let subs_map = state.subscriptions.read().await;
    let mut summary = Vec::new();
    for sub_id in &vm_config.subscription_ids {
        if let Some(rt) = subs_map.get(sub_id) {
            let g = rt.read().await;
            summary.push(format!("- {}: {:?}", g.row.display_name, g.state));
        }
    }
    drop(subs_map);

    events::record_system_error(
        &state.event_log_tx,
        format!("虚拟模型 {} 全部候选不可用", vm_name.as_str()),
        Some(serde_json::json!({
            "virtual_model": vm_name.as_str(),
            "dispatch_id": dispatch_id.to_string(),
            "candidates": summary,
        })),
    );

    Ok(overloaded::response_with_summary(vm_name, &summary))
}

fn rewrite_response_model(mut body: Value, virtual_name: &str) -> Value {
    if body.get("model").is_some() {
        body["model"] = Value::String(virtual_name.to_string());
    }
    body
}

fn extract_usage(body: &Value, key: &str) -> Option<u32> {
    body.get("usage")
        .and_then(|u| u.get(key))
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
}

fn build_non_streaming_response(
    status: StatusCode,
    upstream_headers: reqwest::header::HeaderMap,
    body: Value,
) -> Response {
    let body_bytes = match serde_json::to_vec(&body) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(error_body("internal_server_error", &e.to_string())),
            )
                .into_response();
        }
    };
    let mut response = Response::new(axum::body::Body::from(body_bytes));
    *response.status_mut() = status;
    let hdrs = response.headers_mut();
    if let Some(ct) = upstream_headers.get(reqwest::header::CONTENT_TYPE) {
        if let Ok(name) = HeaderName::try_from("content-type") {
            if let Ok(value) = HeaderValue::from_bytes(ct.as_bytes()) {
                hdrs.insert(name, value);
            }
        }
    } else {
        hdrs.insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }
    response
}

fn build_error_status(status: reqwest::StatusCode) -> Response {
    let axum_status = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    (
        axum_status,
        Json(error_body(
            "upstream_error",
            &format!("upstream returned {}", status.as_u16()),
        )),
    )
        .into_response()
}

