//! Outbound header assembly for the Anthropic passthrough path (issue #43).
//!
//! When a subscription opts in via `forward_client_headers`, a built-in
//! whitelist of client headers is forwarded to the upstream so Anthropic
//! compatible relays can rate-limit per session / see beta flags. The
//! whitelist deliberately excludes `user-agent` (cc-router identifies itself
//! through its own reqwest UA) and every credential header the client sent to
//! cc-router (`authorization` / `x-api-key` / `cookie`).
//!
//! Priority on name collision (later insert wins):
//! forwarded client headers < `required_headers` (user-editable on custom
//! subscriptions) < auth header < hard-coded `content-type`.

use std::collections::{BTreeMap, HashSet};

use axum::http::HeaderMap;
use reqwest::header::{
    HeaderMap as ReqHeaderMap, HeaderName as ReqHeaderName, HeaderValue as ReqHeaderValue,
};

/// Exact-match whitelist. Lowercase only: axum normalizes inbound names.
const PASSTHROUGH_FORWARD_EXACT: &[&str] = &[
    "x-claude-code-session-id",
    "anthropic-beta",
    "anthropic-version",
    "x-app",
];

/// Prefix-match whitelist (Anthropic SDK telemetry headers).
const PASSTHROUGH_FORWARD_PREFIXES: &[&str] = &["x-stainless-"];

fn matches_builtin(name: &str) -> bool {
    PASSTHROUGH_FORWARD_EXACT.contains(&name)
        || PASSTHROUGH_FORWARD_PREFIXES
            .iter()
            .any(|p| name.starts_with(p))
}

