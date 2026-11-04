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

/// Outbound variant: additionally masks any header whose value contains one of
/// `secrets` (the subscription's API key / OAuth token), whatever its name —
/// custom auth header names and `{api_key}` template headers would slip past
/// the name heuristic. Secrets shorter than 8 chars are ignored.
pub fn serialize_outbound(map: &HeaderMap, enabled: bool, secrets: &[&str]) -> Option<String> {
    if !enabled {
        return None;
    }
    // reqwest fills in its client-level UA at send time; show what actually goes on the wire.
    if map.contains_key(axum::http::header::USER_AGENT) {
        return serialize_with(map, true, secrets);
    }
    let mut with_ua = map.clone();
    with_ua.insert(
        axum::http::header::USER_AGENT,
        axum::http::HeaderValue::from_static(crate::proxy::upstream::DEFAULT_USER_AGENT),
    );
    serialize_with(&with_ua, true, secrets)
}

fn serialize_with(map: &HeaderMap, enabled: bool, secrets: &[&str]) -> Option<String> {
    if !enabled {
        return None;
    }
    let secrets: Vec<&str> = secrets.iter().copied().filter(|s| s.len() >= 8).collect();
    let mut pairs: Vec<[String; 2]> = map
        .iter()
        .map(|(name, value)| {
            let n = name.as_str().to_ascii_lowercase();
            let raw = String::from_utf8_lossy(value.as_bytes());
            let v = if secrets.iter().any(|sec| raw.contains(sec)) {
                mask(&raw)
            } else {
                redact_value(&n, &raw)
            };
            [n, v]
        })
        .collect();
    let mut out = serde_json::to_string(&pairs).ok()?;
    if out.len() <= HEADER_LOG_LIMIT {
        return Some(out);
    }
    // Drop entries from the tail until the list plus the marker fits.
    let marker = ["…".to_string(), "truncated".to_string()];
    while !pairs.is_empty() {
        pairs.pop();
        pairs.push(marker.clone());
        out = serde_json::to_string(&pairs).ok()?;
        pairs.pop();
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
    pub request: Option<String>,
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
        let got = pairs(&serialize_outbound(&m, true, &[key]).unwrap());
        for p in &got {
            assert!(!p[1].contains("0123456789"), "{p:?} leaked the key");
        }
        assert_eq!(got.iter().find(|p| p[0] == "x-team").unwrap()[1], "platform");
        assert_eq!(serialize_outbound(&m, false, &[key]), None);
        // empty / short secrets are ignored (would mask everything)
        let got = pairs(&serialize_outbound(&map(&[("x-team", "platform")]), true, &["", "ab"]).unwrap());
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
        let got = pairs(&serialize_outbound(&map(&[("x-app", "cli")]), true, &[]).unwrap());
        let ua = got.iter().find(|p| p[0] == "user-agent").expect("default UA row");
        assert_eq!(ua[1], crate::proxy::upstream::DEFAULT_USER_AGENT);
        // explicit UA (Codex / Kiro) is kept as-is, not duplicated
        let got = pairs(&serialize_outbound(&map(&[("user-agent", "codex_cli_rs/1.0")]), true, &[]).unwrap());
        assert_eq!(got.iter().filter(|p| p[0] == "user-agent").count(), 1);
        assert_eq!(got[0][1], "codex_cli_rs/1.0");
    }

    #[test]
    fn parse_rejects_garbage() {
        assert_eq!(parse("not json"), None);
        assert_eq!(parse("[]"), Some(vec![]));
        assert_eq!(parse(r#"[["a"]]"#), None, "wrong pair shape");
    }
}
