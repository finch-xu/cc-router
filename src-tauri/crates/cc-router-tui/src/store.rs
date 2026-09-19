//! 订阅列表的共享状态: 多个页面 (总览 / 订阅 / 未来的虚拟模型) 都要读同一份数据, 不该各自存一份
//! 互相不同步的副本。加载结果按 [`crate::action::Fetch`] 共用同一条单调递增的 `issued` 序号
//! (`runtime.rs` 里的计数器横跨 `Fetch::Overview` 与 `Fetch::Subscriptions`), 晚到的旧结果据此丢弃。

use crate::client::dto::Subscription;

#[derive(Debug, Default)]
pub struct Store {
    /// 后端给的原始顺序; 排序由各页面在画的时候按需要的口径自己排。
    subscriptions: Vec<Subscription>,
    /// 已接受的最新一次加载的序号。
    subs_issued: u64,
    subs_loaded: bool,
}

impl Store {
    pub fn subscriptions(&self) -> &[Subscription] {
        &self.subscriptions
    }

    pub fn subscriptions_loaded(&self) -> bool {
        self.subs_loaded
    }

    pub fn subscription(&self, id: &str) -> Option<&Subscription> {
        self.subscriptions.iter().find(|s| s.id == id)
    }

    /// 接受一份订阅列表。`issued` 小于已接受的序号 → 丢弃, 返回 `None` (晚到的旧结果)。
    /// 否则替换并返回「状态变了的订阅 id」(首次加载返回空 —— 首次不算变化): 以
    /// `(state, enabled, is_dispatchable)` 三元组比较, 只比两边都有的 id。
    pub fn apply_subscriptions(&mut self, issued: u64, subs: Vec<Subscription>) -> Option<Vec<String>> {
        if self.subs_loaded && issued < self.subs_issued {
            return None;
        }
        let changed = if self.subs_loaded {
            subs.iter()
                .filter(|new| {
                    self.subscriptions.iter().find(|old| old.id == new.id).is_some_and(|old| {
                        (old.state, old.enabled, old.is_dispatchable) != (new.state, new.enabled, new.is_dispatchable)
                    })
                })
                .map(|s| s.id.clone())
                .collect()
        } else {
            Vec::new()
        };
        self.subscriptions = subs;
        self.subs_issued = issued;
        self.subs_loaded = true;
        Some(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::dto::SubscriptionState;

    fn sub(id: &str, state: SubscriptionState) -> Subscription {
        Subscription {
            id: id.into(),
            display_name: id.into(),
            provider_display_name: "p".into(),
            enabled: true,
            state,
            cooldown_until: None,
            last_error_message: None,
            is_dispatchable: state == SubscriptionState::Healthy,
            quota_usage: vec![],
        }
    }

    #[test]
    fn first_load_reports_no_changes() {
        let mut store = Store::default();
        let changed = store.apply_subscriptions(1, vec![sub("a", SubscriptionState::Healthy)]);
        assert_eq!(changed, Some(vec![]));
        assert!(store.subscriptions_loaded());
        assert_eq!(store.subscriptions().len(), 1);
    }

    #[test]
    fn changed_ids_are_reported_by_state_enabled_or_dispatchable() {
        let mut store = Store::default();
        store.apply_subscriptions(1, vec![sub("a", SubscriptionState::Healthy), sub("b", SubscriptionState::Healthy)]);

        // b 的状态变了 (连带 is_dispatchable), a 不变。
        let mut b = sub("b", SubscriptionState::RateLimited);
        b.is_dispatchable = false;
        let changed = store.apply_subscriptions(2, vec![sub("a", SubscriptionState::Healthy), b]);
        assert_eq!(changed, Some(vec!["b".to_string()]));

        // enabled 变了也算 (state/is_dispatchable 都不变)。
        let mut a_disabled = sub("a", SubscriptionState::Healthy);
        a_disabled.enabled = false;
        let mut b2 = sub("b", SubscriptionState::RateLimited);
        b2.is_dispatchable = false;
        let changed2 = store.apply_subscriptions(3, vec![a_disabled, b2]);
        assert_eq!(changed2, Some(vec!["a".to_string()]));

        // is_dispatchable 单独变, state 仍是 Healthy —— 这正是 token 限额打满的场景:
        // SubscriptionState 本身不变 (后端不为"用户自己设的配额满了"单独发一个 state), 只有
        // is_dispatchable 从 true 掉到 false。fix round 1 (M2) 前的用例只覆盖了「state 和
        // is_dispatchable 一起变」, 漏了这条最容易在生产里出现的组合。
        let mut store2 = Store::default();
        store2.apply_subscriptions(1, vec![sub("c", SubscriptionState::Healthy)]);
        let mut c_quota_full = sub("c", SubscriptionState::Healthy);
        c_quota_full.is_dispatchable = false;
        let changed3 = store2.apply_subscriptions(2, vec![c_quota_full]);
        assert_eq!(changed3, Some(vec!["c".to_string()]), "is_dispatchable 单独变也要算变化");

        // state 单独变, is_dispatchable 不变 (两个都不可调度的状态之间切换, 比如限流 → 临时错误)。
        let mut store3 = Store::default();
        let mut d1 = sub("d", SubscriptionState::RateLimited);
        d1.is_dispatchable = false;
        store3.apply_subscriptions(1, vec![d1]);
        let mut d2 = sub("d", SubscriptionState::TransientError);
        d2.is_dispatchable = false;
        let changed4 = store3.apply_subscriptions(2, vec![d2]);
        assert_eq!(changed4, Some(vec!["d".to_string()]), "state 单独变也要算变化");
    }

    #[test]
    fn a_stale_result_is_dropped() {
        let mut store = Store::default();
        store.apply_subscriptions(5, vec![sub("a", SubscriptionState::Healthy)]);
        let changed = store.apply_subscriptions(3, vec![sub("a", SubscriptionState::RateLimited)]);
        assert_eq!(changed, None, "issued=3 晚于已接受的 issued=5, 应该被丢弃");
        assert_eq!(store.subscriptions()[0].state, SubscriptionState::Healthy, "内容不该被旧结果覆盖");
    }

    #[test]
    fn equal_issued_is_accepted() {
        let mut store = Store::default();
        store.apply_subscriptions(4, vec![sub("a", SubscriptionState::Healthy)]);
        let changed = store.apply_subscriptions(4, vec![sub("a", SubscriptionState::RateLimited)]);
        assert!(changed.is_some(), "同序号视为同一次, 应该接受");
        assert_eq!(store.subscriptions()[0].state, SubscriptionState::RateLimited);
    }

    #[test]
    fn ids_only_on_one_side_are_not_changes() {
        let mut store = Store::default();
        store.apply_subscriptions(1, vec![sub("a", SubscriptionState::Healthy), sub("b", SubscriptionState::Healthy)]);
        // b 被移除, c 是新出现的 —— 都不算「状态变了」。
        let changed = store.apply_subscriptions(2, vec![sub("a", SubscriptionState::Healthy), sub("c", SubscriptionState::Healthy)]);
        assert_eq!(changed, Some(vec![]));
    }
}
