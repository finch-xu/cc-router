//! Outbound header assembly for the Anthropic passthrough path.
//!
//! Which client headers reach the upstream is decided by the subscription's
//! [`HeaderPolicy`] (migration 026, replaces the issue #43 boolean switch).
//! On top of every mode sits a hard-deny set — credentials the client sent to
//! cc-router, hop-by-hop headers, and anything whose name looks like a
//! credential — that no policy, yaml `forward_headers` entry, or allowlist can
//! unblock. The policy never affects headers configured on the subscription
//! itself: `required_headers` (already merged with `extra_headers`) are always
//! sent.
//!
//! Priority on name collision (later insert wins):
//! forwarded client headers < `required_headers` (yaml + user `extra_headers`)
//! < auth header < hard-coded `content-type`.

use std::collections::{BTreeMap, HashSet};

use axum::http::HeaderMap;
use reqwest::header::{
    HeaderMap as ReqHeaderMap, HeaderName as ReqHeaderName, HeaderValue as ReqHeaderValue,
};

use crate::subscription::header_policy::{HeaderPolicy, HeaderPolicyMode};

/// Exact names never forwarded. Lowercase only: axum normalizes inbound names.
const HARD_DENY_EXACT: &[&str] = &[
    "authorization",
    "x-api-key",
    "cookie",
    "host",
    "content-length",
    "content-type",
    // reqwest negotiates its own encoding; forwarding the client's would make
    // the upstream compress with an encoding reqwest may not decode.
    "accept-encoding",
    "connection",
    "keep-alive",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Prefixes never forwarded (`proxy-authorization`, `proxy-connection`, the
/// TUI local-pass secret `x-ccr-local`, the web UI marker `x-ccr-ui`).
const HARD_DENY_PREFIXES: &[&str] = &["proxy-", "x-ccr-"];

/// Substrings that mark a header name as credential-like.
const CREDENTIAL_MARKERS: &[&str] = &["key", "token", "secret", "auth", "cookie", "session"];

/// Protocol header families exempt from the credential heuristic: Claude Code
/// and Anthropic add new names here every release (`x-claude-code-session-id`),
/// and none of them carry secrets — credentials travel in `authorization` /
/// `x-api-key`, which stay hard-denied.
const CREDENTIAL_EXEMPT_PREFIXES: &[&str] = &["x-claude-code-", "anthropic-"];

/// Built-in Claude Code preset (`HeaderPolicyMode::ClaudeCode`).
const CLAUDE_CODE_PREFIXES: &[&str] = &["anthropic-", "x-claude-code-", "x-stainless-"];
const CLAUDE_CODE_EXACT: &[&str] = &["x-app"];

/// Name heuristic shared with request-log redaction: true when a header name
/// (lowercase) contains a credential marker and is not explicitly exempted.
pub fn looks_like_credential(lower_name: &str) -> bool {
    !CREDENTIAL_EXEMPT_PREFIXES.iter().any(|p| lower_name.starts_with(p))
        && CREDENTIAL_MARKERS.iter().any(|m| lower_name.contains(m))
}

/// True when the client header must never reach an upstream, whatever the policy.
pub fn is_hard_denied(lower_name: &str) -> bool {
    HARD_DENY_EXACT.contains(&lower_name)
        || HARD_DENY_PREFIXES.iter().any(|p| lower_name.starts_with(p))
        || looks_like_credential(lower_name)
}

/// Case-insensitive match against a policy name list; `foo-*` is a prefix.
fn list_matches(names: &[String], lower_name: &str) -> bool {
    names.iter().any(|raw| {
        let entry = raw.to_ascii_lowercase();
        match entry.strip_suffix('*') {
            Some(prefix) => lower_name.starts_with(prefix),
            None => entry == lower_name,
        }
    })
}

/// Whether the policy itself selects this header. Does not apply the
/// hard-deny set; callers combine it with [`is_hard_denied`].
pub fn policy_allows(policy: &HeaderPolicy, lower_name: &str) -> bool {
    match policy.mode {
        HeaderPolicyMode::None => false,
        HeaderPolicyMode::ClaudeCode => {
            CLAUDE_CODE_EXACT.contains(&lower_name)
                || CLAUDE_CODE_PREFIXES.iter().any(|p| lower_name.starts_with(p))
        }
        HeaderPolicyMode::Allowlist => list_matches(&policy.names, lower_name),
        HeaderPolicyMode::Denylist => !list_matches(&policy.names, lower_name),
        HeaderPolicyMode::All => true,
    }
}

/// Build the complete outbound header map for the Anthropic passthrough path.
///
/// `required_headers` must already include the user's `extra_headers` (see
/// `SubscriptionRow::resolved_required_headers`). Invalid header names/values
/// are silently skipped, matching the if-let-Ok style at every dispatch site.
pub fn build_anthropic_passthrough_headers(
    client_headers: &HeaderMap,
    policy: &HeaderPolicy,
    yaml_forward: &[String],
    required_headers: &BTreeMap<String, String>,
    auth_header_name: &str,
    auth_header_value: &str,
) -> ReqHeaderMap {
    let mut out = ReqHeaderMap::new();

    // Forwarded client headers go in first so every cc-router-controlled
    // header inserted below overrides them on name collision. Iterate the
    // client map: http's iter yields one pair per value, so multi-value
    // headers (repeated anthropic-beta) survive via append.
    let yaml_set: HashSet<String> = yaml_forward
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    for (name, value) in client_headers.iter() {
        let n = name.as_str();
        if is_hard_denied(n) {
            continue;
        }
        if policy_allows(policy, n) || yaml_set.contains(n) {
            if let (Ok(rn), Ok(rv)) = (
                ReqHeaderName::try_from(n),
                ReqHeaderValue::from_bytes(value.as_bytes()),
            ) {
                out.append(rn, rv);
            }
        }
    }

    // insert (unlike append) drops every previously forwarded value under the
    // same name — configured headers replace, they do not merge.
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
    use crate::subscription::header_policy::{HeaderPolicy, HeaderPolicyMode};
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

    fn policy(mode: HeaderPolicyMode, names: &[&str]) -> HeaderPolicy {
        HeaderPolicy { mode, names: names.iter().map(|s| s.to_string()).collect() }
    }

    fn build(
        client: &HeaderMap,
        policy: &HeaderPolicy,
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
            policy,
            &yaml,
            &required,
            "x-api-key",
            "sk-real-upstream-key",
        )
    }

    fn cc_client() -> HeaderMap {
        client_headers(&[
            ("x-claude-code-session-id", "sess-1"),
            ("x-claude-code-request-class", "main"),
            ("anthropic-beta", "context-1m-2025"),
            ("anthropic-version", "2023-06-01"),
            ("anthropic-workspace-id", "ws-1"),
            ("x-app", "cli"),
            ("x-stainless-os", "MacOS"),
            ("user-agent", "claude-cli/2.1"),
            ("x-foo", "bar"),
        ])
    }

    #[test]
    fn mode_none_forwards_nothing() {
        let out = build(&cc_client(), &HeaderPolicy::none(), &[], &[]);
        assert_eq!(out.len(), 2);
        assert_eq!(out.get("x-api-key").unwrap(), "sk-real-upstream-key");
        assert_eq!(out.get("content-type").unwrap(), "application/json");
    }

    #[test]
    fn mode_none_still_honors_yaml_whitelist() {
        let client = client_headers(&[("x-custom-tenant", "t1"), ("x-app", "cli")]);
        let out = build(&client, &HeaderPolicy::none(), &["X-Custom-Tenant"], &[]);
        assert_eq!(out.get("x-custom-tenant").unwrap(), "t1");
        assert!(out.get("x-app").is_none());
    }

    #[test]
    fn mode_claude_code_forwards_prefixes() {
        let out = build(&cc_client(), &HeaderPolicy::claude_code(), &[], &[]);
        for n in [
            "anthropic-beta",
            "anthropic-version",
            "anthropic-workspace-id",
            "x-claude-code-session-id",
            "x-claude-code-request-class",
            "x-stainless-os",
            "x-app",
        ] {
            assert!(out.get(n).is_some(), "{n} should be forwarded");
        }
        assert!(out.get("user-agent").is_none());
        assert!(out.get("x-foo").is_none());
    }

    #[test]
    fn prefix_match_requires_trailing_segment_marker() {
        let client = client_headers(&[
            ("x-stainless-os", "MacOS"),
            ("x-stainless", "bare"),
            ("x-stainlessfoo", "nope"),
        ]);
        let out = build(&client, &HeaderPolicy::claude_code(), &[], &[]);
        assert_eq!(out.get("x-stainless-os").unwrap(), "MacOS");
        assert!(out.get("x-stainless").is_none());
        assert!(out.get("x-stainlessfoo").is_none());
    }

    #[test]
    fn mode_allowlist_exact_and_prefix() {
        let client = client_headers(&[
            ("x-team", "a"),
            ("x-trace-id", "t"),
            ("x-teamx", "no"),
            ("anthropic-beta", "no"),
        ]);
        let out = build(&client, &policy(HeaderPolicyMode::Allowlist, &["X-Team", "x-trace-*"]), &[], &[]);
        assert_eq!(out.get("x-team").unwrap(), "a");
        assert_eq!(out.get("x-trace-id").unwrap(), "t");
        assert!(out.get("x-teamx").is_none());
        assert!(out.get("anthropic-beta").is_none());
    }

    #[test]
    fn mode_denylist_excludes_listed() {
        let out = build(&cc_client(), &policy(HeaderPolicyMode::Denylist, &["x-stainless-*"]), &[], &[]);
        assert_eq!(out.get("x-app").unwrap(), "cli");
        assert_eq!(out.get("user-agent").unwrap(), "claude-cli/2.1");
        assert!(out.get("x-stainless-os").is_none());
    }

    #[test]
    fn mode_all_still_blocks_hard_denied() {
        let client = client_headers(&[
            ("authorization", "Bearer local"),
            ("x-api-key", "local-dummy"),
            ("cookie", "a=b"),
            ("host", "127.0.0.1:23456"),
            ("content-length", "42"),
            ("accept-encoding", "gzip"),
            ("connection", "keep-alive"),
            ("proxy-connection", "keep-alive"),
            ("x-ccr-local", "secret"),
            ("x-goog-api-key", "g"),
            ("x-session-token", "s"),
            ("x-claude-code-session-id", "sess-1"),
            ("openai-organization", "org"),
            ("x-foo", "bar"),
        ]);
        let out = build(&client, &policy(HeaderPolicyMode::All, &[]), &[], &[]);
        assert_eq!(out.get("x-claude-code-session-id").unwrap(), "sess-1");
        assert_eq!(out.get("openai-organization").unwrap(), "org");
        assert_eq!(out.get("x-foo").unwrap(), "bar");
        for n in [
            "authorization",
            "cookie",
            "host",
            "content-length",
            "accept-encoding",
            "connection",
            "proxy-connection",
            "x-ccr-local",
            "x-goog-api-key",
            "x-session-token",
        ] {
            assert!(out.get(n).is_none(), "{n} must never be forwarded");
        }
        let keys: Vec<_> = out.get_all("x-api-key").iter().collect();
        assert_eq!(keys, vec!["sk-real-upstream-key"]);
    }

    #[test]
    fn allowlist_cannot_unblock_hard_denied() {
        let client = client_headers(&[("authorization", "Bearer local"), ("cookie", "a=b")]);
        let out = build(&client, &policy(HeaderPolicyMode::Allowlist, &["authorization", "cookie"]), &[], &[]);
        assert!(out.get("authorization").is_none());
        assert!(out.get("cookie").is_none());
    }

    #[test]
    fn yaml_whitelist_cannot_unblock_hard_denied() {
        let client = client_headers(&[("cookie", "a=b")]);
        let out = build(&client, &HeaderPolicy::none(), &["cookie"], &[]);
        assert!(out.get("cookie").is_none());
    }

    #[test]
    fn multi_value_preserved_in_every_forwarding_mode() {
        let client = client_headers(&[("anthropic-beta", "flag-a"), ("anthropic-beta", "flag-b")]);
        for p in [HeaderPolicy::claude_code(), policy(HeaderPolicyMode::All, &[])] {
            let out = build(&client, &p, &[], &[]);
            let values: Vec<_> = out.get_all("anthropic-beta").iter().collect();
            assert_eq!(values, vec!["flag-a", "flag-b"], "{:?}", p.mode);
        }
    }

    #[test]
    fn yaml_whitelist_unions_with_policy_without_duplicates() {
        let client = client_headers(&[("anthropic-beta", "flag-a"), ("x-extra", "1")]);
        let out = build(&client, &HeaderPolicy::claude_code(), &["anthropic-beta", "x-extra"], &[]);
        assert_eq!(out.get_all("anthropic-beta").iter().count(), 1);
        assert_eq!(out.get("x-extra").unwrap(), "1");
    }

    #[test]
    fn extra_overrides_forwarded_same_name() {
        let client = client_headers(&[("anthropic-beta", "a,b")]);
        let out = build(&client, &HeaderPolicy::claude_code(), &[], &[("anthropic-beta", "c")]);
        let values: Vec<_> = out.get_all("anthropic-beta").iter().collect();
        assert_eq!(values, vec!["c"]);
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
            &HeaderPolicy::claude_code(),
            &[],
            &[("anthropic-version", "2023-06-01"), ("anthropic-beta", "only")],
        );
        assert_eq!(out.get("anthropic-version").unwrap(), "2023-06-01");
        let betas: Vec<_> = out.get_all("anthropic-beta").iter().collect();
        assert_eq!(betas, vec!["only"]);
    }

    #[test]
    fn auth_header_beats_yaml_whitelisted_client_value() {
        let client = client_headers(&[("x-api-key", "client-smuggled")]);
        let out = build(&client, &policy(HeaderPolicyMode::All, &[]), &["x-api-key"], &[]);
        let values: Vec<_> = out.get_all("x-api-key").iter().collect();
        assert_eq!(values, vec!["sk-real-upstream-key"]);
    }

    #[test]
    fn content_type_is_always_json() {
        let client = client_headers(&[("content-type", "text/plain")]);
        let out = build(&client, &policy(HeaderPolicyMode::All, &[]), &["content-type"], &[]);
        let values: Vec<_> = out.get_all("content-type").iter().collect();
        assert_eq!(values, vec!["application/json"]);
    }

    #[test]
    fn credential_predicate() {
        for n in ["x-api-key", "x-goog-api-key", "x-session-token", "x-client-secret", "proxy-authorization", "set-cookie", "x-auth", "x-session"] {
            assert!(looks_like_credential(n), "{n}");
        }
        for n in [
            "x-claude-code-session-id",
            "x-claude-code-session-parent",
            "anthropic-beta",
            "anthropic-workspace-key-hint",
            "x-app",
            "openai-organization",
        ] {
            assert!(!looks_like_credential(n), "{n}");
        }
    }
}
