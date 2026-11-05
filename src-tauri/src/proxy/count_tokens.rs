//! `POST /v1/messages/count_tokens` (Claude Code gateway guide: optional; when
//! absent the client falls back to a character-based estimate).
//!
//! Only Anthropic-passthrough subscriptions can answer it — translation
//! upstreams have no equivalent endpoint. Selection is read-only: it reuses
//! `build_candidate_order` (which never advances `last_used_index`; only the
//! messages pipeline does), creates no sticky affinity, writes no request log,
//! touches no quota and no state machine.

use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use bytes::Bytes;
use chrono::Utc;
use serde_json::Value;

use crate::provider::model::{AuthType, EndpointProtocol};
use crate::proxy::handler::error_response;
use crate::proxy::pipeline::candidate_model_or_skip;
use crate::proxy::response_headers::copy_passthrough_response_headers;
use crate::proxy::forward;
use crate::state::AppState;
use crate::subscription::model::SubscriptionRow;
use crate::subscription::schedule::WallClock;
use crate::virtual_model::scheduler::build_candidate_order;
use crate::virtual_model::VirtualModelName;

/// Derives the count_tokens URL from a subscription's messages URL: the path
/// must end in `/messages` (query kept). Anything else (`/run/{model}` etc.)
/// has no count_tokens counterpart → None.
pub fn count_tokens_url(messages_url: &str) -> Option<String> {
    let (path, query) = match messages_url.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (messages_url, None),
    };
    let path = path.trim_end_matches('/');
    if !path.ends_with("/messages") {
        return None;
    }
    let mut out = format!("{path}/count_tokens");
    if let Some(q) = query {
        out.push('?');
        out.push_str(q);
    }
    Some(out)
}

/// Outbound target for one count_tokens call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub url: String,
    /// Model to write into the body; None = keep the client's model
    /// (fallback without a fallback slot).
    pub model: Option<String>,
}

/// Picks the target for `vm` from one candidate row, or None when the row
/// cannot answer count_tokens (translation auth, System One endpoint, no slot
/// model right now, unknown URL shape, or fallback-only translation).
pub fn target_for(row: &SubscriptionRow, vm: VirtualModelName, clock: WallClock) -> Option<Target> {
    if row.auth_type != AuthType::ApiKey || row.endpoint_protocol != EndpointProtocol::Messages {
        return None;
    }
    let url = count_tokens_url(&row.messages_url())?;
    let model = if vm.is_fallback() {
        row.model_slots.fallback_model().map(str::to_string)
    } else {
        // Same per-time-window slot rule as the messages pipeline; empty slot → skip.
        candidate_model_or_skip(row, vm, clock).ok()?
    };
    Some(Target { url, model })
}

fn not_available() -> Response {
    error_response(StatusCode::NOT_FOUND, "not_found_error", "count_tokens 不可用")
}

pub async fn handle(state: &AppState, client_headers: HeaderMap, body: Bytes) -> Response {
    let mut parsed: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                &format!("JSON 解析失败: {e}"),
            )
        }
    };
    let Some(model) = parsed.get("model").and_then(|v| v.as_str()).map(str::to_string) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid_request_error", "缺少 model 字段");
    };
    let vm = VirtualModelName::parse(&model).unwrap_or(VirtualModelName::Fallback);
    if vm.is_jev() {
        return not_available();
    }
    let Some(vm_config) = state.virtual_models.read().await.get(&vm).cloned() else {
        return not_available();
    };
    let clock = WallClock::now();
    let subs = state.subscriptions.read().await.clone();
    let order = build_candidate_order(&vm_config, &subs, Utc::now(), clock, None).await;

    for sub_id in order.candidate_ids {
        let Some(rt) = subs.get(&sub_id) else { continue };
        let (target, row) = {
            let g = rt.read().await;
            (target_for(&g.row, vm, clock), g.row.clone())
        };
        let Some(target) = target else { continue };
        if let Some(m) = &target.model {
            parsed["model"] = Value::String(m.clone());
        }
        let Ok(bytes) = serde_json::to_vec(&parsed) else { return not_available() };
        let headers = forward::build_anthropic_passthrough_headers(
            &client_headers,
            &row.header_policy,
            &row.forward_headers,
            &row.resolved_required_headers_tagged(),
            &row.auth_header_name,
            &row.auth_header_value(),
        )
        .to_reqwest();
        let resp = match state.http_client.post(&target.url).headers(headers).body(bytes).send().await {
            Ok(r) => r,
            Err(_) => return not_available(),
        };
        let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        if status == StatusCode::NOT_FOUND {
            return not_available();
        }
        let resp_headers = resp.headers().clone();
        let body = resp.bytes().await.unwrap_or_default();
        let mut out = Response::new(axum::body::Body::from(body));
        *out.status_mut() = status;
        copy_passthrough_response_headers(&resp_headers, out.headers_mut());
        return out;
    }
    not_available()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription::model::ModelSlots;

    #[test]
    fn count_tokens_url_rewrites_path() {
        assert_eq!(
            count_tokens_url("https://api.anthropic.com/v1/messages").as_deref(),
            Some("https://api.anthropic.com/v1/messages/count_tokens")
        );
        assert_eq!(
            count_tokens_url("https://x.example/anthropic/v1/messages?beta=true").as_deref(),
            Some("https://x.example/anthropic/v1/messages/count_tokens?beta=true")
        );
        assert_eq!(count_tokens_url("https://x.example/run/{model}"), None);
        assert_eq!(count_tokens_url("https://x.example/v1/messagesx"), None);
    }

    fn anthropic_row() -> SubscriptionRow {
        let mut row = SubscriptionRow::test_fixture("anthropic", "default");
        row.base_url = "https://api.anthropic.com".into();
        row.messages_path = "/v1/messages".into();
        row.auth_type = AuthType::ApiKey;
        row.model_slots = ModelSlots {
            opus: "claude-opus-5-5".into(),
            ..row.model_slots.clone()
        };
        row
    }

    #[test]
    fn passthrough_row_targets_slot_model() {
        let t = target_for(&anthropic_row(), VirtualModelName::Opus, WallClock::now()).unwrap();
        assert_eq!(t.url, "https://api.anthropic.com/v1/messages/count_tokens");
        assert_eq!(t.model.as_deref(), Some("claude-opus-5-5"));
    }

    #[test]
    fn count_tokens_skips_translation_and_systemone() {
        let mut gemini = anthropic_row();
        gemini.auth_type = AuthType::GeminiApiKey;
        assert!(target_for(&gemini, VirtualModelName::Opus, WallClock::now()).is_none());
        let mut s1 = anthropic_row();
        s1.endpoint_protocol = EndpointProtocol::Systemone;
        assert!(target_for(&s1, VirtualModelName::Opus, WallClock::now()).is_none());
    }

    #[test]
    fn count_tokens_fallback_without_slot_keeps_model() {
        let row = anthropic_row();
        let t = target_for(&row, VirtualModelName::Fallback, WallClock::now()).unwrap();
        assert_eq!(t.model, None);
        let mut with_slot = anthropic_row();
        with_slot.model_slots.fallback = "claude-haiku-5-5".into();
        let t = target_for(&with_slot, VirtualModelName::Fallback, WallClock::now()).unwrap();
        assert_eq!(t.model.as_deref(), Some("claude-haiku-5-5"));
    }

    #[test]
    fn empty_slot_is_skipped() {
        let mut row = anthropic_row();
        row.model_slots.sonnet = String::new();
        assert!(target_for(&row, VirtualModelName::Sonnet, WallClock::now()).is_none());
    }
}
