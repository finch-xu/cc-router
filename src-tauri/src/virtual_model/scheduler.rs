//! 虚拟模型调度器（设计稿 §7）。
//!
//! 输入：虚拟模型配置 + 所有订阅运行时 + 当前时间 (UTC 瞬时 + 本地挂钟)
//! 输出：按调度模式排序的候选订阅列表（含不可用的过滤）

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::subscription::model::{SubscriptionRow, SubscriptionRuntime};
use crate::subscription::schedule::WallClock;
use crate::virtual_model::model::{RoutingMode, VirtualModelConfig, VirtualModelName};

pub struct ScheduleOrder {
    pub candidate_ids: Vec<Uuid>,
    /// 本次调度选中的索引（用于更新 last_used_index）。None = 不前进索引：所有订阅都不可用,
    /// 或 sticky 命中已钉住的订阅。
    pub chosen_index: Option<usize>,
}

/// Whether the subscription serves this virtual model's slot at `clock`. Fallback / jev have no
/// four-slot mapping and always pass (never use `VirtualModelName::slot` here: it maps them to
/// Sonnet).
pub(crate) fn provides_slot_at(row: &SubscriptionRow, vm: &VirtualModelName, clock: WallClock) -> bool {
    vm.four_slot()
        .map_or(true, |slot| row.slot_model_at(slot, clock).is_some())
}

/// 可调度 = 状态 / 冷却 / 限额都允许, 且此刻 (`clock`) 提供该虚拟模型的槽位。
fn schedulable(
    rt: &SubscriptionRuntime,
    vm: &VirtualModelName,
    now: DateTime<Utc>,
    clock: WallClock,
) -> bool {
    rt.is_dispatchable(now) && provides_slot_at(&rt.row, vm, clock)
}

