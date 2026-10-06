//! `POST /v1/systemone`: Jev (System One 决策协议) 的多上游网关。
//!
//! 协议原样透传、不翻译; 只服务 `model-jev` 虚拟模型, 只调度端点协议为 systemone 的订阅。
//! 候选循环 [`run_candidates`] 不碰 `AppState`: 发请求 / 状态机 / 日志都经 [`AttemptSink`]
//! 注入, 生产实现在 Task 8 的 `LiveSink`, 测试用假实现。
//!
//! 错误分类直接复用对话路径的 [`classify_response`] (4xx 切下家不冷却): 实测三家上游对同一份
//! 请求的接受范围不同 (未知模型 400 / 400 / 404, `stream:true` 一家 400 两家 200), 「换一家可能
//! 成功」对 Jev 同样成立。

use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
use futures::future::BoxFuture;
use reqwest::header::{
    HeaderMap as ReqHeaderMap, HeaderName as ReqHeaderName, HeaderValue as ReqHeaderValue,
    CONTENT_TYPE,
};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::observability::request_log::{RequestLogEntry, RequestStatus};
use crate::proxy::client_fingerprint::ClientContext;
use crate::proxy::pipeline::{truncate_body, ERROR_BODY_LIMIT};
use crate::proxy::retry::{classify_response, ShouldRetry};
use crate::proxy::tool_log::ToolLogFields;
use crate::subscription::model::{SubscriptionRow, SubscriptionRuntime};
use crate::subscription::state_machine;
use crate::virtual_model::VirtualModelName;
use std::time::Instant;

use chrono::Utc;
use tracing::{info, warn};

use crate::observability::events;
use crate::provider::model::{EndpointProtocol, SystemoneWire};
use crate::proxy::pipeline::{emit_attempt_finished, emit_attempt_started};
use crate::proxy::upstream;
use crate::state::AppState;
use crate::virtual_model::scheduler::build_candidate_order;
use crate::virtual_model::RoutingMode;

/// 一次出站请求的全部内容, 由订阅快照 + 客户端 body 纯函数算出 (探测复用同一个函数)。
#[derive(Debug, Clone)]
pub struct Outbound {
    pub url: String,
    pub headers: ReqHeaderMap,
    pub body: Bytes,
    /// 实际发往上游的 model (日志 `real_model_name`)。
    pub real_model: String,
    /// 是否改写过 `body.model`; 决定成功响应的 model 是否恢复成客户端原值。
    pub rewritten: bool,
    /// 上游响应的外形 (决定 [`normalize_response`] 是否拆信封)。
    pub wire: SystemoneWire,
}

/// 发往上游的 model。客户端写虚拟名 `model-jev` (可带 `anthropic/` / `openai/` 前缀) 时
/// 与 `model-opus` → opus 槽同理: 用订阅的 Jev 槽, 槽为空则退到端点示例模型 (创建时快照进
/// `model_discovery.example_models`) —— 虚拟名原样发出去上游必然不认识。客户端写真实模型名时,
/// Jev 槽非空就改写成槽值, 否则透传。返回 None = 原样转发。
fn target_model<'a>(row: &'a SubscriptionRow, client_model: &str) -> Option<&'a str> {
    let slot = row.model_slots.jev_model();
    if VirtualModelName::parse(client_model) == Some(VirtualModelName::Jev) {
        return slot.or_else(|| row.model_discovery.example_models.first().map(String::as_str));
    }
    slot
}

/// 目标 model 与客户端 model 不同时改写 `body.model` (重序列化, 语义相等);
/// 否则请求体逐字节原样转发。
/// Outbound headers: auth + the subscription's required_headers ({api_key} filled) + content-type.
/// Cloudflare's gateway needs cf-aig-gateway-id; OpenRouter's attribution headers are harmless here.
pub fn build_outbound(row: &SubscriptionRow, raw_body: &Bytes, client_model: &str) -> Result<Outbound, String> {
    let (body, real_model, rewritten) = match target_model(row, client_model) {
        Some(target) if target != client_model => {
            let mut v: Value = serde_json::from_slice(raw_body).map_err(|e| e.to_string())?;
            v["model"] = Value::String(target.to_string());
            let bytes = serde_json::to_vec(&v).map_err(|e| e.to_string())?;
            (Bytes::from(bytes), target.to_string(), true)
        }
        _ => (raw_body.clone(), client_model.to_string(), false),
    };
    let mut headers = ReqHeaderMap::new();
    if let (Ok(name), Ok(value)) = (
        ReqHeaderName::try_from(row.auth_header_name.as_str()),
        ReqHeaderValue::from_str(&row.auth_header_value()),
    ) {
        headers.insert(name, value);
    }
    for (k, v) in row.resolved_required_headers() {
        if let (Ok(name), Ok(value)) = (ReqHeaderName::try_from(k.as_str()), ReqHeaderValue::from_str(&v)) {
            headers.insert(name, value);
        }
    }
    headers.insert(CONTENT_TYPE, ReqHeaderValue::from_static("application/json"));
    let url = crate::provider::url_template::fill_model(&row.messages_url(), &real_model);
    Ok(Outbound { url, headers, body, real_model, rewritten, wire: row.systemone_wire })
}

