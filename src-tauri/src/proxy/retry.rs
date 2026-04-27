//! 上游错误分类（设计稿 §5.2）：决定是否切订阅重试。

use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub enum ShouldRetry {
    Yes(RetryReason),
    No,
}

#[derive(Debug, Clone, Copy)]
pub enum RetryReason {
    AuthFailed,
    RateLimited,
    ServerError,
    Network,
    StreamAbortedBeforeAnyEvent,
}

impl RetryReason {
    /// 触发事件时默认等待（目前没有实际退避，仅记录语义）。
    pub fn _suggested_backoff(self) -> Duration {
        match self {
            Self::AuthFailed => Duration::ZERO,
            Self::RateLimited => Duration::ZERO,
            Self::ServerError => Duration::ZERO,
            Self::Network => Duration::ZERO,
            Self::StreamAbortedBeforeAnyEvent => Duration::ZERO,
        }
    }
}

/// 按 HTTP 状态决定是否应该切订阅重试。
///
/// 调度策略 (2026-04 修订):
/// - 2xx → 不重试,返回客户端
/// - 401/403 → 切下家,触发 AuthFailed 状态
/// - 429 → 切下家,触发 RateLimited 状态 + 60s 冷却
/// - 5xx → 切下家,累积 3 次进 TransientError + 30s 起递增冷却
/// - **其他 4xx (含 400/402/404/422 等) → 切下家但不冷却**
///   原因: provider 之间能力差异 (如 thinking 块支持) 会让同一份请求在 A 上 200 在 B 上 400。
///   保留 "不冷却" 语义避免客户端 bug 把所有订阅冷却 (state_machine::classify_http 中处理)。
/// - 其他状态码 → 不重试 (兜底)
pub fn classify_response(status: u16, _body: Option<&str>) -> ShouldRetry {
    match status {
        200..=299 => ShouldRetry::No,
        401 | 403 => ShouldRetry::Yes(RetryReason::AuthFailed),
        429 => ShouldRetry::Yes(RetryReason::RateLimited),
        500..=599 => ShouldRetry::Yes(RetryReason::ServerError),
        400..=499 => ShouldRetry::Yes(RetryReason::ServerError),
        _ => ShouldRetry::No,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_yes(s: ShouldRetry) -> bool {
        matches!(s, ShouldRetry::Yes(_))
    }

    #[test]
    fn success_does_not_retry() {
        assert!(matches!(classify_response(200, None), ShouldRetry::No));
        assert!(matches!(classify_response(204, None), ShouldRetry::No));
    }

    #[test]
    fn auth_failed_retries_with_auth_reason() {
        assert!(matches!(
            classify_response(401, None),
            ShouldRetry::Yes(RetryReason::AuthFailed)
        ));
        assert!(matches!(
            classify_response(403, None),
            ShouldRetry::Yes(RetryReason::AuthFailed)
        ));
    }

    #[test]
    fn rate_limited_retries_with_rate_limit_reason() {
        assert!(matches!(
            classify_response(429, None),
            ShouldRetry::Yes(RetryReason::RateLimited)
        ));
    }

    #[test]
    fn server_error_retries() {
        assert!(matches!(
            classify_response(500, None),
            ShouldRetry::Yes(RetryReason::ServerError)
        ));
        assert!(matches!(
            classify_response(503, None),
            ShouldRetry::Yes(RetryReason::ServerError)
        ));
    }

    #[test]
    fn other_4xx_now_retries() {
        // 修复前: 400/402/404/422 全都 No, 任务暂停
        // 修复后: 都跳下家, state_machine 不冷却
        assert!(is_yes(classify_response(400, None)));
        assert!(is_yes(classify_response(402, None)));
        assert!(is_yes(classify_response(404, None)));
        assert!(is_yes(classify_response(422, None)));
    }
}
