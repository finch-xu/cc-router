//! 请求 pipeline（设计稿 §5.1 步骤 1-6 + 重试）。

use std::time::Instant;

use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use reqwest::header::{HeaderMap as ReqHeaderMap, HeaderName as ReqHeaderName, HeaderValue as ReqHeaderValue};
use serde_json::{json, Value};
use tauri::Emitter;
use tracing::{info, warn};
use uuid::Uuid;

use crate::error::AppResult;
use crate::observability::events::{self, Severity};
use crate::observability::request_log::{RequestLogEntry, RequestStatus};
use crate::proxy::handler::error_body;
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
            supports_thinking,
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
                guard.row.supports_thinking_blocks,
            )
        };

        let mut upstream_body = request_body.clone();
        // fallback 不改写 model 字段（原始 model 即 real_model, body 里已经有）
        if !is_fallback {
            upstream_body["model"] = Value::String(real_model.clone());
        }
        // 不支持 thinking 的订阅: 剥离顶层 thinking 字段 + messages 历史中的 thinking 块,
        // 避免历史中残留的 thinking 块被发到不识别的上游导致 400。
        if !supports_thinking {
            strip_thinking(&mut upstream_body);
        }
        // DeepSeek anthropic 兼容层硬性约束（实测 2026-04-30）:
        //
        // deepseek-v4-pro 默认开启 thinking 模式. 在多轮 + tool_use 场景下,
        // 上游严格要求 assistant 历史必须保留之前模型自己产生的 thinking 块,
        // 否则返回 400: "The `content[].thinking` in the thinking mode must be
        // passed back to the API."（与 Anthropic 官方协议一致, 见官方 extended-thinking 文档）.
        //
        // 这在 cc-router round_robin 跨订阅场景被天然破坏: 如上一轮路由到不返
        // thinking 块的家（如某些中转网关上的 claude-opus-4-6 实测不返 thinking）,
        // 客户端拿到的 assistant 历史就缺 thinking 块. 下一轮 round_robin 路由到
        // deepseek 时, deepseek 看不到历史 thinking 块即触发 400, cc-router 内部
        // 重试切下一家成功, 但 UI 历史里会留 deepseek 失败 attempt 记录.
        //
        // Fix: 历史无 thinking 块时主动注入 `thinking: {type:"disabled"}` 让 deepseek
        // 跳过 thinking 模式严格校验. 不动客户端已显式设置的 thinking 字段.
        // 详见 apply_deepseek_thinking_quirk 函数注释 + tests/proxy_e2e.rs 的两个 e2e.
        //
        // 为什么硬编码 provider_id=="deepseek" 而非新增 capability: 当前只一家有此 quirk,
        // 抽象会重蹈 v4 `thinking_block_field_name` 覆辙（issue #3 教训, 第一个使用者就错）.
        if provider_id == "deepseek" {
            apply_deepseek_thinking_quirk(&mut upstream_body);
        }
        let serialized_body = serde_json::to_vec(&upstream_body)?;

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
                emit_attempt_finished(state, sub_id, vm_name, status.is_success());
                // 流式：若非 2xx 按非流式错误处理
                if !status.is_success() {
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
                );
                return Ok(response);
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

/// 剥离请求体里与 Anthropic extended thinking 协议相关的部分:
///
/// 1. 顶层 `thinking` 字段 (启用开关 `{type: "enabled", budget_tokens: ...}`)
/// 2. `messages[].content[]` 数组中 `type == "thinking"` 或 `type == "redacted_thinking"` 的块
///
/// 触发条件: 订阅的 `supports_thinking_blocks` 为 false。
///
/// 背景: Claude Code 在收到上游返回的 thinking 块后,会把它存进 assistant 历史,下一轮请求会
/// 推回到 messages 数组。如果这一轮路由到不支持 extended thinking 的 provider, 上游会以
/// 「thinking 块存在但 thinking 模式未启用」为由返回 400。剥离干净避免这个循环。
fn strip_thinking(body: &mut Value) {
    if let Some(obj) = body.as_object_mut() {
        obj.remove("thinking");
    }
    let Some(messages) = body.get_mut("messages").and_then(|v| v.as_array_mut()) else {
        return;
    };
    for msg in messages {
        let Some(content) = msg.get_mut("content") else {
            continue;
        };
        let Some(arr) = content.as_array_mut() else {
            continue; // 字符串形式的 content 不动
        };
        arr.retain(|block| {
            !matches!(
                block.get("type").and_then(|t| t.as_str()),
                Some("thinking") | Some("redacted_thinking")
            )
        });
    }
}