/// 改写过 model 时把响应 model 恢复成客户端写的名字 (与 fallback 兜底槽的回显规则一致);
/// 否则透传上游解析出的具体版本名 (如 `jev-latest` → `jev-1.13.0`)。
pub fn restore_model(mut body: Value, client_model: &str, rewritten: bool) -> Value {
    if rewritten && body.get("model").is_some() {
        body["model"] = Value::String(client_model.to_string());
    }
    body
}

/// cc-router 自己生成的错误体, 采用 TypeSafe 官方形状 (协议所有者, SDK 大概率按它解析)。
pub fn error_body(error_type: &str, message: &str) -> Value {
    json!({ "detail": { "error_type": error_type, "message": message } })
}

pub fn error_response(status: StatusCode, error_type: &str, message: &str) -> Response {
    (status, Json(error_body(error_type, message))).into_response()
}

/// 探测用的最小请求: 一道 noul 题。只看 HTTP 状态, 不看答案。
pub fn probe_body(model: &str) -> Value {
    json!({
        "model": model,
        "state": "ping",
        "questions": { "ok": { "type": "noul", "instructions": "Is this a connectivity test?" } }
    })
}

#[derive(Debug, Clone)]
pub enum AttemptResult {
    Http { status: StatusCode, body: Value, body_text: Option<String> },
    Network(String),
}

fn error_type_for(status: StatusCode) -> &'static str {
    match status.as_u16() {
        401 | 403 => "authentication_error",
        429 => "rate_limit_error",
        400..=499 => "api_usage_error",
        _ => "api_error",
    }
}

fn detail_result(status: StatusCode, error_type: &str, message: &str) -> AttemptResult {
    let body = error_body(error_type, message);
    let body_text = Some(body.to_string());
    AttemptResult::Http { status, body, body_text }
}

/// Map an upstream reply onto the standard System One shape (spec 2.1 / 3.3).
pub fn normalize_response(wire: SystemoneWire, r: AttemptResult) -> AttemptResult {
    match r {
        AttemptResult::Http { status, body, .. } if wire == SystemoneWire::CloudflareRun => {
            if status.is_success() {
                return match body.get("result").filter(|v| v.is_object()) {
                    Some(result) => AttemptResult::Http { status, body: result.clone(), body_text: None },
                    None => detail_result(StatusCode::BAD_GATEWAY, "api_error", "Cloudflare 响应缺少 result"),
                };
            }
            let first = body.get("errors").and_then(|e| e.get(0));
            let msg = first.and_then(|e| e.get("message")).and_then(Value::as_str);
            let code = first.and_then(|e| e.get("code")).and_then(Value::as_i64);
            let message = match (msg, code) {
                (Some(m), Some(c)) => format!("{m} (cloudflare code {c})"),
                (Some(m), None) => m.to_string(),
                _ => format!("HTTP {}", status.as_u16()),
            };
            detail_result(status, error_type_for(status), &message)
        }
        other => other,
    }
}

pub struct Candidate {
    pub sub_id: Uuid,
    pub display_name: String,
    pub provider_id: String,
    pub endpoint_id: String,
    pub rt: Arc<RwLock<SubscriptionRuntime>>,
    pub outbound: Outbound,
}

/// 候选循环的副作用出口。`send` 发请求, `record` 在每次 attempt 之后被调用一次
/// (状态机 + 请求日志)。生产实现见 `LiveSink`。
pub trait AttemptSink {
    fn send<'a>(&'a mut self, c: &'a Candidate) -> BoxFuture<'a, AttemptResult>;
    fn record<'a>(&'a mut self, c: &'a Candidate, r: &'a AttemptResult, retry_count: u32) -> BoxFuture<'a, ()>;
}

#[derive(Debug)]
pub enum LoopEnd {
    /// 某个候选给出了不需要重试的响应 (2xx, 或 classify 判为不重试的其他状态)。
    Done { index: usize, status: StatusCode, body: Value },
    /// 全部失败, 且最后一次失败是「请求本身的错误」(4xx, 不含 401/403/429): 原样返回给客户端。
    RequestRejected { status: StatusCode, body: Value, body_text: Option<String> },
    /// 全部失败 (网络 / 5xx / 鉴权 / 限流) 或没有候选: 503。
    Exhausted,
}

/// 4xx 里属于「请求本身写错了」的那些; 401/403/429 是订阅状态问题。
fn is_request_error(status: StatusCode) -> bool {
    status.is_client_error() && !matches!(status.as_u16(), 401 | 403 | 429)
}

pub async fn run_candidates<S: AttemptSink + Send>(cands: &[Candidate], sink: &mut S) -> LoopEnd {
    let mut last_rejection: Option<(StatusCode, Value, Option<String>)> = None;
    for (index, c) in cands.iter().enumerate() {
        let retry_count = index as u32;
        let r = sink.send(c).await;
        sink.record(c, &r, retry_count).await;
        match r {
            AttemptResult::Http { status, body, body_text } => {
                if let ShouldRetry::No = classify_response(status.as_u16(), None) {
                    return LoopEnd::Done { index, status, body };
                }
                last_rejection = is_request_error(status).then_some((status, body, body_text));
            }
            AttemptResult::Network(_) => last_rejection = None,
        }
    }
    match last_rejection {
        Some((status, body, body_text)) => LoopEnd::RequestRejected { status, body, body_text },
        None => LoopEnd::Exhausted,
    }
}

