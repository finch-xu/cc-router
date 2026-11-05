//! Request-log capture of the three header sets shown in the request detail
//! dialog: what the client sent, what cc-router sent upstream, and what the
//! upstream returned (migration 027).
//!
//! Same discipline as `effort_log.rs` / `tool_log.rs`: capturing must never
//! affect log delivery or the request itself. Every function here is total —
//! no panics, no errors, `None` when there is nothing to record.
//!
//! Stored shape: a JSON array of `[name, value]` pairs (lowercase names, order
//! and duplicates preserved), credential values masked, capped at
//! [`HEADER_LOG_LIMIT`] bytes.

use axum::http::HeaderMap;

use crate::proxy::header_source::HeaderSource;

/// Upper bound for one serialized column.
pub const HEADER_LOG_LIMIT: usize = 16 * 1024;

/// Credential header names masked regardless of the name heuristic.
const ALWAYS_MASKED: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "x-api-key",
    "cookie",
    "set-cookie",
    "x-ccr-local",
];

/// Masks credential values: longer than 12 chars keeps the first and last 4,
/// anything shorter becomes `•••`. Other headers are returned unchanged.
pub fn redact_value(lower_name: &str, value: &str) -> String {
    if ALWAYS_MASKED.contains(&lower_name) || log_name_is_secret(lower_name) || value_is_secret(value) {
        mask(value)
    } else {
        value.to_string()
    }
}

/// Header families that never carry secrets and are what people read logs for
/// (rate-limit budgets count "tokens", Claude Code ids contain "session").
const LOG_EXEMPT_PREFIXES: &[&str] = &["x-claude-code-", "anthropic-", "x-ratelimit-", "ratelimit-"];

/// Name substrings that mark a credential in the log. Deny-by-default: any
/// header containing one is masked unless its family is in
/// [`LOG_EXEMPT_PREFIXES`] — that narrow allowlist is what keeps rate-limit
/// budgets ("…-tokens") and Claude Code ids ("…-session-id") readable.
const LOG_SECRET_MARKERS: &[&str] = &[
    "key", "token", "secret", "pass", "credential", "signature", "cookie", "auth", "session",
];

/// Log-side name heuristic: the forwarding markers plus pass / credential /
/// signature, with known-safe families exempted first.
fn log_name_is_secret(lower_name: &str) -> bool {
    if LOG_EXEMPT_PREFIXES.iter().any(|p| lower_name.starts_with(p)) {
        return false;
    }
    LOG_SECRET_MARKERS.iter().any(|m| lower_name.contains(m))
}

/// Values that are credentials whatever the header is called.
fn value_is_secret(value: &str) -> bool {
    let v = value.trim_start().to_ascii_lowercase();
    v.starts_with("bearer ") || v.starts_with("basic ")
}

