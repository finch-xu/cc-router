//! 虚拟模型调度器（设计稿 §7）。
//!
//! 输入：虚拟模型配置 + 所有订阅运行时 + 当前时间
//! 输出：按调度模式排序的候选订阅列表（含不可用的过滤）

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::subscription::model::SubscriptionRuntime;
use crate::virtual_model::model::{RoutingMode, VirtualModelConfig};

pub struct ScheduleOrder {
    pub candidate_ids: Vec<Uuid>,
    /// 本次调度选中的索引（用于更新 last_used_index）。
    /// None 表示所有订阅都不可用。
    pub chosen_index: Option<usize>,
}

/// 根据调度模式把 `subscription_ids` 扫描成一个候选顺序。
/// 候选已经按"健康→尝试顺序"筛过滤过。
pub async fn build_candidate_order(
    vm: &VirtualModelConfig,
    all_subs: &HashMap<Uuid, Arc<RwLock<SubscriptionRuntime>>>,
    now: DateTime<Utc>,
) -> ScheduleOrder {
    let n = vm.subscription_ids.len();
    if n == 0 {
        return ScheduleOrder {
            candidate_ids: vec![],
            chosen_index: None,
        };
    }

    // 构造扫描顺序
    let scan_order: Vec<usize> = match vm.mode {
        RoutingMode::Sequential => (0..n).collect(),
        RoutingMode::RoundRobin => {
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
        if guard.is_dispatchable(now) {
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
    use crate::subscription::model::{ModelSlots, SubscriptionRow, SubscriptionState};
    use crate::virtual_model::model::VirtualModelName;

    fn make_rt(enabled: bool, state: SubscriptionState) -> SubscriptionRuntime {
        let row = SubscriptionRow {
            id: Uuid::new_v4(),
            provider_id: "p".into(),
            endpoint_id: "e".into(),
            display_name: "sub".into(),
            api_key: "k".into(),
            model_slots: ModelSlots {
                opus: "a".into(),
                sonnet: "b".into(),
                haiku: "c".into(),
            },
            enabled,
            is_auth_failed: matches!(state, SubscriptionState::AuthFailed),
            last_error_message: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
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
            last_used_index: 0,
        };
        let order = build_candidate_order(&vm, &map, Utc::now()).await;
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
            last_used_index: 0,
        };
        let order = build_candidate_order(&vm, &map, Utc::now()).await;
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
            last_used_index: 0,
        };
        let order = build_candidate_order(&vm, &map, Utc::now()).await;
        assert!(order.chosen_index.is_none());
        assert!(order.candidate_ids.is_empty());
    }
}
