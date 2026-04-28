//! 全局订阅巡检器:每 10 分钟扫描已加入虚拟模型且当前异常的订阅,
//! 用最小 prompt 探测一次,通过则触发 `Event::UserManualReset` 复活订阅。
//!
//! 范式参考 `observability::request_log::run_consumer`(同 `tauri::async_runtime::spawn`
//! + `tokio::time::interval` 主循环模式)。
//!
//! 设计要点:
//! - 仅扫被任意虚拟模型引用的订阅(孤儿订阅复活了也没用,跳过)
//! - 状态白名单: RateLimited / QuotaExhausted / TransientError / AuthFailed
//!   Disabled 不参与(用户主动关掉的, 意图明确)
//! - 顺序逐条 ping(不并行)对上游温和
//! - ping 失败仅记 debug 日志, 不动状态机(失败原因 cooldown 已表达)
//! - 启动后立即扫一次(`tokio::time::interval` 默认首 tick 即触发), 让持久化的
//!   AuthFailed 订阅可被立即验证

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::state::AppState;
use crate::subscription::{
    model::{SubscriptionRuntime, SubscriptionState},
    ping, state_machine,
};

const RECHECK_INTERVAL_SECS: u64 = 600; // 10 分钟

pub async fn run(state: AppState) {
    let mut ticker = tokio::time::interval(Duration::from_secs(RECHECK_INTERVAL_SECS));
    // tokio::time::interval 首 tick 立即触发, 启动时即扫一次。
    // 无异常订阅时 scan_and_recheck 内部快速返回, 零开销。
    loop {
        ticker.tick().await;
        scan_and_recheck(&state).await;
    }
}

async fn scan_and_recheck(state: &AppState) {
    // 1. 收集被任意虚拟模型引用的订阅 ID
    let referenced: HashSet<Uuid> = {
        let vms = state.virtual_models.read().await;
        vms.values()
            .flat_map(|vm| vm.subscription_ids.iter().copied())
            .collect()
    };

    if referenced.is_empty() {
        debug!("recheck_worker: 无虚拟模型引用任何订阅, 跳过");
        return;
    }

    // 2. 筛出"被引用 + 异常状态"的订阅 (内层锁立即释放)
    let candidates: Vec<Arc<RwLock<SubscriptionRuntime>>> = {
        let subs = state.subscriptions.read().await;
        let mut out = Vec::new();
        for (id, rt) in subs.iter() {
            if !referenced.contains(id) {
                continue;
            }
            let g = rt.read().await;
            if matches!(
                g.state,
                SubscriptionState::RateLimited
                    | SubscriptionState::QuotaExhausted
                    | SubscriptionState::TransientError
                    | SubscriptionState::AuthFailed
            ) {
                out.push(rt.clone());
            }
        }
        out
    };

    if candidates.is_empty() {
        debug!("recheck_worker: 无异常订阅, 跳过");
        return;
    }

    info!(count = candidates.len(), "recheck_worker: 开始扫描异常订阅");

    // 3. 顺序 ping (对上游温和, 也避免内存压力)
    for rt in candidates {
        // clone 出 row 全字段(含 snapshot 连接信息), 避免 ping 全程持锁
        let row = {
            let g = rt.read().await;
            g.row.clone()
        };
        let sub_id = row.id;
        let display_name = row.display_name.clone();

        let Some(model) = ping::pick_test_model(&row) else {
            debug!(%sub_id, "recheck_worker: 无可用 model, 跳过");
            continue;
        };

        let result = ping::probe(&state.probe_client, &row, &model).await;
        if result.ok {
            match state_machine::apply(
                &state.db,
                &state.app_handle,
                &state.event_log_tx,
                rt.clone(),
                state_machine::Event::UserManualReset,
            )
            .await
            {
                Ok(_) => info!(
                    %sub_id,
                    %display_name,
                    %model,
                    http = ?result.http_status,
                    "recheck_worker 复活订阅"
                ),
                Err(e) => warn!(?e, %sub_id, "recheck_worker UserManualReset 失败"),
            }
        } else {
            debug!(
                %sub_id,
                %display_name,
                http = ?result.http_status,
                msg = %result.message,
                "recheck_worker: 仍未恢复"
            );
        }
    }
}