/// `•••` for short values, otherwise first 4 + `…` + last 4.
fn mask(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 12 {
        return "•••".to_string();
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

/// Serializes a header map for the log. `None` when `enabled` is false.
pub fn serialize(map: &HeaderMap, enabled: bool) -> Option<String> {
    serialize_with(map, enabled, &[])
}

/// Outbound headers of a dispatch that assembles its own reqwest map (the
/// translation paths and System One): masked pairs, reqwest's default UA
/// added when none is set. Sources and automatic Accept-Encoding are filled in by [`tag_capture`],
/// once the pipeline has the subscription's [`SourceHints`] at hand.
///
/// Masking additionally covers any header whose value contains one of
/// `secrets` (the subscription's API key / OAuth token), whatever its name —
/// custom auth header names and `{api_key}` template headers would slip past
/// the name heuristic. Secrets shorter than 8 chars are ignored.
pub fn capture_untagged(map: &HeaderMap, enabled: bool, secrets: &[&str]) -> Option<OutboundCapture> {
    enabled.then(|| OutboundCapture::Untagged(untagged_pairs(map, secrets)))
}

fn untagged_pairs(map: &HeaderMap, secrets: &[&str]) -> Vec<[String; 2]> {
    let mut pairs: Vec<[String; 2]> = map
        .iter()
        .map(|(name, value)| {
            let n = name.as_str().to_ascii_lowercase();
            let raw = String::from_utf8_lossy(value.as_bytes());
            let v = redact_with(&n, &raw, secrets);
            [n, v]
        })
        .collect();
    // reqwest fills in its client-level UA at send time; show what actually goes on the wire.
    if !map.contains_key(axum::http::header::USER_AGENT) {
        pairs.push(["user-agent".to_string(), crate::proxy::upstream::DEFAULT_USER_AGENT.to_string()]);
    }
    pairs
}

/// Outbound request headers captured at send time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboundCapture {
    /// Already `[[name, value, source]]` JSON (passthrough path).
    Tagged(String),
    /// Masked `[name, value]` pairs awaiting [`tag_capture`].
    Untagged(Vec<[String; 2]>),
}

/// What the classifier needs to know about a subscription.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceHints {
    /// Empty means `authorization`.
    pub auth_header_name: String,
    /// Header names the user configured.
    pub custom_names: Vec<String>,
    /// yaml `forward_headers`.
    pub forward_names: Vec<String>,
}

/// Source of one outbound header on a path that did not record it while
/// assembling. First match wins, names compared case-insensitively:
/// content-type / accept → cc-router; auth header → auth; default UA →
/// cc-router; yaml-forwarded name whose client value is identical →
/// forwarded (a dispatch that rewrites it afterwards, like the Codex UA,
/// changes the value and falls through); user-configured name → custom;
/// anything else → provider. `value` is the masked value; the client side is
/// masked the same way before comparing.
pub fn classify(name: &str, value: &str, hints: &SourceHints, client: &HeaderMap) -> HeaderSource {
    let n = name.to_ascii_lowercase();
    if n == "content-type" || n == "accept" {
        return HeaderSource::CcRouter;
    }
    let auth = if hints.auth_header_name.is_empty() { "authorization" } else { hints.auth_header_name.as_str() };
    if n.eq_ignore_ascii_case(auth) {
        return HeaderSource::Auth;
    }
    if n == "user-agent" && value.starts_with("cc-router/") {
        return HeaderSource::CcRouter;
    }
    if hints.forward_names.iter().any(|f| f.eq_ignore_ascii_case(&n))
        && client
            .get_all(n.as_str())
            .iter()
            .any(|cv| redact_value(&n, &String::from_utf8_lossy(cv.as_bytes())) == value)
    {
        return HeaderSource::Forwarded;
    }
    if hints.custom_names.iter().any(|c| c.eq_ignore_ascii_case(&n)) {
        return HeaderSource::Custom;
    }
    HeaderSource::Provider
}

/// Final `upstream_request_headers` column: tagged JSON as-is, untagged pairs
/// classified with [`classify`] and serialized with the usual size cap.
pub fn tag_capture(cap: OutboundCapture, hints: &SourceHints, client: &HeaderMap) -> Option<String> {
    match cap {
        OutboundCapture::Tagged(json) => Some(json),
        OutboundCapture::Untagged(pairs) => {
            let mut rows: Vec<[String; 3]> = pairs
                .into_iter()
                .map(|[n, v]| {
                    let src = classify(&n, &v, hints, client).as_str().to_string();
                    [n, v, src]
                })
                .collect();
            append_auto_encoding(&mut rows);
            let marker = ["…".to_string(), "truncated".to_string(), HeaderSource::CcRouter.as_str().to_string()];
            to_capped_json(rows, marker)
        }
    }
}

