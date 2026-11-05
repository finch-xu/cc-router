//! Outbound header preview for the subscription edit page: what the
//! passthrough path would send upstream for a typical Claude Code request,
//! computed with the same assembly function as real traffic so the preview
//! and the wire cannot drift apart. Read-only: no request, no write.

use std::collections::BTreeMap;

use serde::Serialize;
use tauri::State;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::proxy::forward::DropReason;
use crate::proxy::header_source::HeaderSource;
use crate::state::AppState;
use crate::subscription::header_policy::HeaderPolicy;
use crate::subscription::model::SubscriptionRow;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct PreviewHeader {
    pub name: String,
    pub value: String,
    pub source: HeaderSource,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct PreviewDropped {
    pub name: String,
    pub value: String,
    pub reason: DropReason,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct HeaderPreviewDto {
    pub sent: Vec<PreviewHeader>,
    pub dropped: Vec<PreviewDropped>,
}

/// Preview for `row` under `policy`, using [`SAMPLE_CLAUDE_CODE_HEADERS`] as
/// the client request. Values are masked like the request log; the default
/// UA reqwest adds at send time is listed when nothing else sets one.
///
/// [`SAMPLE_CLAUDE_CODE_HEADERS`]: crate::proxy::forward::SAMPLE_CLAUDE_CODE_HEADERS
pub fn preview(row: &SubscriptionRow, policy: &HeaderPolicy) -> HeaderPreviewDto {
    use crate::proxy::forward::{build_anthropic_passthrough_headers, SAMPLE_CLAUDE_CODE_HEADERS};
    use crate::proxy::header_log::redact_with;

    let mut client = axum::http::HeaderMap::new();
    for (k, v) in SAMPLE_CLAUDE_CODE_HEADERS {
        client.append(
            axum::http::HeaderName::from_static(k),
            axum::http::HeaderValue::from_static(v),
        );
    }
    let out = build_anthropic_passthrough_headers(
        &client,
        policy,
        &row.forward_headers,
        &row.resolved_required_headers_tagged(),
        &row.auth_header_name,
        &row.auth_header_value(),
    );
    let secrets = [row.api_key.as_str()];
    let mut sent: Vec<PreviewHeader> = out
        .sent
        .into_iter()
        .map(|(name, value, source)| {
            let value = redact_with(&name, &value, &secrets);
            PreviewHeader { name, value, source }
        })
        .collect();
    if !sent.iter().any(|h| h.name == "user-agent") {
        sent.push(PreviewHeader {
            name: "user-agent".into(),
            value: crate::proxy::upstream::DEFAULT_USER_AGENT.into(),
            source: HeaderSource::CcRouter,
        });
    }
    let dropped = out
        .dropped
        .into_iter()
        .map(|d| PreviewDropped {
            value: redact_with(&d.name, &d.value, &secrets),
            name: d.name,
            reason: d.reason,
        })
        .collect();
    HeaderPreviewDto { sent, dropped }
}

/// Header preview with the edit page's unsaved draft. `headers` is the table
/// the user edits: `extra_headers` on a built-in subscription,
/// `required_headers` on a user-defined one. Validated like a save would be.
#[tauri::command]
pub async fn preview_outbound_headers(
    state: State<'_, AppState>,
    subscription_id: String,
    header_policy: HeaderPolicy,
    headers: BTreeMap<String, String>,
) -> AppResult<HeaderPreviewDto> {
    use crate::commands::subscriptions::{validate_extra_headers, validate_required_headers};
    use crate::provider::model::{AuthType, EndpointProtocol};

    let id = Uuid::parse_str(&subscription_id).map_err(|_| AppError::BadRequest("无效 id".into()))?;
    let mut row = {
        let subs = state.subscriptions.read().await;
        let rt = subs.get(&id).ok_or_else(|| AppError::SubscriptionNotFound(id.to_string()))?;
        // bound first: the read guard must drop before `subs`
        let row = rt.read().await.row.clone();
        row
    };
    if row.auth_type != AuthType::ApiKey || row.endpoint_protocol == EndpointProtocol::Systemone {
        return Err(AppError::BadRequest("该订阅不消费请求头透传策略".into()));
    }
    if row.is_user_defined {
        validate_required_headers(&headers, &row.auth_header_name)?;
        row.required_headers = headers;
    } else {
        validate_extra_headers(&headers, &row.auth_header_name, row.auth_type)?;
        row.extra_headers = headers;
    }
    Ok(preview(&row, &header_policy))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription::header_policy::HeaderPolicyMode;

    fn row() -> SubscriptionRow {
        let mut r = SubscriptionRow::test_fixture("anthropic", "api_paygo");
        r.auth_header_name = "x-api-key".into();
        r.api_key = "sk-test-abcdefghijk".into();
        r.required_headers.insert("anthropic-version".into(), "2023-06-01".into());
        r
    }

    fn sent<'a>(d: &'a HeaderPreviewDto, name: &str) -> Vec<(&'a str, HeaderSource)> {
        d.sent.iter().filter(|h| h.name == name).map(|h| (h.value.as_str(), h.source)).collect()
    }

    fn reason(d: &HeaderPreviewDto, name: &str) -> Option<DropReason> {
        d.dropped.iter().find(|h| h.name == name).map(|h| h.reason)
    }

    #[test]
    fn preview_none_policy_drops_all_sample_headers() {
        let d = preview(&row(), &HeaderPolicy::none());
        assert_eq!(reason(&d, "authorization"), Some(DropReason::HardDenied));
        assert_eq!(reason(&d, "x-app"), Some(DropReason::NotSelected));
        assert!(d.sent.iter().all(|h| h.source != HeaderSource::Forwarded));
        assert_eq!(sent(&d, "anthropic-version"), vec![("2023-06-01", HeaderSource::Provider)]);
        assert_eq!(
            sent(&d, "user-agent"),
            vec![(crate::proxy::upstream::DEFAULT_USER_AGENT, HeaderSource::CcRouter)]
        );
    }

    #[test]
    fn preview_claude_code_forwards_ua() {
        let d = preview(&row(), &HeaderPolicy::claude_code());
        assert_eq!(
            sent(&d, "user-agent"),
            vec![("claude-cli/2.1.0 (external, cli)", HeaderSource::Forwarded)]
        );
        // the yaml-fixed version beats the client's same-valued one
        assert_eq!(sent(&d, "anthropic-version"), vec![("2023-06-01", HeaderSource::Provider)]);
        assert_eq!(reason(&d, "anthropic-version"), Some(DropReason::Overridden));
    }

    #[test]
    fn preview_masks_auth_value() {
        let d = preview(&row(), &HeaderPolicy { mode: HeaderPolicyMode::All, names: vec![] });
        let auth = sent(&d, "x-api-key");
        assert_eq!(auth.len(), 1);
        assert_eq!(auth[0].1, HeaderSource::Auth);
        assert!(!auth[0].0.contains("abcdefghijk"), "{}", auth[0].0);
        for h in &d.dropped {
            assert!(!h.value.contains("sample-0000000000"), "{h:?} leaked the sample bearer");
        }
    }
}
