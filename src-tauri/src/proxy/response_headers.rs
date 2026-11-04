//! Upstream → client response handling for the Anthropic passthrough path
//! (Claude Code gateway compatibility guide, "response headers").
//!
//! Claude Code reads `retry-after` / `x-should-retry` to decide retries and
//! `anthropic-ratelimit-unified-*` to show plan usage, and its capability-
//! rejection recovery matches the upstream's error wording, so error bodies
//! must reach it unchanged.

use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use bytes::Bytes;

/// Exact upstream response headers returned to the client.
const COPY_EXACT: &[&str] = &[
    "content-type",
    "cache-control",
    "retry-after",
    "x-should-retry",
    "request-id",
];

/// Prefixes of upstream response headers returned to the client.
const COPY_PREFIXES: &[&str] = &["anthropic-ratelimit-"];

/// Copies the whitelisted upstream response headers. `transfer-encoding` is not
/// copied: hyper frames the body itself.
pub fn copy_passthrough_response_headers(from: &reqwest::header::HeaderMap, to: &mut HeaderMap) {
    for (name, value) in from.iter() {
        let n = name.as_str();
        let wanted = COPY_EXACT.contains(&n) || COPY_PREFIXES.iter().any(|p| n.starts_with(p));
        if !wanted {
            continue;
        }
        if let (Ok(hn), Ok(hv)) = (HeaderName::try_from(n), HeaderValue::from_bytes(value.as_bytes())) {
            to.append(hn, hv);
        }
    }
}

/// Last upstream 4xx seen on the passthrough path, kept so that when every
/// candidate fails the client gets the upstream's own status and body instead
/// of cc-router's 503 summary.
#[derive(Debug, Clone)]
pub struct UpstreamError {
    pub status: StatusCode,
    pub headers: reqwest::header::HeaderMap,
    /// Raw body bytes as the upstream sent them.
    pub body: Bytes,
}

impl UpstreamError {
    /// Only client errors are worth replaying; 5xx / network failures are
    /// cc-router's to summarize. 401/403 are excluded too: they mean the
    /// *subscription's* upstream key is bad, and replaying them would make
    /// Claude Code report its own (cc-router) token as invalid.
    pub fn is_replayable(status: StatusCode) -> bool {
        status.is_client_error()
            && status != StatusCode::UNAUTHORIZED
            && status != StatusCode::FORBIDDEN
    }

    /// Response with the upstream status, original body bytes, and whitelisted
    /// headers. Missing content-type defaults to `application/json`.
    pub fn into_response(self) -> Response {
        let mut response = Response::new(axum::body::Body::from(self.body));
        *response.status_mut() = self.status;
        let hdrs = response.headers_mut();
        copy_passthrough_response_headers(&self.headers, hdrs);
        if !hdrs.contains_key(axum::http::header::CONTENT_TYPE) {
            hdrs.insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
        }
        response
    }
}

/// `retry-after` seconds for cc-router's own 503: the shortest remaining
/// cooldown among candidates, clamped to 1..=60 (Claude Code stops retrying
/// above 60). `None` when no candidate is cooling down.
pub fn retry_after_secs(remaining_cooldowns_ms: impl IntoIterator<Item = i64>) -> Option<u64> {
    remaining_cooldowns_ms
        .into_iter()
        .filter(|ms| *ms > 0)
        .min()
        .map(|ms| ((ms as u64).div_ceil(1000)).clamp(1, 60))
}