pub fn state_event(r: &AttemptResult) -> state_machine::Event {
    match r {
        AttemptResult::Http { status, .. } if status.is_success() => state_machine::Event::RequestSucceeded,
        AttemptResult::Http { status, .. } => state_machine::Event::HttpStatus(status.as_u16()),
        AttemptResult::Network(_) => state_machine::Event::NetworkError,
    }
}

/// FastAPI 风格的 422 会在 `detail[].input` 里回显整份请求 (含 state / questions), 落库前去掉。
/// 只处理「JSON 且 `detail` 是数组」这一种形状, 其余原样返回。
fn redact_echoed_input(text: &str) -> String {
    let Ok(mut v) = serde_json::from_str::<Value>(text) else {
        return text.to_string();
    };
    let Some(items) = v.get_mut("detail").and_then(|d| d.as_array_mut()) else {
        return text.to_string();
    };
    for item in items.iter_mut() {
        if let Some(obj) = item.as_object_mut() {
            obj.remove("input");
        }
    }
    serde_json::to_string(&v).unwrap_or_else(|_| text.to_string())
}

fn usage(body: &Value, key: &str) -> Option<u32> {
    body.get("usage").and_then(|u| u.get(key)).and_then(|v| v.as_u64()).map(|v| v as u32)
}

/// 每次 attempt 一行。不记 state / questions / answers / 置信度 / cost (隐私纪律同工具调用只存名字)。
pub fn log_entry(
    c: &Candidate,
    r: &AttemptResult,
    retry_count: u32,
    ctx: &ClientContext,
    latency_ms: u64,
) -> RequestLogEntry {
    let (status, http_status, response_model_name, input, output, error_message, upstream_body) = match r {
        AttemptResult::Http { status, body, body_text } => {
            let ok = status.is_success();
            (
                if ok { RequestStatus::Success } else { RequestStatus::Error },
                Some(status.as_u16()),
                body.get("model").and_then(|v| v.as_str()).map(str::to_string),
                usage(body, "input_tokens"),
                usage(body, "output_tokens"),
                (!ok).then(|| format!("HTTP {}", status.as_u16())),
                if ok { None } else { body_text.as_deref().map(|s| truncate_body(&redact_echoed_input(s), ERROR_BODY_LIMIT)) },
            )
        }
        AttemptResult::Network(msg) => (
            RequestStatus::Error,
            None,
            None,
            None,
            None,
            Some(format!("网络错误: {msg}")),
            None,
        ),
    };
    RequestLogEntry {
        id: Uuid::new_v4(),
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
        virtual_model_name: VirtualModelName::Jev,
        subscription_id: c.sub_id,
        provider_id: c.provider_id.clone(),
        endpoint_id: c.endpoint_id.clone(),
        real_model_name: c.outbound.real_model.clone(),
        response_model_name,
        is_streaming: false,
        status,
        http_status,
        ttft_ms: None,
        total_latency_ms: Some(latency_ms),
        upstream_input_tokens: input,
        upstream_output_tokens: output,
        upstream_cache_creation: None,
        upstream_cache_read: None,
        retry_count,
        error_message,
        upstream_response_body: upstream_body,
        client_tool: ctx.info.tool,
        client_user_agent: ctx.info.user_agent.clone(),
        client_version: ctx.info.version.clone(),
        client_ip: ctx.ip.clone(),
        entry_kind: Some(ctx.entry_kind.as_str()),
        downstream_http_version: ctx.http_version.clone(),
        client_effort: None,
        effective_effort: None,
        effort_source: None,
        upstream_effort: None,
        tool_calls: ToolLogFields::empty(),
    }
}

struct LiveSink<'s> {
    state: &'s AppState,
    ctx: &'s ClientContext,
    mode: RoutingMode,
    start: Instant,
}

