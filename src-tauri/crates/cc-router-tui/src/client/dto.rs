//! 视图结构体: 只声明 TUI 实际用到的字段, 其余忽略 (serde 默认行为)。
//! 与后端 DTO 的契约由 golden fixture 锁住: 主 crate 的测试把样例 DTO 序列化进
//! `tests/fixtures/*.json`, 本文件的测试把它们反序列化回来。后端改字段名 → 主 crate 那边先失败。

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ProxyStatus {
    pub running: bool,
    pub mode: String,
    pub http_port: Option<u16>,
    pub https_port: Option<u16>,
    pub listen_all: bool,
    pub base_url: String,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionState {
    Healthy,
    RateLimited,
    QuotaExhausted,
    TransientError,
    AuthFailed,
    Disabled,
    /// 后端将来加了新状态时, 旧 TUI 不应该整个列表都解析失败。
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Subscription {
    pub id: String,
    pub display_name: String,
    pub provider_display_name: String,
    pub enabled: bool,
    pub state: SubscriptionState,
    /// Unix 毫秒
    pub cooldown_until: Option<i64>,
    pub last_error_message: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Settings {
    pub preferred_language: String,
    pub tui_enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_state_does_not_break_the_list() {
        let subs: Vec<Subscription> = serde_json::from_str(
            r#"[{"id":"a","display_name":"n","provider_display_name":"p","enabled":true,
                 "state":"some_future_state","cooldown_until":null,"last_error_message":null,"extra":1}]"#,
        )
        .unwrap();
        assert_eq!(subs[0].state, SubscriptionState::Unknown);
    }
}