/// Add only after classifying explicit headers: a configured encoding remains
/// custom/provider/forwarded even if its value happens to equal the default.
fn append_auto_encoding(rows: &mut Vec<[String; 3]>) {
    if !rows.iter().any(|r| r[0] == "accept-encoding") {
        rows.push([
            "accept-encoding".to_string(),
            crate::proxy::upstream::DEFAULT_ACCEPT_ENCODING.to_string(),
            HeaderSource::CcRouter.as_str().to_string(),
        ]);
    }
}

fn serialize_with(map: &HeaderMap, enabled: bool, secrets: &[&str]) -> Option<String> {
    if !enabled {
        return None;
    }
    let pairs: Vec<[String; 2]> = map
        .iter()
        .map(|(name, value)| {
            let n = name.as_str().to_ascii_lowercase();
            let raw = String::from_utf8_lossy(value.as_bytes());
            let v = redact_with(&n, &raw, secrets);
            [n, v]
        })
        .collect();
    let marker = ["…".to_string(), "truncated".to_string()];
    to_capped_json(pairs, marker)
}

/// Masked value: the subscription's own secrets (≥ 8 chars) wherever they
/// appear, otherwise the name / value heuristics of [`redact_value`].
pub fn redact_with(lower_name: &str, raw: &str, secrets: &[&str]) -> String {
    if secrets.iter().any(|sec| sec.len() >= 8 && raw.contains(sec)) {
        mask(raw)
    } else {
        redact_value(lower_name, raw)
    }
}

/// Outbound headers with their source as `[[name, value, source]]`. Adds
/// reqwest's default UA and Accept-Encoding when none is set, like
/// [`capture_untagged`] + [`tag_capture`]. Same masking and size cap.
pub fn serialize_tagged(
    sent: &[(String, String, HeaderSource)],
    enabled: bool,
    secrets: &[&str],
) -> Option<String> {
    if !enabled {
        return None;
    }
    let mut rows: Vec<[String; 3]> = sent
        .iter()
        .map(|(n, v, src)| {
            let n = n.to_ascii_lowercase();
            let v = redact_with(&n, v, secrets);
            [n, v, src.as_str().to_string()]
        })
        .collect();
    if !rows.iter().any(|r| r[0] == "user-agent") {
        rows.push([
            "user-agent".to_string(),
            crate::proxy::upstream::DEFAULT_USER_AGENT.to_string(),
            HeaderSource::CcRouter.as_str().to_string(),
        ]);
    }
    append_auto_encoding(&mut rows);
    let marker = ["…".to_string(), "truncated".to_string(), HeaderSource::CcRouter.as_str().to_string()];
    to_capped_json(rows, marker)
}

/// [`serialize_tagged`] wrapped for [`UpstreamHeaderCapture::request`].
pub fn capture_tagged(
    sent: &[(String, String, HeaderSource)],
    enabled: bool,
    secrets: &[&str],
) -> Option<OutboundCapture> {
    serialize_tagged(sent, enabled, secrets).map(OutboundCapture::Tagged)
}

/// JSON of `rows`, capped at [`HEADER_LOG_LIMIT`] by dropping entries from
/// the tail and appending `marker`.
fn to_capped_json<T: serde::Serialize + Clone>(mut rows: Vec<T>, marker: T) -> Option<String> {
    let mut out = serde_json::to_string(&rows).ok()?;
    if out.len() <= HEADER_LOG_LIMIT {
        return Some(out);
    }
    while !rows.is_empty() {
        rows.pop();
        rows.push(marker.clone());
        out = serde_json::to_string(&rows).ok()?;
        rows.pop();
        if out.len() <= HEADER_LOG_LIMIT {
            return Some(out);
        }
    }
    serde_json::to_string(&[marker]).ok()
}

/// Parses a stored column back into pairs; malformed JSON → None.
pub fn parse(stored: &str) -> Option<Vec<[String; 2]>> {
    serde_json::from_str(stored).ok()
}