/// Inserts `retry-after` when `secs` is Some.
pub fn set_retry_after(headers: &mut HeaderMap, secs: Option<u64>) {
    if let Some(s) = secs {
        if let Ok(v) = HeaderValue::from_str(&s.to_string()) {
            headers.insert(HeaderName::from_static("retry-after"), v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upstream(pairs: &[(&str, &str)]) -> reqwest::header::HeaderMap {
        let mut m = reqwest::header::HeaderMap::new();
        for (k, v) in pairs {
            m.append(
                reqwest::header::HeaderName::try_from(*k).unwrap(),
                reqwest::header::HeaderValue::from_str(v).unwrap(),
            );
        }
        m
    }

    #[test]
    fn copies_whitelisted_and_ratelimit_prefix() {
        let from = upstream(&[
            ("retry-after", "12"),
            ("x-should-retry", "false"),
            ("request-id", "req_1"),
            ("anthropic-ratelimit-unified-status", "allowed"),
            ("anthropic-ratelimit-unified-reset", "1793820000"),
            ("content-type", "text/event-stream"),
            ("cf-ray", "abc"),
            ("set-cookie", "a=b"),
            ("server", "cloudflare"),
            ("transfer-encoding", "chunked"),
        ]);
        let mut to = HeaderMap::new();
        copy_passthrough_response_headers(&from, &mut to);
        assert_eq!(to.get("retry-after").unwrap(), "12");
        assert_eq!(to.get("x-should-retry").unwrap(), "false");
        assert_eq!(to.get("request-id").unwrap(), "req_1");
        assert_eq!(to.get("anthropic-ratelimit-unified-status").unwrap(), "allowed");
        assert_eq!(to.get("anthropic-ratelimit-unified-reset").unwrap(), "1793820000");
        assert_eq!(to.get("content-type").unwrap(), "text/event-stream");
        for n in ["cf-ray", "set-cookie", "server", "transfer-encoding"] {
            assert!(to.get(n).is_none(), "{n}");
        }
    }

    async fn body_of(r: Response) -> Bytes {
        axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap()
    }

    #[tokio::test]
    async fn replay_keeps_status_body_bytes_and_headers() {
        let raw = br#"{"type":"error","error":{"type":"invalid_request_error","message":"context_management: Extra inputs are not permitted"}}"#;
        let e = UpstreamError {
            status: StatusCode::BAD_REQUEST,
            headers: upstream(&[("content-type", "application/json"), ("x-should-retry", "false"), ("cf-ray", "x")]),
            body: Bytes::from_static(raw),
        };
        let r = e.into_response();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        assert_eq!(r.headers().get("x-should-retry").unwrap(), "false");
        assert!(r.headers().get("cf-ray").is_none());
        assert_eq!(body_of(r).await.as_ref(), raw);
    }

    #[tokio::test]
    async fn stream_4xx_with_empty_body_passes_through() {
        let e = UpstreamError { status: StatusCode::TOO_MANY_REQUESTS, headers: upstream(&[("retry-after", "30")]), body: Bytes::new() };
        let r = e.into_response();
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(r.headers().get("content-type").unwrap(), "application/json");
        assert_eq!(r.headers().get("retry-after").unwrap(), "30");
        assert!(body_of(r).await.is_empty());
    }

    #[test]
    fn replayable_is_client_errors_only() {
        assert!(UpstreamError::is_replayable(StatusCode::BAD_REQUEST));
        assert!(UpstreamError::is_replayable(StatusCode::TOO_MANY_REQUESTS));
        // 上游订阅凭据失效不能冒充客户端 (本机 token) 失效: 留给 503 摘要说明是哪家订阅
        assert!(!UpstreamError::is_replayable(StatusCode::UNAUTHORIZED));
        assert!(!UpstreamError::is_replayable(StatusCode::FORBIDDEN));
        assert!(!UpstreamError::is_replayable(StatusCode::SERVICE_UNAVAILABLE));
        assert!(!UpstreamError::is_replayable(StatusCode::OK));
    }

    #[test]
    fn retry_after_clamps_and_picks_shortest() {
        assert_eq!(retry_after_secs([30_000, 90_000]), Some(30));
        assert_eq!(retry_after_secs([400]), Some(1));
        assert_eq!(retry_after_secs([600_000]), Some(60));
        assert_eq!(retry_after_secs([-5, 0]), None, "expired cooldowns do not count");
        assert_eq!(retry_after_secs(std::iter::empty()), None);
        assert_eq!(retry_after_secs([1_500]), Some(2), "rounds up");
    }
}
