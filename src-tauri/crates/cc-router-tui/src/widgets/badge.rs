//! 订阅的状态徽章: 符号 + 文案 + 颜色。总览页、订阅页共用同一套判定。

use ratatui::style::Color;

use crate::client::dto::{Subscription, SubscriptionState};
use crate::i18n::Strings;
use crate::theme::{state_symbol, Theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Badge {
    pub symbol: &'static str,
    pub label: &'static str,
    pub color: Color,
}

/// 用户看到的状态不完全等于 `SubscriptionState`:
/// 被手动停用的订阅状态仍可能是 healthy; 用户自己设的 token 限额满了也不是一种 state。
pub fn badge(sub: &Subscription, theme: &Theme, s: &Strings) -> Badge {
    let state = if !sub.enabled {
        SubscriptionState::Disabled
    } else if sub.state == SubscriptionState::Healthy && !sub.is_dispatchable && sub.quota_usage.iter().any(|q| q.exceeded) {
        return Badge {
            symbol: state_symbol(SubscriptionState::QuotaExhausted),
            label: s.st_quota_reached,
            color: theme.state_color(SubscriptionState::QuotaExhausted),
        };
    } else {
        sub.state
    };
    Badge { symbol: state_symbol(state), label: s.state(state), color: theme.state_color(state) }
}

/// 排序用: 出问题的排最前 (0), 可调度的居中 (1), 用户自己停用的排最后 (2)。
pub fn severity(sub: &Subscription) -> u8 {
    if !sub.enabled {
        2
    } else if sub.is_dispatchable {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::dto::{ModelSlots, QuotaPeriod, QuotaUsage};
    use crate::i18n::ZH;
    use crate::theme::ColorMode;

    fn sub(enabled: bool, state: SubscriptionState, is_dispatchable: bool) -> Subscription {
        Subscription {
            id: "a".into(),
            display_name: "n".into(),
            provider_display_name: "p".into(),
            enabled,
            state,
            cooldown_until: None,
            last_error_message: None,
            is_dispatchable,
            quota_usage: vec![],
            provider_id: "p".into(),
            base_url: "https://example.invalid".into(),
            auth_type: "api_key".into(),
            model_slots: ModelSlots { fable: "d".into(), opus: "a".into(), sonnet: "b".into(), haiku: "c".into(), fallback: String::new() },
            slot_efforts: Default::default(),
            referenced_by: vec![],
            balance_supported: false,
            balance_cache: None,
            model_cache: None,
        }
    }

    #[test]
    fn manually_disabled_wins_over_the_stored_state() {
        let theme = Theme::new(ColorMode::Ansi16);
        let b = badge(&sub(false, SubscriptionState::Healthy, false), &theme, &ZH);
        assert_eq!((b.symbol, b.label), ("○", ZH.st_disabled));
    }

    #[test]
    fn healthy_but_over_user_quota_is_shown_as_quota_reached() {
        let theme = Theme::new(ColorMode::Ansi16);
        let mut s = sub(true, SubscriptionState::Healthy, false);
        s.quota_usage = vec![QuotaUsage {
            period: QuotaPeriod::Daily,
            limit: Some(10),
            input: 10,
            output: 0,
            cache_creation: 0,
            cache_read: 0,
            exceeded: true,
        }];
        let b = badge(&s, &theme, &ZH);
        assert_eq!((b.symbol, b.label), ("◑", ZH.st_quota_reached));
    }

    #[test]
    fn otherwise_follows_the_state() {
        let theme = Theme::new(ColorMode::Ansi16);
        let b = badge(&sub(true, SubscriptionState::RateLimited, false), &theme, &ZH);
        assert_eq!((b.symbol, b.label, b.color), ("◐", ZH.st_rate_limited, Color::Yellow));
    }

    #[test]
    fn severity_puts_broken_first_and_disabled_last() {
        assert_eq!(severity(&sub(true, SubscriptionState::AuthFailed, false)), 0);
        assert_eq!(severity(&sub(true, SubscriptionState::Healthy, true)), 1);
        assert_eq!(severity(&sub(false, SubscriptionState::Healthy, false)), 2);
    }
}