/// Build the complete outbound header map for the Anthropic passthrough path.
///
/// `forward_enabled=false` skips the built-in whitelist entirely, so the
/// outbound header set matches the pre-switch behavior (the per-subscription
/// yaml `forward_headers` whitelist is still honored either way). Invalid
/// header names/values are silently skipped, matching the historical
/// if-let-Ok style at every dispatch site.
pub fn build_anthropic_passthrough_headers(
    client_headers: &HeaderMap,
    forward_enabled: bool,
    yaml_forward: &[String],
    required_headers: &BTreeMap<String, String>,
    auth_header_name: &str,
    auth_header_value: &str,
) -> ReqHeaderMap {
    let mut out = ReqHeaderMap::new();

    // Forwarded client headers go in first so every cc-router-controlled
    // header inserted below overrides them on name collision. Iterate the
    // client map (not the whitelist): prefix entries cannot be enumerated
    // from the whitelist side, and http's iter yields one pair per value so
    // multi-value headers (repeated anthropic-beta) survive via append.
    let yaml_set: HashSet<String> = yaml_forward
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    for (name, value) in client_headers.iter() {
        let n = name.as_str();
        if (forward_enabled && matches_builtin(n)) || yaml_set.contains(n) {
            if let (Ok(rn), Ok(rv)) = (
                ReqHeaderName::try_from(n),
                ReqHeaderValue::from_bytes(value.as_bytes()),
            ) {
                out.append(rn, rv);
            }
        }
    }

    // insert (unlike append) drops every previously forwarded value under the
    // same name — required_headers replace, they do not merge.
    for (k, v) in required_headers.iter() {
        if let (Ok(rn), Ok(rv)) = (
            ReqHeaderName::try_from(k.as_str()),
            ReqHeaderValue::from_str(v),
        ) {
            out.insert(rn, rv);
        }
    }

    if let (Ok(rn), Ok(rv)) = (
        ReqHeaderName::try_from(auth_header_name),
        ReqHeaderValue::from_str(auth_header_value),
    ) {
        out.insert(rn, rv);
    }

    out.insert(
        reqwest::header::CONTENT_TYPE,
        ReqHeaderValue::from_static("application/json"),
    );

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderName, HeaderValue};

    fn client_headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.append(
                HeaderName::try_from(*k).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        m
    }

    fn build(
        client: &HeaderMap,
        enabled: bool,
        yaml: &[&str],
        required: &[(&str, &str)],
    ) -> ReqHeaderMap {
        let yaml: Vec<String> = yaml.iter().map(|s| s.to_string()).collect();
        let required: BTreeMap<String, String> = required
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        build_anthropic_passthrough_headers(
            client,
            enabled,
            &yaml,
            &required,
            "x-api-key",
            "sk-real-upstream-key",
        )
    }

    #[test]
    fn switch_off_matches_legacy_behavior() {
        let client = client_headers(&[
            ("x-claude-code-session-id", "sess-1"),
            ("anthropic-beta", "context-1m-2025"),
            ("anthropic-version", "2023-06-01"),
            ("x-app", "cli"),
            ("x-stainless-lang", "js"),
        ]);
        let out = build(&client, false, &[], &[]);
        // Only auth + content-type: identical to the pre-switch outbound set.
        assert_eq!(out.len(), 2);
        assert_eq!(out.get("x-api-key").unwrap(), "sk-real-upstream-key");
        assert_eq!(out.get("content-type").unwrap(), "application/json");
    }

    #[test]
    fn switch_off_still_honors_yaml_whitelist() {
        let client = client_headers(&[("x-custom-tenant", "t1"), ("x-app", "cli")]);
        let out = build(&client, false, &["X-Custom-Tenant"], &[]);
        assert_eq!(out.get("x-custom-tenant").unwrap(), "t1");
        assert!(out.get("x-app").is_none());
    }

    #[test]
    fn switch_on_forwards_builtin_whitelist() {
        let client = client_headers(&[
            ("x-claude-code-session-id", "sess-1"),
            ("anthropic-beta", "context-1m-2025"),
            ("anthropic-version", "2023-06-01"),
            ("x-app", "cli"),
            ("x-stainless-lang", "js"),
        ]);
        let out = build(&client, true, &[], &[]);
        assert_eq!(out.get("x-claude-code-session-id").unwrap(), "sess-1");
        assert_eq!(out.get("anthropic-beta").unwrap(), "context-1m-2025");
        assert_eq!(out.get("anthropic-version").unwrap(), "2023-06-01");
        assert_eq!(out.get("x-app").unwrap(), "cli");
        assert_eq!(out.get("x-stainless-lang").unwrap(), "js");
    }

    #[test]
    fn prefix_match_requires_trailing_segment_marker() {
        let client = client_headers(&[
            ("x-stainless-os", "MacOS"),
            ("x-stainless", "bare"),
            ("x-stainlessfoo", "nope"),
        ]);
        let out = build(&client, true, &[], &[]);
        assert_eq!(out.get("x-stainless-os").unwrap(), "MacOS");
        assert!(out.get("x-stainless").is_none());
        assert!(out.get("x-stainlessfoo").is_none());
    }

    #[test]
    fn credentials_and_user_agent_never_forwarded() {
        let client = client_headers(&[
            ("user-agent", "claude-cli/2.0"),
            ("authorization", "Bearer local-token"),
            ("x-api-key", "local-dummy-key"),
            ("cookie", "a=b"),
            ("host", "127.0.0.1:23456"),
            ("content-length", "42"),
        ]);
        let out = build(&client, true, &[], &[]);
        assert!(out.get("user-agent").is_none());
        assert!(out.get("authorization").is_none());
        assert!(out.get("cookie").is_none());
        assert!(out.get("host").is_none());
        assert!(out.get("content-length").is_none());
        // x-api-key present, but with the subscription's value, never the client's.
        assert_eq!(out.get("x-api-key").unwrap(), "sk-real-upstream-key");
    }

    #[test]
    fn yaml_whitelist_unions_with_builtin_without_duplicates() {
        let client = client_headers(&[("anthropic-beta", "flag-a"), ("x-extra", "1")]);
        // yaml lists an already-builtin name plus its own entry.
        let out = build(&client, true, &["anthropic-beta", "x-extra"], &[]);
        assert_eq!(out.get_all("anthropic-beta").iter().count(), 1);
        assert_eq!(out.get("x-extra").unwrap(), "1");
    }

    #[test]
    fn multi_value_headers_keep_all_values() {
        let client = client_headers(&[("anthropic-beta", "flag-a"), ("anthropic-beta", "flag-b")]);
        let out = build(&client, true, &[], &[]);
        let values: Vec<_> = out.get_all("anthropic-beta").iter().collect();
        assert_eq!(values, vec!["flag-a", "flag-b"]);
    }

    #[test]
    fn required_headers_override_forwarded_values() {
        let client = client_headers(&[
            ("anthropic-version", "2099-01-01"),
            ("anthropic-beta", "flag-a"),
            ("anthropic-beta", "flag-b"),
        ]);
        let out = build(
            &client,
            true,
            &[],
            &[("anthropic-version", "2023-06-01"), ("anthropic-beta", "only")],
        );
        assert_eq!(out.get("anthropic-version").unwrap(), "2023-06-01");
        // insert clears every forwarded value under the same name.
        let betas: Vec<_> = out.get_all("anthropic-beta").iter().collect();
        assert_eq!(betas, vec!["only"]);
    }

    #[test]
    fn auth_header_beats_yaml_whitelisted_client_value() {
        // A yaml whitelist that (mis)lists the auth header name must not let
        // the client's own credential reach the upstream.
        let client = client_headers(&[("x-api-key", "client-smuggled")]);
        let out = build(&client, true, &["x-api-key"], &[]);
        let values: Vec<_> = out.get_all("x-api-key").iter().collect();
        assert_eq!(values, vec!["sk-real-upstream-key"]);
    }

    #[test]
    fn content_type_is_always_json() {
        let client = client_headers(&[("content-type", "text/plain")]);
        let out = build(&client, true, &["content-type"], &[]);
        let values: Vec<_> = out.get_all("content-type").iter().collect();
        assert_eq!(values, vec!["application/json"]);
    }

    #[test]
    fn empty_inputs_yield_auth_and_content_type_only() {
        let out = build(&HeaderMap::new(), true, &[], &[]);
        assert_eq!(out.len(), 2);
        assert_eq!(out.get("x-api-key").unwrap(), "sk-real-upstream-key");
        assert_eq!(out.get("content-type").unwrap(), "application/json");
    }
}
