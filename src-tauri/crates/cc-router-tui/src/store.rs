//! 订阅列表的共享状态: 多个页面 (总览 / 订阅 / 未来的虚拟模型) 都要读同一份数据, 不该各自存一份
//! 互相不同步的副本。加载结果按 [`crate::action::Fetch`] 共用同一条单调递增的 `issued` 序号
//! (`runtime.rs` 里的计数器横跨 `Fetch::Overview` 与 `Fetch::Subscriptions`), 晚到的旧结果据此丢弃。

use crate::client::dto::Subscription;

/// 一份带发起序号的后端数据。晚到的旧结果 (`issued` 小于已接受的序号) 与不晚于屏障的结果
/// (`issued` <= `barrier`) 都丢弃。`barrier` 是就地操作 (Task 4 的 `Mutation`) 完成那一刻盖的一条
/// 「此后这个序号以内的结果一律不算数」的线——挡住那些在变更完成前就已经发起、内容还是变更前
/// 旧值的加载, 不让它们晚到时把乐观更新过的新状态冲回去。
#[derive(Debug)]
pub struct Versioned<T> {
    value: T,
    issued: u64,
    barrier: u64,
    loaded: bool,
}

impl<T: Default> Default for Versioned<T> {
    fn default() -> Self {
        Self { value: T::default(), issued: 0, barrier: 0, loaded: false }
    }
}

impl<T> Versioned<T> {
    pub fn get(&self) -> &T {
        &self.value
    }

    /// 仅供乐观更新用, 不动序号: 调用方确定这是一次「先斩后奏」的本地纠正, 后端权威结果落地时
    /// 该按原来的 `issued` / `barrier` 规则正常覆盖它。
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.value
    }

    pub fn loaded(&self) -> bool {
        self.loaded
    }

    /// 接受则返回 `Some(旧值)`, 调用方据此做 diff; 丢弃返回 `None`。
    pub fn accept(&mut self, issued: u64, value: T) -> Option<T> {
        if self.loaded && issued < self.issued {
            return None;
        }
        if issued <= self.barrier {
            return None;
        }
        self.issued = issued;
        self.loaded = true;
        Some(std::mem::replace(&mut self.value, value))
    }

    /// 一次变更完成时调用: 此后 `issued` <= `barrier` 的结果一律丢弃。只增不减——两次变更前后脚
    /// 完成时, 后一次的屏障不该被前一次更小的值盖回去。
    pub fn set_barrier(&mut self, barrier: u64) {
        if barrier > self.barrier {
            self.barrier = barrier;
        }
    }
}

#[derive(Debug, Default)]
pub struct Store {
    /// 后端给的原始顺序; 排序由各页面在画的时候按需要的口径自己排。
    subscriptions: Versioned<Vec<Subscription>>,
}

impl Store {
    pub fn subscriptions(&self) -> &[Subscription] {
        self.subscriptions.get()
    }

    pub fn subscriptions_loaded(&self) -> bool {
        self.subscriptions.loaded()
    }

    pub fn subscription(&self, id: &str) -> Option<&Subscription> {
        self.subscriptions.get().iter().find(|s| s.id == id)
    }

    /// 乐观更新: `set_subscription_enabled` 成功后立刻把这条订阅的 `enabled` 改过来, 不等下一次
    /// `apply_subscriptions`——否则「按 e、还没等到重拉结果就又按一次 e」会读到没改过的旧值,
    /// 算出同一个目标值发给后端 (变成 no-op), 且第二次 toast 文案与第一次相同被去重规则吞掉,
    /// 用户毫无反馈。**刻意不碰序号**: 这只是本地临时纠正, 后端权威结果 (`Fetch::Subscriptions`
    /// 刷新) 落地时该按原来的序号 / 屏障规则正常覆盖它。返回 `true` 表示找到了这条订阅并改了。
    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> bool {
        match self.subscriptions.get_mut().iter_mut().find(|s| s.id == id) {
            Some(sub) => {
                sub.enabled = enabled;
                true
            }
            None => false,
        }
    }