/// DeepSeek anthropic 兼容层 thinking 模式严格性 quirk 修复.
///
/// # 现象（实测 2026-04-30, 8 个 curl 实验交叉验证）
///
/// | 场景 | 不加 thinking 开关 | 加 `thinking:{type:disabled}` |
/// |---|---|---|
/// | 单轮 user message | 200, 默认返 thinking+text | 200, 只返 text |
/// | 多轮纯文本（assistant 无 thinking 块） | **200**（deepseek 宽容） | 200 |
/// | 多轮 + tool_use（assistant 无 thinking 块） | **400** ★ | **200** |
/// | 多轮 + tool_use（assistant 含原样 thinking 块） | 200 | 200 |
///
/// 400 错误体:
/// ```json
/// {"error":{"message":"The `content[].thinking` in the thinking mode must be passed back to the API.","type":"invalid_request_error"}}
/// ```
///
/// # 为什么 cc-router 会触发
///
/// deepseek 默认开启 thinking 模式. round_robin 跨订阅时, 上一轮路由到不返 thinking
/// 的家（如某些中转网关上的 claude-opus-4-6）, 客户端拿到的 assistant 历史就缺
/// thinking 块. 下一轮路由到 deepseek 时, deepseek 严格校验 → 400.
///
/// # 修复策略
///
/// 主动注入 `thinking: {"type": "disabled"}` 让 deepseek 跳过 thinking 模式严格校验.
/// 这是 deepseek 协议官方支持的开关（注意: `"thinking": false` 会被 schema 拒为
/// `invalid type: boolean, expected ThinkingOptions`, 必须用对象形式）.
///
/// # 触发条件
///
/// - messages 历史的 assistant 消息中**全部不含** thinking 块
/// - **且**客户端**未显式设置**顶层 `thinking` 字段（尊重客户端选择, 不覆盖）
///
/// # 不影响项
///
/// - 单轮请求 → 默认进入: 没 assistant 历史 → 注入 disable. 想要 thinking 的客户端
///   自己设 `thinking:{type:enabled}` 即可（被尊重不覆盖）.
/// - 多轮纯文本无 thinking → 也注入 disable, 无副作用（实测 deepseek 接受）.
/// - 多轮历史含 thinking 块（即用户连续打 deepseek 拿到的 thinking 原样回传）→
///   不注入 disable, deepseek 正常 thinking 模式继续.
pub fn apply_deepseek_thinking_quirk(body: &mut Value) {
    if body.get("thinking").is_some() {
        return;
    }
    if !history_lacks_thinking_block(body) {
        return;
    }
    if let Some(obj) = body.as_object_mut() {
        obj.insert("thinking".to_string(), json!({"type": "disabled"}));
    }
}

