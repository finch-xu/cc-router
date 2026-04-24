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
pub fn classify_response(status: u16, _body: Option<&str>) -> ShouldRetry {
    match status {
        200..=299 => ShouldRetry::No,
        400 => ShouldRetry::No,
        401 | 403 => ShouldRetry::Yes(RetryReason::AuthFailed),
        429 => ShouldRetry::Yes(RetryReason::RateLimited),
        500..=599 => ShouldRetry::Yes(RetryReason::ServerError),
        _ => ShouldRetry::No,
    }
}