    /// 接受一份订阅列表。晚到的旧结果 (序号小于已接受的) 或不晚于屏障的结果 (见
    /// [`Store::set_subscriptions_barrier`]) → 丢弃, 返回 `None`。否则替换并返回「状态变了的
    /// 订阅 id」(首次加载返回空 —— 首次不算变化): 以 `(state, enabled, is_dispatchable)` 三元组
    /// 比较, 只比两边都有的 id。
    pub fn apply_subscriptions(&mut self, issued: u64, subs: Vec<Subscription>) -> Option<Vec<String>> {
        let was_loaded = self.subscriptions.loaded();
        let old = self.subscriptions.accept(issued, subs)?;
        let changed = if was_loaded {
            self.subscriptions
                .get()
                .iter()
                .filter(|new| {
                    old.iter()
                        .find(|o| o.id == new.id)
                        .is_some_and(|o| (o.state, o.enabled, o.is_dispatchable) != (new.state, new.enabled, new.is_dispatchable))
                })
                .map(|s| s.id.clone())
                .collect()
        } else {
            Vec::new()
        };
        Some(changed)
    }

    /// 一次订阅相关的就地操作完成时调用: 此后 `issued` <= `barrier` 的订阅列表一律丢弃, 挡住
    /// 「晚到的、内容还是变更前旧值」的加载把乐观更新冲回去。
    pub fn set_subscriptions_barrier(&mut self, barrier: u64) {
        self.subscriptions.set_barrier(barrier);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::dto::{ModelSlots, SubscriptionState};

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

    #[test]
    fn set_enabled_optimistically_updates_without_bumping_issued() {
        let mut store = Store::default();
        store.apply_subscriptions(5, vec![sub("a", SubscriptionState::Healthy)]);

        assert!(store.set_enabled("a", false));
        assert!(!store.subscription("a").unwrap().enabled);

        // 不该动内部序号: 晚到的、序号更旧的后端结果仍然按老规则被丢弃 (不受这次乐观更新影响)。
        let changed = store.apply_subscriptions(3, vec![sub("a", SubscriptionState::RateLimited)]);
        assert_eq!(changed, None, "issued=3 仍然晚于已接受的 issued=5, 乐观更新不该让这条判断失效");
        assert!(!store.subscription("a").unwrap().enabled, "旧结果被丢弃, 乐观更新的值应该保留");

        assert!(!store.set_enabled("missing", true), "订阅不在 Store 里时应该返回 false, 不 panic");
    }

    #[test]
    fn versioned_drops_stale_and_barred_results() {
        let mut v: Versioned<i32> = Versioned::default();
        assert_eq!(v.accept(5, 1), Some(0), "首次接受, 旧值是 T::default()");
        assert_eq!(v.accept(3, 2), None, "issued=3 晚于已接受的 issued=5, 应该被丢弃");
        v.set_barrier(7);
        assert_eq!(v.accept(7, 3), None, "issued=7 不大于屏障 7, 应该被丢弃");
        assert_eq!(v.accept(8, 4), Some(1), "issued=8 大于屏障, 应该被接受, 返回值是屏障生效前的旧值");
        assert_eq!(*v.get(), 4);
    }

    #[test]
    fn barrier_never_decreases() {
        let mut v: Versioned<i32> = Versioned::default();
        v.set_barrier(10);
        v.set_barrier(3);
        assert_eq!(v.accept(10, 1), None, "屏障应该仍是 10, 不该被更小的值降回去");
        assert_eq!(v.accept(11, 1), Some(0), "大于屏障的序号应该正常被接受");
    }

    #[test]
    fn accept_returns_the_previous_value() {
        let mut v: Versioned<i32> = Versioned::default();
        v.accept(1, 10);
        assert_eq!(v.accept(2, 20), Some(10));
        assert_eq!(*v.get(), 20);
    }

    #[test]
    fn get_mut_does_not_touch_the_sequence() {
        let mut v: Versioned<i32> = Versioned::default();
        v.accept(5, 1);
        *v.get_mut() = 99;
        assert_eq!(*v.get(), 99);
        // 序号没被 get_mut 动过: 一份更旧的结果 (issued=3 < 已接受的 5) 仍然按原规则被丢弃。
        assert_eq!(v.accept(3, 2), None);
        assert_eq!(*v.get(), 99, "旧结果被丢弃, get_mut 改过的值应该保留");
    }
}
