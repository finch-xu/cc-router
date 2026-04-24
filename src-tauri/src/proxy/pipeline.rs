//! 请求 pipeline（设计稿 §5.1 步骤 1-6 + 重试）。

use std::time::Instant;

use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use reqwest::header::{HeaderMap as ReqHeaderMap, HeaderName as ReqHeaderName, HeaderValue as ReqHeaderValue};
use serde_json::Value;
use tracing::{info, warn};
use uuid::Uuid;

use crate::error::AppResult;
use crate::observability::request_log::{RequestLogEntry, RequestStatus};
use crate::provider::model::AuthHeaderFormat;
use crate::proxy::handler::error_body;
use crate::proxy::overloaded;
use crate::proxy::retry::{classify_response, ShouldRetry};
use crate::proxy::sse;
use crate::proxy::upstream;
use crate::state::AppState;
use crate::subscription::state_machine;
use crate::virtual_model::scheduler::build_candidate_order;
use crate::virtual_model::VirtualModelName;

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
    let request_id = Uuid::new_v4();

    for sub_id in order.candidate_ids {
        let rt = {
            let subs_map = state.subscriptions.read().await;
            subs_map.get(&sub_id).cloned()
        };
        let Some(rt) = rt else { continue };

        let (provider_id, endpoint_id, real_model, display_name, api_key) = {
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
                guard.row.api_key.clone(),
            )
        };

        let provider = match state.providers.get(&provider_id) {
            Some(p) => p.clone(),
            None => {
                warn!(%provider_id, "provider 未加载, 跳过订阅");
                continue;
            }
        };
        let endpoint = match provider.endpoint(&endpoint_id) {
            Some(e) => e.clone(),
            None => {
                warn!(%endpoint_id, "endpoint 未找到, 跳过订阅");
                continue;
            }
        };

        // 构造上游请求
        let url = format!(
            "{}{}",
            endpoint.base_url.trim_end_matches('/'),
            if endpoint.messages_path.starts_with('/') {
                endpoint.messages_path.clone()
            } else {
                format!("/{}", endpoint.messages_path)
            }
        );

        let mut upstream_body = request_body.clone();
        // fallback 不改写 model 字段（原始 model 即 real_model, body 里已经有）
        if !is_fallback {
            upstream_body["model"] = Value::String(real_model.clone());
        }
        let serialized_body = serde_json::to_vec(&upstream_body)?;

        let mut upstream_headers = ReqHeaderMap::new();
        // auth
        let auth_value = match provider.auth.header_format {
            AuthHeaderFormat::Bearer => format!("Bearer {api_key}"),
            AuthHeaderFormat::Raw => api_key.clone(),
        };
        if let (Ok(name), Ok(value)) = (
            ReqHeaderName::try_from(provider.auth.header_name.as_str()),
            ReqHeaderValue::from_str(&auth_value),
        ) {
            upstream_headers.insert(name, value);
        }
        // required headers
        for (k, v) in provider.required_headers.iter() {
            if let (Ok(name), Ok(value)) = (
                ReqHeaderName::try_from(k.as_str()),
                ReqHeaderValue::from_str(v),
            ) {
                upstream_headers.insert(name, value);
            }
        }
        // forward headers (白名单)
        for fwd in provider.forward_headers.iter() {
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
            %request_id,
            %sub_id,
            %display_name,
            %real_model,
            url = %url,
            "forwarding to upstream"
        );

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
            }) => {
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
                let _ = state_machine::apply(&state.db, &state.app_handle, rt.clone(), event).await;

                if let ShouldRetry::Yes(_) = should_retry {
                    retry_count += 1;
                    continue;
                }

                // 改写响应 model 字段：真实名 → 虚拟名（§5.4）；fallback 透传不改写
                let final_body = if is_fallback {
                    resp_body
                } else {
                    rewrite_response_model(resp_body, vm_name.as_str())
                };

                // 日志
                let entry = RequestLogEntry {
                    id: request_id,
                    timestamp_ms: chrono::Utc::now().timestamp_millis(),
                    virtual_model_name: vm_name,
                    subscription_id: sub_id,
                    provider_id: provider_id.clone(),
                    endpoint_id: endpoint_id.clone(),
                    real_model_name: real_model.clone(),
                    is_streaming: false,
                    status: if status.is_success() {
                        RequestStatus::Success
                    } else {
                        RequestStatus::Error
                    },
                    http_status: Some(status.as_u16()),
                    ttft_ms: None,
                    total_latency_ms: Some(start.elapsed().as_millis() as u64),
                    upstream_input_tokens: extract_usage(&final_body, "input_tokens"),
                    upstream_output_tokens: extract_usage(&final_body, "output_tokens"),
                    upstream_cache_creation: extract_usage(&final_body, "cache_creation_input_tokens"),
                    upstream_cache_read: extract_usage(&final_body, "cache_read_input_tokens"),
                    retry_count,
                    error_message: None,
                };
                let _ = state.request_log_tx.try_send(entry);

                return Ok(build_non_streaming_response(status, resp_headers, final_body));
            }
            Ok(upstream::UpstreamResponse::Streaming {
                status,
                headers: resp_headers,
                stream,
            }) => {
                // 流式：若非 2xx 按非流式错误处理
                if !status.is_success() {
                    let event = state_machine::Event::HttpStatus(status.as_u16());
                    let _ =
                        state_machine::apply(&state.db, &state.app_handle, rt.clone(), event).await;
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

                let _ = state_machine::apply(
                    &state.db,
                    &state.app_handle,
                    rt.clone(),
                    state_machine::Event::RequestSucceeded,
                )
                .await;

                let log_tx = state.request_log_tx.clone();
                let response = sse::stream_response(
                    resp_headers,
                    stream,
                    vm_name,
                    request_id,
                    sub_id,
                    provider_id,
                    endpoint_id,
                    real_model,
                    retry_count,
                    start,
                    log_tx,
                );
                return Ok(response);
            }
            Err(upstream::UpstreamError::Reqwest(e)) => {
                warn!(?e, "upstream network error");
                let _ = state_machine::apply(
                    &state.db,
                    &state.app_handle,
                    rt.clone(),
                    state_machine::Event::NetworkError,
                )
                .await;
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
