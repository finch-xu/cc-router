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

use crate::proxy::forward::looks_like_credential;

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
    if !(ALWAYS_MASKED.contains(&lower_name) || looks_like_credential(lower_name)) {
        return value.to_string();
    }
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
    if !enabled {
        return None;
    }
    let mut pairs: Vec<[String; 2]> = map
        .iter()
        .map(|(name, value)| {
            let n = name.as_str().to_ascii_lowercase();
            let v = String::from_utf8_lossy(value.as_bytes());
            let v = redact_value(&n, &v);
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

    #[test]
    fn parse_rejects_garbage() {
        assert_eq!(parse("not json"), None);
        assert_eq!(parse("[]"), Some(vec![]));
        assert_eq!(parse(r#"[["a"]]"#), None, "wrong pair shape");
    }
}