/// The three header columns of one request-log row. Deliberately no `Default`:
/// every construction site has to decide what it records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderLogFields {
    pub client_request: Option<String>,
    pub upstream_request: Option<String>,
    pub upstream_response: Option<String>,
}

impl HeaderLogFields {
    /// For log rows not tied to a real request (test and cleanup fixtures).
    pub fn none() -> Self {
        Self { client_request: None, upstream_request: None, upstream_response: None }
    }
}

/// Filled by a dispatch right before / after it sends; the caller copies it
/// into the log entry.
#[derive(Debug, Clone, Default)]
pub struct UpstreamHeaderCapture {
    pub request: Option<OutboundCapture>,
    pub response: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderName, HeaderValue};

    fn map(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.append(HeaderName::try_from(*k).unwrap(), HeaderValue::from_str(v).unwrap());
        }
        m
    }

    fn untagged(m: &HeaderMap, enabled: bool, secrets: &[&str]) -> Vec<[String; 2]> {
        match capture_untagged(m, enabled, secrets) {
            Some(OutboundCapture::Untagged(p)) => p,
            other => panic!("expected untagged capture, got {other:?}"),
        }
    }

    fn pairs(stored: &str) -> Vec<[String; 2]> {
        parse(stored).expect("valid stored json")
    }

    #[test]
    fn redacts_credentials_keeps_others() {
        assert_eq!(redact_value("authorization", "Bearer sk-ant-abcdefghijklmnop"), "Bear…mnop");
        assert_eq!(redact_value("x-api-key", "short"), "•••");
        assert_eq!(redact_value("x-goog-api-key", "AIzaSyD-0123456789abcdef"), "AIza…cdef");
        assert_eq!(redact_value("set-cookie", "__cf_bm=abcdefghijklmnopqrstu"), "__cf…rstu");
        assert_eq!(redact_value("anthropic-beta", "context-1m-2025-08-07"), "context-1m-2025-08-07");
        assert_eq!(redact_value("x-claude-code-session-id", "4b1e0c9a-2d7f"), "4b1e0c9a-2d7f");
        assert_eq!(redact_value("content-type", "application/json"), "application/json");
    }

    #[test]
    fn serialize_preserves_order_and_duplicates() {
        let m = map(&[("anthropic-beta", "a"), ("anthropic-beta", "b"), ("x-app", "cli")]);
        let got = pairs(&serialize(&m, true).unwrap());
        assert_eq!(
            got,
            vec![
                ["anthropic-beta".to_string(), "a".to_string()],
                ["anthropic-beta".to_string(), "b".to_string()],
                ["x-app".to_string(), "cli".to_string()],
            ]
        );
    }

    #[test]
    fn serialize_masks_credentials() {
        let m = map(&[("x-api-key", "sk-ant-api03-0123456789abcdef")]);
        let got = pairs(&serialize(&m, true).unwrap());
        assert_eq!(got[0][1], "sk-a…cdef");
    }

    #[test]
    fn disabled_returns_none() {
        assert_eq!(serialize(&map(&[("x-app", "cli")]), false), None);
    }

    #[test]
    fn truncates_to_limit_with_marker() {
        let big = "v".repeat(200);
        let mut m = HeaderMap::new();
        for i in 0..200 {
            m.append(
                HeaderName::try_from(format!("x-h-{i}")).unwrap(),
                HeaderValue::from_str(&big).unwrap(),
            );
        }
        let s = serialize(&m, true).unwrap();
        assert!(s.len() <= HEADER_LOG_LIMIT, "{}", s.len());
        let got = pairs(&s);
        assert_eq!(got.last().unwrap(), &["…".to_string(), "truncated".to_string()]);
        assert!(got.len() > 10, "keeps as many entries as fit");
    }

    #[test]
    fn non_utf8_value_is_lossy_not_panic() {
        let mut m = HeaderMap::new();
        m.insert(HeaderName::from_static("x-weird"), HeaderValue::from_bytes(&[0x66, 0xff, 0x6f]).unwrap());
        let got = pairs(&serialize(&m, true).unwrap());
        assert!(got[0][1].starts_with('f') && got[0][1].ends_with('o'), "{:?}", got[0][1]);
    }

    /// 出站头: 订阅自己的 Key 不论落在哪个头名下 (自定义鉴权头名 / {api_key} 模板头) 都被打码。
    #[test]
    fn outbound_masks_secret_wherever_it_appears() {
        let key = "sk-live-0123456789abcdef";
        let m = map(&[
            ("x-gateway", key),
            ("cf-aig-routing", "Bearer sk-live-0123456789abcdef"),
            ("x-team", "platform"),
        ]);
        let got = untagged(&m, true, &[key]);
        for p in &got {
            assert!(!p[1].contains("0123456789"), "{p:?} leaked the key");
        }
        assert_eq!(got.iter().find(|p| p[0] == "x-team").unwrap()[1], "platform");
        assert_eq!(capture_untagged(&m, false, &[key]), None);
        // empty / short secrets are ignored (would mask everything)
        let got = untagged(&map(&[("x-team", "platform")]), true, &["", "ab"]);
        assert_eq!(got[0][1], "platform");
    }

    /// 日志打码规则独立于出站转发的启发式: 限流类头要原样可见, 第三方凭据头 / Bearer 值要打码。
    #[test]
    fn log_redaction_keeps_ratelimit_and_masks_third_party_credentials() {
        for (n, v) in [
            ("x-ratelimit-remaining-tokens", "39000"),
            ("x-ratelimit-limit-tokens", "40000"),
            ("x-ratelimit-reset-tokens", "6m0s"),
            ("anthropic-ratelimit-unified-status", "allowed"),
            ("x-claude-code-session-id", "4b1e0c9a-2d7f-4c88"),
        ] {
            assert_eq!(redact_value(n, v), v, "{n} must stay visible");
        }
        for (n, v) in [
            ("x-gateway-pass", "hunter2-hunter2-hunter2"),
            ("x-password", "hunter2-hunter2-hunter2"),
            ("x-client-credential", "abcdefghijklmnopqrst"),
            ("x-signature", "abcdefghijklmnopqrst"),
            ("x-access-token", "abcdefghijklmnopqrst"),
            // deny-by-default: session ids and tokens anywhere in the name
            ("x-session-id", "abcdefghijklmnopqrst"),
            ("x-token-v2", "abcdefghijklmnopqrst"),
            ("x-auth-token-ref", "abcdefghijklmnopqrst"),
            ("x-custom-relay", "Bearer abcdefghijklmnopqrst"),
            ("x-custom-relay2", "Basic dXNlcjpwYXNzd29yZA=="),
        ] {
            assert_ne!(redact_value(n, v), v, "{n} must be masked");
        }
    }

    /// reqwest 在发送时补上客户端默认 UA; 出站表要如实显示它 (网关 / WAF 排障最想看的就是 UA)。
    #[test]
    fn outbound_shows_default_user_agent_when_not_set() {
        let got = untagged(&map(&[("x-app", "cli")]), true, &[]);
        let ua = got.iter().find(|p| p[0] == "user-agent").expect("default UA row");
        assert_eq!(ua[1], crate::proxy::upstream::DEFAULT_USER_AGENT);
        // explicit UA (Codex / Kiro) is kept as-is, not duplicated
        let got = untagged(&map(&[("user-agent", "codex_cli_rs/1.0")]), true, &[]);
        assert_eq!(got.iter().filter(|p| p[0] == "user-agent").count(), 1);
        assert_eq!(got[0][1], "codex_cli_rs/1.0");
    }

    #[test]
    fn serialize_tagged_adds_default_ua_and_masks() {
        use crate::proxy::header_source::HeaderSource;
        let sent = vec![(
            "x-api-key".to_string(),
            "sk-ant-abcdefghijklmnop".to_string(),
            HeaderSource::Auth,
        )];
        let got: Vec<[String; 3]> =
            serde_json::from_str(&serialize_tagged(&sent, true, &[]).unwrap()).unwrap();
        assert_eq!(got[0], ["x-api-key".to_string(), "sk-a…mnop".to_string(), "auth".to_string()]);
        assert_eq!(
            got[1],
            [
                "user-agent".to_string(),
                crate::proxy::upstream::DEFAULT_USER_AGENT.to_string(),
                "cc_router".to_string()
            ]
        );
        assert_eq!(serialize_tagged(&sent, false, &[]), None);
    }

    fn hints(auth: &str, custom: &[&str], forward: &[&str]) -> SourceHints {
        SourceHints {
            auth_header_name: auth.to_string(),
            custom_names: custom.iter().map(|s| s.to_string()).collect(),
            forward_names: forward.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn classify_codex_forced_ua_is_provider() {
        let client = map(&[("user-agent", "claude-cli/2.1")]);
        let h = hints("authorization", &[], &["user-agent"]);
        assert_eq!(
            classify("user-agent", "codex_cli_rs/0.1 (Mac OS; arm64) cc-router/6.4.0", &h, &client),
            HeaderSource::Provider
        );
    }

    #[test]
    fn classify_forwarded_same_value() {
        let client = map(&[("x-trace", "t1")]);
        let h = hints("authorization", &[], &["X-Trace"]);
        assert_eq!(classify("x-trace", "t1", &h, &client), HeaderSource::Forwarded);
        assert_eq!(classify("x-trace", "t2", &h, &client), HeaderSource::Provider);
    }

    #[test]
    fn classify_custom_and_auth_and_default() {
        let client = HeaderMap::new();
        let h = hints("x-goog-api-key", &["X-Team"], &[]);
        assert_eq!(classify("x-team", "p", &h, &client), HeaderSource::Custom);
        assert_eq!(classify("x-goog-api-key", "AIza…abcd", &h, &client), HeaderSource::Auth);
        assert_eq!(classify("anthropic-version", "2023-06-01", &h, &client), HeaderSource::Provider);
        assert_eq!(classify("user-agent", "cc-router/1", &h, &client), HeaderSource::CcRouter);
        assert_eq!(classify("content-type", "application/json", &h, &client), HeaderSource::CcRouter);
        assert_eq!(classify("accept", "text/event-stream", &h, &client), HeaderSource::CcRouter);
        let empty_auth = hints("", &[], &[]);
        assert_eq!(classify("authorization", "Bear…mnop", &empty_auth, &client), HeaderSource::Auth);
    }

    #[test]
    fn tag_capture_tags_untagged() {
        let cap = OutboundCapture::Untagged(vec![
            ["authorization".to_string(), "Bear…mnop".to_string()],
            ["x-team".to_string(), "p".to_string()],
        ]);
        let got: Vec<[String; 3]> = serde_json::from_str(
            &tag_capture(cap, &hints("", &["x-team"], &[]), &HeaderMap::new()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            got,
            vec![
                ["authorization".to_string(), "Bear…mnop".to_string(), "auth".to_string()],
                ["x-team".to_string(), "p".to_string(), "custom".to_string()],
                ["accept-encoding".to_string(), crate::proxy::upstream::DEFAULT_ACCEPT_ENCODING.to_string(), "cc_router".to_string()],
            ]
        );
        let tagged = OutboundCapture::Tagged("[]".to_string());
        assert_eq!(tag_capture(tagged, &hints("", &[], &[]), &HeaderMap::new()).as_deref(), Some("[]"));
    }

    #[test]
    fn parse_rejects_garbage() {
        assert_eq!(parse("not json"), None);
        assert_eq!(parse("[]"), Some(vec![]));
        assert_eq!(parse(r#"[["a"]]"#), None, "wrong pair shape");
    }
}