impl AttemptSink for LiveSink<'_> {
    fn send<'a>(&'a mut self, c: &'a Candidate) -> BoxFuture<'a, AttemptResult> {
        Box::pin(async move {
            // sticky: 候选真正被交付请求时才钉 (含切下家; 不弹回), 与对话路径一致。
            if self.mode == RoutingMode::Sticky {
                if let Some(key) = self.ctx.session_key.as_deref() {
                    let mut t = self.state.session_affinity.lock().unwrap_or_else(|e| e.into_inner());
                    t.pin(VirtualModelName::Jev, key, c.sub_id, Instant::now());
                }
            }
            info!(sub_id = %c.sub_id, display_name = %c.display_name, real_model = %c.outbound.real_model, url = %c.outbound.url, "forwarding systemone request");
            emit_attempt_started(self.state, c.sub_id, VirtualModelName::Jev);
            let result = upstream::send(
                &self.state.http_client,
                &c.outbound.url,
                c.outbound.body.to_vec(),
                c.outbound.headers.clone(),
                false,
            )
            .await;
            let r = match result {
                Ok(upstream::UpstreamResponse::NonStreaming { status, body, body_text, .. }) => {
                    AttemptResult::Http { status, body, body_text }
                }
                // is_streaming=false 时 upstream::send 不会返回 Streaming; 保守地当网络错误处理。
                Ok(upstream::UpstreamResponse::Streaming { .. }) => {
                    AttemptResult::Network("unexpected streaming response".into())
                }
                Err(e) => AttemptResult::Network(e.to_string()),
            };
            let r = normalize_response(c.outbound.wire, r);
            let ok = matches!(&r, AttemptResult::Http { status, .. } if status.is_success());
            emit_attempt_finished(self.state, c.sub_id, VirtualModelName::Jev, ok);
            r
        })
    }

    fn record<'a>(&'a mut self, c: &'a Candidate, r: &'a AttemptResult, retry_count: u32) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let _ = state_machine::apply(
                &self.state.db,
                &self.state.app_handle,
                &self.state.event_log_tx,
                c.rt.clone(),
                state_event(r),
            )
            .await;
            let latency = self.start.elapsed().as_millis() as u64;
            let _ = self.state.request_log_tx.try_send(log_entry(c, r, retry_count, self.ctx, latency));
        })
    }
}

fn json_response(status: StatusCode, body: &Value) -> Response {
    (status, Json(body.clone())).into_response()
}

pub async fn dispatch(state: &AppState, raw_body: Bytes, client_model: String, ctx: &ClientContext) -> Response {
    let vm = VirtualModelName::Jev;
    let vm_config = state.virtual_models.read().await.get(&vm).cloned();
    let Some(vm_config) = vm_config.filter(|c| !c.subscription_ids.is_empty()) else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "overloaded_error", "model-jev 未绑定任何订阅");
    };

    let pinned = match (vm_config.mode, ctx.session_key.as_deref()) {
        (RoutingMode::Sticky, Some(key)) => {
            let mut t = state.session_affinity.lock().unwrap_or_else(|e| e.into_inner());
            t.get(vm, key, Instant::now())
        }
        _ => None,
    };
    let subs_map = state.subscriptions.read().await.clone();
    let order = build_candidate_order(&vm_config, &subs_map, Utc::now(), pinned).await;
    drop(subs_map);
    if let Some(idx) = order.chosen_index {
        if let Some(cfg) = state.virtual_models.write().await.get_mut(&vm) {
            cfg.last_used_index = idx;
        }
    }

    let mut cands = Vec::new();
    let mut skipped_wrong_protocol = Vec::new();
    for sub_id in &order.candidate_ids {
        let Some(rt) = state.get_subscription(sub_id).await else { continue };
        let g = rt.read().await;
        // 运行时守卫: 写入 / 导入已拦跨类绑定, 这里兜历史数据。
        if g.row.endpoint_protocol != EndpointProtocol::Systemone {
            warn!(%sub_id, display_name = %g.row.display_name, "non-systemone subscription skipped under model-jev");
            skipped_wrong_protocol.push(g.row.display_name.clone());
            continue;
        }
        let outbound = match build_outbound(&g.row, &raw_body, &client_model) {
            Ok(o) => o,
            Err(e) => {
                return error_response(StatusCode::BAD_REQUEST, "invalid_request_error", &format!("请求体解析失败: {e}"));
            }
        };
        cands.push(Candidate {
            sub_id: *sub_id,
            display_name: g.row.display_name.clone(),
            provider_id: g.row.provider_id.clone(),
            endpoint_id: g.row.endpoint_id.clone(),
            rt: rt.clone(),
            outbound,
        });
    }

    let mut sink = LiveSink { state, ctx, mode: vm_config.mode, start: Instant::now() };
    match run_candidates(&cands, &mut sink).await {
        LoopEnd::Done { index, status, body } => {
            let body = if status.is_success() {
                restore_model(body, &client_model, cands[index].outbound.rewritten)
            } else {
                body
            };
            json_response(status, &body)
        }
        LoopEnd::RequestRejected { status, body, body_text } => match body_text {
            // 原样返回上游的错误字节, 客户端看到的就是上游的原话。
            Some(text) => (status, [(axum::http::header::CONTENT_TYPE, "application/json")], text).into_response(),
            None => json_response(status, &body),
        },
        LoopEnd::Exhausted => {
            let subs_map = state.subscriptions.read().await;
            let mut summary = Vec::new();
            let now = Utc::now();
            for sub_id in &vm_config.subscription_ids {
                if let Some(rt) = subs_map.get(sub_id) {
                    let g = rt.read().await;
                    let exceeded = g.row.token_quotas.first_exceeded(&g.quota_usage, now).map(|p| p.label_zh());
                    summary.push(summary_line(
                        &g.row.display_name,
                        &format!("{:?}", g.state),
                        exceeded,
                        skipped_wrong_protocol.contains(&g.row.display_name),
                    ));
                }
            }
            drop(subs_map);
            events::record_system_error(
                &state.event_log_tx,
                "虚拟模型 model-jev 全部候选不可用".to_string(),
                Some(json!({ "virtual_model": vm.as_str(), "candidates": summary })),
            );
            error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "overloaded_error",
                &format!("All subscriptions for model-jev are unavailable.\nDetails:\n{}", summary.join("\n")),
            )
        }
    }
}

