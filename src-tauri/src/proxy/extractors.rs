//! axum 自定义 extractor. 当前仅一个: HttpVersion (从 Request::version() 拿下游 HTTP 协议版本).
//!
//! 用 extractor 而不是改 handler 接 Request<Body>, 是因为现有 handler 已经用
//! `HeaderMap + Bytes` 风格提取, 加一个 extractor 参数比换 body 提取方式侵入小得多.

use axum::async_trait;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;

/// 下游 (CC ↔ cc-router) 这一段的 HTTP 协议版本.
/// HTTPS+h2 协商成功时是 `HTTP/2.0`, 否则 `HTTP/1.1`.
pub struct HttpVersion(pub axum::http::Version);

#[async_trait]
impl<S: Send + Sync> FromRequestParts<S> for HttpVersion {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(HttpVersion(parts.version))
    }
}

/// 把 axum::http::Version 格式化成 DB 友好的字符串 ("HTTP/1.1" / "HTTP/2.0").
/// 用 Debug 而不是 Display: Display 实现在不同 hyper/http 版本里有过差异
/// (有的版本是 "HTTP/2" 有的是 "HTTP/2.0"), Debug 跨版本稳定.
pub fn format_http_version(v: axum::http::Version) -> String {
    format!("{:?}", v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_http_version_known_variants() {
        // 确保 DB 落进去的是已知字串, 改 hyper 版本时这个测试会 fail 提示重新对齐.
        assert_eq!(format_http_version(axum::http::Version::HTTP_11), "HTTP/1.1");
        assert_eq!(format_http_version(axum::http::Version::HTTP_2), "HTTP/2.0");
        assert_eq!(format_http_version(axum::http::Version::HTTP_10), "HTTP/1.0");
    }
}