/// 判断 messages 历史里 assistant 消息是否完全不含 thinking 块。
/// - 任何 assistant 消息 content 数组里有 type=="thinking" 块 → false
/// - 所有 assistant 消息都不含 thinking 块（含空 messages 数组）→ true
fn history_lacks_thinking_block(body: &Value) -> bool {
    let Some(messages) = body.get("messages").and_then(|v| v.as_array()) else {
        return true;
    };
    for msg in messages {
        if msg.get("role").and_then(|r| r.as_str()) != Some("assistant") {
            continue;
        }
        let Some(content) = msg.get("content").and_then(|c| c.as_array()) else {
            continue;
        };
        for block in content {
            if block.get("type").and_then(|t| t.as_str()) == Some("thinking") {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{apply_deepseek_thinking_quirk, history_lacks_thinking_block, strip_thinking};
    use serde_json::json;

    #[test]
    fn removes_top_level_thinking_field() {
        let mut body = json!({
            "model": "claude-opus-4-7",
            "thinking": { "type": "enabled", "budget_tokens": 1024 },
            "messages": []
        });
        strip_thinking(&mut body);
        assert!(body.get("thinking").is_none());
        assert!(body.get("model").is_some());
    }

    #[test]
    fn removes_thinking_blocks_in_assistant_messages() {
        let mut body = json!({
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "hi" }] },
                {
                    "role": "assistant",
                    "content": [
                        { "type": "thinking", "thinking": "let me think...", "signature": "abc" },
                        { "type": "text", "text": "hello" },
                        { "type": "redacted_thinking", "data": "encrypted" }
                    ]
                }
            ]
        });
        strip_thinking(&mut body);
        let assistant_content = body["messages"][1]["content"].as_array().unwrap();
        assert_eq!(assistant_content.len(), 1);
        assert_eq!(assistant_content[0]["type"], "text");
    }

    #[test]
    fn preserves_text_and_tool_blocks() {
        let mut body = json!({
            "messages": [{
                "role": "assistant",
                "content": [
                    { "type": "text", "text": "answer" },
                    { "type": "tool_use", "id": "x", "name": "calc", "input": {} }
                ]
            }]
        });
        strip_thinking(&mut body);
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
    }

    #[test]
    fn does_not_break_string_content() {
        // 用户消息常用字符串形式的 content
        let mut body = json!({
            "messages": [
                { "role": "user", "content": "hello" }
            ]
        });
        strip_thinking(&mut body);
        assert_eq!(body["messages"][0]["content"], "hello");
    }

    #[test]
    fn empty_or_missing_messages_is_safe() {
        let mut body = json!({ "model": "x" });
        strip_thinking(&mut body);
        assert!(body.get("messages").is_none());

        let mut body2 = json!({ "messages": [] });
        strip_thinking(&mut body2);
        assert_eq!(body2["messages"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn deepseek_quirk_injects_disable_when_no_thinking_history() {
        let mut body = json!({
            "messages": [
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "hello"},
                    {"type": "tool_use", "id": "1", "name": "x", "input": {}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "1", "content": "ok"}
                ]}
            ]
        });
        apply_deepseek_thinking_quirk(&mut body);
        assert_eq!(body["thinking"], json!({"type": "disabled"}));
    }

    #[test]
    fn deepseek_quirk_skips_when_history_has_thinking() {
        let mut body = json!({
            "messages": [
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "...", "signature": "abc"},
                    {"type": "text", "text": "hello"}
                ]}
            ]
        });
        apply_deepseek_thinking_quirk(&mut body);
        assert!(
            body.get("thinking").is_none(),
            "客户端没设 thinking 且历史有 thinking 块时不应注入"
        );
    }

    #[test]
    fn deepseek_quirk_respects_client_provided_thinking() {
        let mut body = json!({
            "thinking": {"type": "enabled", "budget_tokens": 1024},
            "messages": [{"role": "user", "content": "hi"}]
        });
        apply_deepseek_thinking_quirk(&mut body);
        assert_eq!(
            body["thinking"],
            json!({"type": "enabled", "budget_tokens": 1024}),
            "客户端显式设了 thinking 字段不应被覆盖"
        );
    }

    #[test]
    fn deepseek_quirk_handles_single_user_message() {
        let mut body = json!({
            "messages": [{"role": "user", "content": "hi"}]
        });
        apply_deepseek_thinking_quirk(&mut body);
        assert_eq!(body["thinking"], json!({"type": "disabled"}));
    }

    #[test]
    fn history_lacks_thinking_block_detects_present_thinking() {
        let body = json!({
            "messages": [
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "x", "signature": "s"}
                ]}
            ]
        });
        assert!(!history_lacks_thinking_block(&body));
    }

    #[test]
    fn history_lacks_thinking_block_ignores_string_content() {
        let body = json!({
            "messages": [
                {"role": "assistant", "content": "hi"},
                {"role": "user", "content": "ok"}
            ]
        });
        assert!(history_lacks_thinking_block(&body));
    }

    #[test]
    fn history_lacks_thinking_block_ignores_user_messages() {
        // user 消息里出现 thinking 块（违反协议但要稳健）, 不应影响判定
        let body = json!({
            "messages": [
                {"role": "user", "content": [
                    {"type": "thinking", "thinking": "stray", "signature": "x"}
                ]}
            ]
        });
        assert!(history_lacks_thinking_block(&body));
    }
}