/// 503 摘要里一条订阅的说明: 跨类跳过优先, 其次是 token 限额 (限额只让订阅不可调度, 状态仍是 Healthy), 最后才是状态。
fn summary_line(display_name: &str, state: &str, exceeded: Option<&str>, skipped_wrong_protocol: bool) -> String {
    if skipped_wrong_protocol {
        return format!("- {display_name}: 不是 System One 订阅, 已跳过");
    }
    match exceeded {
        Some(period) => format!("- {display_name}: 已达 {period} token 限额"),
        None => format!("- {display_name}: {state}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription::model::{SubscriptionRow, SubscriptionRuntime};
    use std::collections::VecDeque;

    fn systemone_row(slot: &str) -> SubscriptionRow {
        let mut row = SubscriptionRow::test_fixture("typesafe", "default");
        row.endpoint_protocol = crate::provider::model::EndpointProtocol::Systemone;
        row.base_url = "https://api.typesafe.ai".into();
        row.messages_path = "/v1/systemone".into();
        row.auth_header_name = "Authorization".into();
        row.api_key = "k".into();
        row.model_slots.jev = slot.into();
        row.required_headers.insert("anthropic-version".into(), "2023-06-01".into());
        row
    }

    #[test]
    fn summary_line_reports_quota_before_state() {
        assert_eq!(summary_line("a", "Healthy", Some("每日"), false), "- a: 已达 每日 token 限额");
        assert_eq!(summary_line("a", "Healthy", None, false), "- a: Healthy");
        assert_eq!(summary_line("a", "Healthy", Some("每日"), true), "- a: 不是 System One 订阅, 已跳过");
    }

    const RAW: &str = r#"{"model":"jev-latest","state":"hi","questions":{"a":{"type":"noul","instructions":"y?"}}}"#;

    #[test]
    fn empty_slot_forwards_bytes_untouched() {
        let raw = Bytes::from_static(RAW.as_bytes());
        let out = build_outbound(&systemone_row(""), &raw, "jev-latest").unwrap();
        assert_eq!(out.body, raw, "槽为空时请求体逐字节不变");
        assert!(!out.rewritten);
        assert_eq!(out.real_model, "jev-latest");
        assert_eq!(out.url, "https://api.typesafe.ai/v1/systemone");
    }

    #[test]
    fn slot_rewrites_model_and_keeps_everything_else() {
        let raw = Bytes::from_static(RAW.as_bytes());
        let out = build_outbound(&systemone_row("clef-flash"), &raw, "jev-latest").unwrap();
        assert!(out.rewritten);
        assert_eq!(out.real_model, "clef-flash");
        let mut sent: Value = serde_json::from_slice(&out.body).unwrap();
        let mut orig: Value = serde_json::from_str(RAW).unwrap();
        assert_eq!(sent["model"], "clef-flash");
        sent["model"] = Value::Null;
        orig["model"] = Value::Null;
        assert_eq!(sent, orig, "除 model 外语义相等");
    }

    const RAW_VM: &str = r#"{"model":"model-jev","state":"hi","questions":{"a":{"type":"noul","instructions":"y?"}}}"#;

    /// 客户端写虚拟名 model-jev: 与 model-opus → opus 槽同理, 改写成订阅的 Jev 槽。
    #[test]
    fn virtual_name_uses_jev_slot() {
        let raw = Bytes::from_static(RAW_VM.as_bytes());
        let out = build_outbound(&systemone_row("clef-flash"), &raw, "model-jev").unwrap();
        assert!(out.rewritten);
        assert_eq!(out.real_model, "clef-flash");
        let sent: Value = serde_json::from_slice(&out.body).unwrap();
        assert_eq!(sent["model"], "clef-flash");
        let up = serde_json::json!({"model": "clef-flash", "answers": {}});
        assert_eq!(restore_model(up, "model-jev", out.rewritten)["model"], "model-jev", "响应回显虚拟名");
    }

    /// Jev 槽为空时虚拟名不能原样发给上游 (上游不认识 model-jev), 退到端点示例模型。
    #[test]
    fn virtual_name_without_slot_falls_back_to_example_model() {
        let raw = Bytes::from_static(RAW_VM.as_bytes());
        let mut row = systemone_row("");
        row.model_discovery.example_models = vec!["jev-latest".into(), "jev-preview".into()];
        let out = build_outbound(&row, &raw, "model-jev").unwrap();
        assert!(out.rewritten);
        assert_eq!(out.real_model, "jev-latest");
        // 带厂商前缀的写法同样认 (与对话入口的虚拟名解析一致)
        let out = build_outbound(&row, &raw, "openai/model-jev").unwrap();
        assert_eq!(out.real_model, "jev-latest");
    }

    /// 槽和示例都没有时只能原样发出 (上游会报未知模型, 走正常的 4xx 换下家)。
    #[test]
    fn virtual_name_without_slot_or_example_passes_through() {
        let raw = Bytes::from_static(RAW_VM.as_bytes());
        let mut row = systemone_row("");
        row.model_discovery.example_models.clear();
        let out = build_outbound(&row, &raw, "model-jev").unwrap();
        assert!(!out.rewritten);
        assert_eq!(out.body, raw);
    }

    #[test]
    fn slot_equal_to_client_model_is_not_a_rewrite() {
        let raw = Bytes::from_static(RAW.as_bytes());
        let out = build_outbound(&systemone_row("jev-latest"), &raw, "jev-latest").unwrap();
        assert!(!out.rewritten);
        assert_eq!(out.body, raw);
    }

    #[test]
    fn outbound_headers_are_auth_required_and_content_type() {
        let raw = Bytes::from_static(RAW.as_bytes());
        let out = build_outbound(&systemone_row(""), &raw, "jev-latest").unwrap();
        assert_eq!(out.headers.len(), 3, "{:?}", out.headers);
        assert_eq!(out.headers["authorization"], "Bearer k");
        assert_eq!(out.headers["content-type"], "application/json");
        // systemone 现在发送订阅的 required_headers (spec §3.3)
        assert_eq!(out.headers["anthropic-version"], "2023-06-01");
    }

    #[test]
    fn response_model_restored_only_when_rewritten() {
        let up = serde_json::json!({"model": "clef-flash", "answers": {}});
        assert_eq!(restore_model(up.clone(), "jev-latest", true)["model"], "jev-latest");
        let up = serde_json::json!({"model": "jev-1.13.0", "answers": {}});
        assert_eq!(restore_model(up, "jev-latest", false)["model"], "jev-1.13.0", "未改写时透传上游解析后的版本名");
    }

    #[test]
    fn error_body_uses_typesafe_shape() {
        let b = error_body("overloaded_error", "boom");
        assert_eq!(b["detail"]["error_type"], "overloaded_error");
        assert_eq!(b["detail"]["message"], "boom");
    }

    // ---------- 候选循环 ----------

    struct MockSink {
        replies: VecDeque<AttemptResult>,
        sent: Vec<Uuid>,
        recorded: Vec<(Uuid, u32)>,
    }

    impl MockSink {
        fn new(replies: Vec<AttemptResult>) -> Self {
            Self { replies: replies.into(), sent: Vec::new(), recorded: Vec::new() }
        }
    }

    impl AttemptSink for MockSink {
        fn send<'a>(&'a mut self, c: &'a Candidate) -> BoxFuture<'a, AttemptResult> {
            self.sent.push(c.sub_id);
            let r = self.replies.pop_front().expect("测试没给足回复");
            Box::pin(async move { r })
        }
        fn record<'a>(&'a mut self, c: &'a Candidate, _r: &'a AttemptResult, retry_count: u32) -> BoxFuture<'a, ()> {
            self.recorded.push((c.sub_id, retry_count));
            Box::pin(async {})
        }
    }

    fn cand(name: &str) -> Candidate {
        let row = systemone_row("");
        let raw = Bytes::from_static(RAW.as_bytes());
        Candidate {
            sub_id: Uuid::new_v4(),
            display_name: name.into(),
            provider_id: row.provider_id.clone(),
            endpoint_id: row.endpoint_id.clone(),
            outbound: build_outbound(&row, &raw, "jev-latest").unwrap(),
            rt: Arc::new(RwLock::new(SubscriptionRuntime::from_row(row))),
        }
    }

    fn http(status: u16, body: Value) -> AttemptResult {
        let status = StatusCode::from_u16(status).unwrap();
        let body_text = (!status.is_success()).then(|| body.to_string());
        AttemptResult::Http { status, body, body_text }
    }

    fn ok_body() -> Value {
        serde_json::json!({"model": "jev-1.13.0", "answers": {"a": {"type": "noul", "noul": 0.9}}, "usage": {"input_tokens": 300, "output_tokens": 20}})
    }

    fn cf_row() -> SubscriptionRow {
        let mut row = systemone_row("");
        row.base_url = "https://api.cloudflare.com/client/v4/accounts/acct/ai".into();
        row.messages_path = "/run/@cf/cloudflare/{model}".into();
        row.model_discovery.example_models = vec!["clef-flash".into()];
        row.systemone_wire = crate::provider::model::SystemoneWire::CloudflareRun;
        row.api_key = "k-1".into();
        row.required_headers.insert("cf-aig-gateway-id".into(), "gw".into());
        row.required_headers.insert("cf-aig-authorization".into(), "Bearer {api_key}".into());
        row
    }

    #[test]
    fn cloudflare_url_carries_model_and_headers_are_sent() {
        let raw = Bytes::from_static(br#"{"model":"model-jev","state":"s","questions":{}}"#);
        let out = build_outbound(&cf_row(), &raw, "model-jev").unwrap();
        assert_eq!(out.url, "https://api.cloudflare.com/client/v4/accounts/acct/ai/run/@cf/cloudflare/clef-flash");
        assert_eq!(out.headers.get("cf-aig-gateway-id").unwrap(), "gw");
        assert_eq!(out.headers.get("cf-aig-authorization").unwrap(), "Bearer k-1");
        assert_eq!(out.headers.get("content-type").unwrap(), "application/json");
        let body: Value = serde_json::from_slice(&out.body).unwrap();
        assert_eq!(body["model"], "clef-flash");
    }

    #[test]
    fn cloudflare_success_envelope_is_unwrapped() {
        // spec 2.1 real bytes
        let body = serde_json::json!({"result":{"model":"clef-flash","answers":{"refund":{"type":"noul","noul":0.9803}},"usage":{"input_tokens":160,"output_tokens":0}},"success":true,"errors":[],"messages":[]});
        let AttemptResult::Http { status, body, body_text } = normalize_response(SystemoneWire::CloudflareRun, http(200, body)) else { panic!() };
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["answers"]["refund"]["noul"], 0.9803);
        assert_eq!(body["usage"]["input_tokens"], 160);
        assert!(body.get("success").is_none());
        assert!(body_text.is_none());
    }

    #[test]
    fn cloudflare_errors_become_detail_shape() {
        let cases = [
            (400, 5006, "AiError: Bad input", "api_usage_error"),
            (422, 5012, "AiError: Request body failed validation", "api_usage_error"),
            (400, 7000, "No route for that URI", "api_usage_error"),
            (401, 10000, "Authentication error", "authentication_error"),
            (429, 3040, "Capacity temporarily exceeded", "rate_limit_error"),
            (500, 3043, "Internal server error", "api_error"),
        ];
        for (status, code, msg, ty) in cases {
            let body = serde_json::json!({"errors":[{"message":msg,"code":code}],"success":false,"result":{},"messages":[]});
            let AttemptResult::Http { status: s, body, body_text } = normalize_response(SystemoneWire::CloudflareRun, http(status, body)) else { panic!() };
            assert_eq!(s.as_u16(), status);
            assert_eq!(body["detail"]["error_type"], ty);
            assert_eq!(body["detail"]["message"], format!("{msg} (cloudflare code {code})"));
            assert_eq!(serde_json::from_str::<Value>(body_text.as_deref().unwrap()).unwrap(), body);
        }
    }

    #[test]
    fn cloudflare_error_without_errors_array_uses_http_status() {
        let AttemptResult::Http { body, .. } = normalize_response(SystemoneWire::CloudflareRun, http(502, serde_json::json!({}))) else { panic!() };
        assert_eq!(body["detail"]["message"], "HTTP 502");
    }

    #[test]
    fn cloudflare_2xx_without_result_is_bad_gateway() {
        let r = normalize_response(SystemoneWire::CloudflareRun, http(200, serde_json::json!({"success":true})));
        let AttemptResult::Http { status, body, .. } = r else { panic!() };
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(body["detail"]["error_type"], "api_error");
    }

    #[test]
    fn standard_wire_is_untouched() {
        let body = ok_body();
        let AttemptResult::Http { body: b, .. } = normalize_response(SystemoneWire::Standard, http(200, body.clone())) else { panic!() };
        assert_eq!(b, body);
    }

    #[test]
    fn network_error_is_untouched() {
        let r = normalize_response(SystemoneWire::CloudflareRun, AttemptResult::Network("boom".into()));
        assert!(matches!(r, AttemptResult::Network(m) if m == "boom"));
    }

    #[tokio::test]
    async fn not_found_on_first_then_success_on_second() {
        let cands = vec![cand("A"), cand("B")];
        let mut sink = MockSink::new(vec![http(404, serde_json::json!({"error": "model not found"})), http(200, ok_body())]);
        match run_candidates(&cands, &mut sink).await {
            LoopEnd::Done { index, status, .. } => assert_eq!((index, status.as_u16()), (1, 200)),
            other => panic!("{other:?}"),
        }
        assert_eq!(sink.recorded, vec![(cands[0].sub_id, 0), (cands[1].sub_id, 1)], "每次 attempt 一条记录, retry_count 递增");
    }

    /// 实测推翻了「400 短路」: 换一家可能成功, 所以 400 也要切下家。
    #[tokio::test]
    async fn bad_request_does_not_short_circuit() {
        let cands = vec![cand("A"), cand("B")];
        let mut sink = MockSink::new(vec![http(400, serde_json::json!({"detail": {"error_type": "api_usage_error", "message": "Invalid request."}})), http(200, ok_body())]);
        assert!(matches!(run_candidates(&cands, &mut sink).await, LoopEnd::Done { index: 1, .. }));
        assert_eq!(sink.sent.len(), 2);
    }

    #[tokio::test]
    async fn all_request_errors_return_the_last_upstream_error() {
        let cands = vec![cand("A"), cand("B")];
        let last = serde_json::json!({"error": {"message": "Model nope does not exist", "code": 400}});
        let mut sink = MockSink::new(vec![http(422, serde_json::json!({"detail": []})), http(400, last.clone())]);
        match run_candidates(&cands, &mut sink).await {
            LoopEnd::RequestRejected { status, body, .. } => {
                assert_eq!(status.as_u16(), 400);
                assert_eq!(body, last);
            }
            other => panic!("{other:?}"),
        }
    }

    /// 401 / 403 / 429 是订阅状态问题, 不当作「请求本身的错误」原样返回。
    #[tokio::test]
    async fn auth_and_rate_limit_failures_are_exhausted_not_passthrough() {
        let cands = vec![cand("A"), cand("B")];
        let mut sink = MockSink::new(vec![http(400, serde_json::json!({})), http(429, serde_json::json!({}))]);
        assert!(matches!(run_candidates(&cands, &mut sink).await, LoopEnd::Exhausted), "最后一次是 429 → 503");
        let mut sink = MockSink::new(vec![http(401, serde_json::json!({})), http(403, serde_json::json!({}))]);
        assert!(matches!(run_candidates(&cands, &mut sink).await, LoopEnd::Exhausted));
    }

    #[tokio::test]
    async fn server_and_network_errors_are_exhausted() {
        let cands = vec![cand("A"), cand("B")];
        let mut sink = MockSink::new(vec![http(529, serde_json::json!({})), AttemptResult::Network("connection refused".into())]);
        assert!(matches!(run_candidates(&cands, &mut sink).await, LoopEnd::Exhausted));
        assert_eq!(sink.sent.len(), 2);
    }

    #[tokio::test]
    async fn no_candidates_is_exhausted() {
        let mut sink = MockSink::new(vec![]);
        assert!(matches!(run_candidates(&[], &mut sink).await, LoopEnd::Exhausted));
    }

    #[test]
    fn log_entry_fields() {
        let c = cand("A");
        let ctx = ClientContext { entry_kind: crate::proxy::client_fingerprint::RequestEntryKind::SystemOne, ..Default::default() };
        let ok = log_entry(&c, &http(200, ok_body()), 1, &ctx, 812);
        assert_eq!(ok.virtual_model_name, crate::virtual_model::VirtualModelName::Jev);
        assert_eq!(ok.entry_kind, Some("systemone"));
        assert!(!ok.is_streaming && ok.ttft_ms.is_none());
        assert_eq!((ok.upstream_input_tokens, ok.upstream_output_tokens), (Some(300), Some(20)));
        assert_eq!((ok.upstream_cache_creation, ok.upstream_cache_read), (None, None));
        assert_eq!(ok.response_model_name.as_deref(), Some("jev-1.13.0"), "记上游原值");
        assert_eq!(ok.real_model_name, "jev-latest");
        assert_eq!((ok.retry_count, ok.total_latency_ms), (1, Some(812)));
        assert!(ok.client_effort.is_none() && ok.effective_effort.is_none() && ok.effort_source.is_none());
        assert_eq!(ok.tool_calls, ToolLogFields::empty());
        assert!(ok.error_message.is_none() && ok.upstream_response_body.is_none());

        let bad = log_entry(&c, &http(400, serde_json::json!({"error": "x"})), 0, &ctx, 5);
        assert_eq!(bad.http_status, Some(400));
        assert_eq!(bad.error_message.as_deref(), Some("HTTP 400"));
        assert!(bad.upstream_response_body.unwrap().contains("\"error\""));

        let net = log_entry(&c, &AttemptResult::Network("refused".into()), 0, &ctx, 5);
        assert_eq!(net.http_status, None);
        assert!(net.error_message.unwrap().contains("refused"));
    }

    #[test]
    fn state_events() {
        use crate::subscription::state_machine::Event;
        assert!(matches!(state_event(&http(200, ok_body())), Event::RequestSucceeded));
        assert!(matches!(state_event(&http(429, serde_json::json!({}))), Event::HttpStatus(429)));
        assert!(matches!(state_event(&AttemptResult::Network("x".into())), Event::NetworkError));
    }

    #[test]
    fn redact_drops_echoed_request_input_from_422() {
        let body = r#"{"detail":[{"type":"missing","loc":["body","questions"],"msg":"Field required","input":{"model":"jev-latest","state":"x"}}]}"#;
        let out = redact_echoed_input(body);
        assert!(!out.contains("\"state\"") && !out.contains("\"input\""), "{out}");
        assert!(out.contains("Field required"), "{out}");
    }

    #[test]
    fn redact_leaves_other_error_shapes_untouched() {
        let ollama = r#"{"error":"model \"nope\" not found"}"#;
        assert_eq!(redact_echoed_input(ollama), ollama);
        assert_eq!(redact_echoed_input("upstream exploded"), "upstream exploded");
        let not_array = r#"{"detail":{"error_type":"api_usage_error","input":"keep"}}"#;
        assert_eq!(redact_echoed_input(not_array), not_array);
    }

    #[test]
    fn log_entry_error_body_is_redacted() {
        let c = cand("A");
        let ctx = ClientContext::default();
        let body = serde_json::json!({"detail":[{"msg":"Field required","input":{"state":"secret"}}]});
        let bad = log_entry(&c, &http(422, body), 0, &ctx, 5);
        let stored = bad.upstream_response_body.unwrap();
        assert!(!stored.contains("secret") && stored.contains("Field required"), "{stored}");
    }

    #[test]
    fn probe_body_is_a_minimal_noul_question() {
        let b = probe_body("clef-flash");
        assert_eq!(b["model"], "clef-flash");
        assert_eq!(b["questions"]["ok"]["type"], "noul");
    }
}