/// 根据调度模式把 `subscription_ids` 扫描成一个候选顺序。
/// 候选已经按"健康→尝试顺序"筛过滤过。`clock` 是本次请求取的本地挂钟时刻, 调用方须在
/// 后续 model 改写里沿用同一个值。
pub async fn build_candidate_order(
    vm: &VirtualModelConfig,
    all_subs: &HashMap<Uuid, Arc<RwLock<SubscriptionRuntime>>>,
    now: DateTime<Utc>,
    clock: WallClock,
    pinned: Option<Uuid>,
) -> ScheduleOrder {
    let n = vm.subscription_ids.len();
    if n == 0 {
        return ScheduleOrder {
            candidate_ids: vec![],
            chosen_index: None,
        };
    }

    // Sticky: 钉住的订阅在本 vm 里且可调度 → 排首, 其余按轮询序兜底, 不前进索引.
    if vm.mode == RoutingMode::Sticky {
        if let Some(pin) = pinned {
            let pin_ok = match all_subs.get(&pin) {
                Some(rt) if vm.subscription_ids.contains(&pin) => {
                    schedulable(&*rt.read().await, &vm.name, now, clock)
                }
                _ => false,
            };
            if pin_ok {
                let start = (vm.last_used_index + 1) % n;
                let mut candidate_ids = vec![pin];
                for i in 0..n {
                    let sub_id = vm.subscription_ids[(start + i) % n];
                    if sub_id == pin {
                        continue;
                    }
                    let Some(rt) = all_subs.get(&sub_id) else { continue };
                    if schedulable(&*rt.read().await, &vm.name, now, clock) {
                        candidate_ids.push(sub_id);
                    }
                }
                return ScheduleOrder {
                    candidate_ids,
                    chosen_index: None,
                };
            }
        }
    }

    // 构造扫描顺序 (Sticky 未命中时按轮询)
    let scan_order: Vec<usize> = match vm.mode {
        RoutingMode::Sequential => (0..n).collect(),
        RoutingMode::RoundRobin | RoutingMode::Sticky => {
            let start = (vm.last_used_index + 1) % n;
            (0..n).map(|i| (start + i) % n).collect()
        }
    };

    let mut candidate_ids = Vec::with_capacity(n);
    let mut chosen_index: Option<usize> = None;

    for &idx in &scan_order {
        let sub_id = vm.subscription_ids[idx];
        let Some(rt) = all_subs.get(&sub_id) else { continue };
        let guard = rt.read().await;
        if schedulable(&guard, &vm.name, now, clock) {
            if chosen_index.is_none() {
                chosen_index = Some(idx);
            }
            candidate_ids.push(sub_id);
        }
    }

    ScheduleOrder {
        candidate_ids,
        chosen_index,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription::model::{SubscriptionRow, SubscriptionState};
    use crate::subscription::schedule::ScheduleRule;
    use crate::virtual_model::model::VirtualModelName;

    /// Wednesday 12:00 local.
    fn noon() -> WallClock {
        WallClock { weekday: 3, minute: 12 * 60 }
    }

    /// Wednesday 20:00 local.
    fn evening() -> WallClock {
        WallClock { weekday: 3, minute: 20 * 60 }
    }

    /// Opus default blank + an every-day 18:00-08:00 rule: provides opus only at night.
    fn night_only_opus(rt: &mut SubscriptionRuntime) {
        rt.row.model_slots.opus = String::new();
        rt.row.slot_schedules.opus = vec![ScheduleRule {
            days: vec![1, 2, 3, 4, 5, 6, 7],
            start: "18:00".into(),
            end: "08:00".into(),
            model: "night".into(),
            effort: None,
        }];
    }

    fn make_rt(enabled: bool, state: SubscriptionState) -> SubscriptionRuntime {
        let mut row = SubscriptionRow::test_fixture("p", "e");
        row.enabled = enabled;
        row.is_auth_failed = matches!(state, SubscriptionState::AuthFailed);
        let mut rt = SubscriptionRuntime::from_row(row);
        rt.state = state;
        rt
    }

    #[tokio::test]
    async fn sequential_picks_first_healthy() {
        let a = make_rt(true, SubscriptionState::RateLimited);
        let b = make_rt(true, SubscriptionState::Healthy);
        let c = make_rt(true, SubscriptionState::Healthy);
        let ids = [a.row.id, b.row.id, c.row.id];
        let mut map: HashMap<Uuid, Arc<RwLock<SubscriptionRuntime>>> = HashMap::new();
        map.insert(a.row.id, Arc::new(RwLock::new(a)));
        map.insert(b.row.id, Arc::new(RwLock::new(b)));
        map.insert(c.row.id, Arc::new(RwLock::new(c)));

        let vm = VirtualModelConfig {
            name: VirtualModelName::Sonnet,
            mode: RoutingMode::Sequential,
            subscription_ids: ids.to_vec(),
            upstream_models: Default::default(),
            last_used_index: 0,
        };
        let order = build_candidate_order(&vm, &map, Utc::now(), noon(), None).await;
        assert_eq!(order.chosen_index, Some(1));
        assert_eq!(order.candidate_ids, vec![ids[1], ids[2]]);
    }

    #[tokio::test]
    async fn round_robin_advances_past_last_used() {
        let a = make_rt(true, SubscriptionState::Healthy);
        let b = make_rt(true, SubscriptionState::Healthy);
        let c = make_rt(true, SubscriptionState::Healthy);
        let ids = [a.row.id, b.row.id, c.row.id];
        let mut map: HashMap<Uuid, Arc<RwLock<SubscriptionRuntime>>> = HashMap::new();
        map.insert(a.row.id, Arc::new(RwLock::new(a)));
        map.insert(b.row.id, Arc::new(RwLock::new(b)));
        map.insert(c.row.id, Arc::new(RwLock::new(c)));

        let vm = VirtualModelConfig {
            name: VirtualModelName::Opus,
            mode: RoutingMode::RoundRobin,
            subscription_ids: ids.to_vec(),
            upstream_models: Default::default(),
            last_used_index: 0,
        };
        let order = build_candidate_order(&vm, &map, Utc::now(), noon(), None).await;
        assert_eq!(order.chosen_index, Some(1));
        assert_eq!(order.candidate_ids[0], ids[1]);
    }

    #[tokio::test]
    async fn empty_when_all_unavailable() {
        let a = make_rt(false, SubscriptionState::Disabled);
        let b = make_rt(true, SubscriptionState::AuthFailed);
        let ids = [a.row.id, b.row.id];
        let mut map: HashMap<Uuid, Arc<RwLock<SubscriptionRuntime>>> = HashMap::new();
        map.insert(a.row.id, Arc::new(RwLock::new(a)));
        map.insert(b.row.id, Arc::new(RwLock::new(b)));

        let vm = VirtualModelConfig {
            name: VirtualModelName::Haiku,
            mode: RoutingMode::Sequential,
            subscription_ids: ids.to_vec(),
            upstream_models: Default::default(),
            last_used_index: 0,
        };
        let order = build_candidate_order(&vm, &map, Utc::now(), noon(), None).await;
        assert!(order.chosen_index.is_none());
        assert!(order.candidate_ids.is_empty());
    }

    fn three(
        map: &mut HashMap<Uuid, Arc<RwLock<SubscriptionRuntime>>>,
        states: [SubscriptionState; 3],
    ) -> [Uuid; 3] {
        let mut ids = [Uuid::nil(); 3];
        for (i, st) in states.into_iter().enumerate() {
            let rt = make_rt(!matches!(st, SubscriptionState::Disabled), st);
            ids[i] = rt.row.id;
            map.insert(rt.row.id, Arc::new(RwLock::new(rt)));
        }
        ids
    }

    #[tokio::test]
    async fn sticky_pinned_first_and_no_index_advance() {
        let mut map = HashMap::new();
        let ids = three(&mut map, [SubscriptionState::Healthy; 3]);
        let vm = VirtualModelConfig {
            name: VirtualModelName::Sonnet,
            mode: RoutingMode::Sticky,
            subscription_ids: ids.to_vec(),
            upstream_models: Default::default(),
            last_used_index: 0,
        };
        let order = build_candidate_order(&vm, &map, Utc::now(), noon(), Some(ids[2])).await;
        assert_eq!(order.candidate_ids[0], ids[2]);
        assert_eq!(order.candidate_ids.len(), 3);
        assert_eq!(order.chosen_index, None, "钉住命中不前进轮询索引");
        // 其余按轮询序 (last_used=0 → 从 1 开始): [2, 1, 0]
        assert_eq!(order.candidate_ids, vec![ids[2], ids[1], ids[0]]);
    }

    #[tokio::test]
    async fn sticky_falls_back_to_round_robin_when_pin_unusable() {
        let mut map = HashMap::new();
        let ids = three(
            &mut map,
            [
                SubscriptionState::Healthy,
                SubscriptionState::Healthy,
                SubscriptionState::RateLimited,
            ],
        );
        let vm = VirtualModelConfig {
            name: VirtualModelName::Sonnet,
            mode: RoutingMode::Sticky,
            subscription_ids: ids.to_vec(),
            upstream_models: Default::default(),
            last_used_index: 0,
        };
        // 钉住的 ids[2] 不可调度 → 轮询: start=1
        let order = build_candidate_order(&vm, &map, Utc::now(), noon(), Some(ids[2])).await;
        assert_eq!(order.chosen_index, Some(1));
        assert_eq!(order.candidate_ids, vec![ids[1], ids[0]]);
        // 未钉 → 同上
        let order = build_candidate_order(&vm, &map, Utc::now(), noon(), None).await;
        assert_eq!(order.chosen_index, Some(1));
        // 钉的 id 不属于本 vm → 同上
        let order = build_candidate_order(&vm, &map, Utc::now(), noon(), Some(Uuid::new_v4())).await;
        assert_eq!(order.chosen_index, Some(1));
    }

    #[tokio::test]
    async fn sequential_and_round_robin_ignore_pinned() {
        let mut map = HashMap::new();
        let ids = three(&mut map, [SubscriptionState::Healthy; 3]);
        let seq = VirtualModelConfig {
            name: VirtualModelName::Opus,
            mode: RoutingMode::Sequential,
            subscription_ids: ids.to_vec(),
            upstream_models: Default::default(),
            last_used_index: 0,
        };
        let o = build_candidate_order(&seq, &map, Utc::now(), noon(), Some(ids[2])).await;
        assert_eq!(o.candidate_ids[0], ids[0]);
        assert_eq!(o.chosen_index, Some(0));
        let rr = VirtualModelConfig {
            name: VirtualModelName::Opus,
            mode: RoutingMode::RoundRobin,
            subscription_ids: ids.to_vec(),
            upstream_models: Default::default(),
            last_used_index: 0,
        };
        let o = build_candidate_order(&rr, &map, Utc::now(), noon(), Some(ids[2])).await;
        assert_eq!(o.candidate_ids[0], ids[1]);
        assert_eq!(o.chosen_index, Some(1));
    }

    fn insert_all(
        rts: Vec<SubscriptionRuntime>,
    ) -> (HashMap<Uuid, Arc<RwLock<SubscriptionRuntime>>>, Vec<Uuid>) {
        let mut map = HashMap::new();
        let mut ids = Vec::new();
        for rt in rts {
            ids.push(rt.row.id);
            map.insert(rt.row.id, Arc::new(RwLock::new(rt)));
        }
        (map, ids)
    }

    #[tokio::test]
    async fn sequential_skips_subscription_without_slot_model_at_this_time() {
        let mut a = make_rt(true, SubscriptionState::Healthy);
        night_only_opus(&mut a);
        let b = make_rt(true, SubscriptionState::Healthy);
        let c = make_rt(true, SubscriptionState::Healthy);
        let (map, ids) = insert_all(vec![a, b, c]);
        let vm = VirtualModelConfig {
            name: VirtualModelName::Opus,
            mode: RoutingMode::Sequential,
            subscription_ids: ids.clone(),
            upstream_models: Default::default(),
            last_used_index: 0,
        };
        let o = build_candidate_order(&vm, &map, Utc::now(), noon(), None).await;
        assert_eq!(o.candidate_ids, vec![ids[1], ids[2]], "not in its window -> left out");
        assert_eq!(o.chosen_index, Some(1));
        let o = build_candidate_order(&vm, &map, Utc::now(), evening(), None).await;
        assert_eq!(o.candidate_ids, vec![ids[0], ids[1], ids[2]], "inside its window -> first again");
        assert_eq!(o.chosen_index, Some(0));
    }

    #[tokio::test]
    async fn round_robin_never_chooses_subscription_without_slot_model() {
        let a = make_rt(true, SubscriptionState::Healthy);
        let mut b = make_rt(true, SubscriptionState::Healthy);
        night_only_opus(&mut b);
        let c = make_rt(true, SubscriptionState::Healthy);
        let (map, ids) = insert_all(vec![a, b, c]);
        let vm = VirtualModelConfig {
            name: VirtualModelName::Opus,
            mode: RoutingMode::RoundRobin,
            subscription_ids: ids.clone(),
            upstream_models: Default::default(),
            last_used_index: 0,
        };
        // start = 1 (ids[1]), which has no opus model at noon -> skip to ids[2]
        let o = build_candidate_order(&vm, &map, Utc::now(), noon(), None).await;
        assert_eq!(o.chosen_index, Some(2));
        assert_eq!(o.candidate_ids, vec![ids[2], ids[0]]);
    }

    #[tokio::test]
    async fn sticky_pin_without_slot_model_is_not_first_candidate() {
        let mut a = make_rt(true, SubscriptionState::Healthy);
        night_only_opus(&mut a);
        let b = make_rt(true, SubscriptionState::Healthy);
        let c = make_rt(true, SubscriptionState::Healthy);
        let (map, ids) = insert_all(vec![a, b, c]);
        let vm = VirtualModelConfig {
            name: VirtualModelName::Opus,
            mode: RoutingMode::Sticky,
            subscription_ids: ids.clone(),
            upstream_models: Default::default(),
            last_used_index: 0,
        };
        let o = build_candidate_order(&vm, &map, Utc::now(), noon(), Some(ids[0])).await;
        assert_ne!(o.candidate_ids.first(), Some(&ids[0]));
        assert!(!o.candidate_ids.contains(&ids[0]));
        // falls back to round robin from start = 1 and advances the index
        assert_eq!(o.chosen_index, Some(1));
        assert_eq!(o.candidate_ids, vec![ids[1], ids[2]]);
        // inside the window the pin holds again
        let o = build_candidate_order(&vm, &map, Utc::now(), evening(), Some(ids[0])).await;
        assert_eq!(o.candidate_ids.first(), Some(&ids[0]));
        assert_eq!(o.chosen_index, None);
    }

    /// `VirtualModelName::slot()` maps Fallback / Jev to Sonnet; the time filter must not.
    #[tokio::test]
    async fn fallback_and_jev_ignore_blank_sonnet_slot() {
        for name in [VirtualModelName::Fallback, VirtualModelName::Jev] {
            let mut a = make_rt(true, SubscriptionState::Healthy);
            a.row.model_slots.sonnet = String::new();
            let (map, ids) = insert_all(vec![a]);
            let vm = VirtualModelConfig {
                name: name.clone(),
                mode: RoutingMode::Sequential,
                subscription_ids: ids.clone(),
                upstream_models: Default::default(),
                last_used_index: 0,
            };
            let o = build_candidate_order(&vm, &map, Utc::now(), noon(), None).await;
            assert_eq!(o.candidate_ids, ids, "{name:?} must not be filtered by the sonnet slot");
        }
    }
}
