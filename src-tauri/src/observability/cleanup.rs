//! 定时清理 requests 表中超过 `log_retention_days` 的旧记录。
//!
//! 不动 `request_stats_daily` —— 那张表是按天聚合的, 永久保留, 让历史用量统计
//! 不受日志清理影响。
//!
//! settings.log_retention_days 语义:
//! - 0 = 永久保留 (跳过删除)
//! - N > 0 = 删除 timestamp < now_ms - N*86400000 的行

use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};

use crate::observability::request_log::{now_ms, DAY_MS};
use crate::settings::model::Settings;

const TICK_INTERVAL: Duration = Duration::from_secs(6 * 3600); // 6 小时

pub async fn run(pool: SqlitePool, settings: Arc<RwLock<Settings>>) {
    // 启动后立刻跑一次 (而不是等 6 小时), 让 retention 改动尽快生效
    sweep_once(&pool, &settings).await;

    let mut ticker = interval(TICK_INTERVAL);
    ticker.tick().await; // 第一 tick 立即返回, 跳过 (我们刚跑完 sweep)
    loop {
        ticker.tick().await;
        sweep_once(&pool, &settings).await;
    }
}

/// 删除 requests 表里 timestamp 老于 retention_days 的行。
/// retention_days = 0 表示「永久保留」, 直接返回 Ok(0)。
pub(crate) async fn delete_older_than(
    pool: &SqlitePool,
    retention_days: u32,
) -> Result<u64, sqlx::Error> {
    if retention_days == 0 {
        return Ok(0);
    }
    let cutoff = now_ms() - (retention_days as i64) * DAY_MS;
    let res = sqlx::query("DELETE FROM requests WHERE timestamp < ?")
        .bind(cutoff)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

async fn sweep_once(pool: &SqlitePool, settings: &Arc<RwLock<Settings>>) {
    let days = settings.read().await.log_retention_days;
    match delete_older_than(pool, days).await {
        Ok(0) if days == 0 => debug!("log_retention_days=0, skipping cleanup (永久保留)"),
        Ok(0) => debug!(retention_days = days, "cleanup ran, no rows deleted"),
        Ok(n) => info!(rows = n, retention_days = days, "cleaned up old request logs"),
        Err(e) => warn!(?e, "cleanup query failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::run_migrations;
    use crate::observability::request_log::{flush_batch, RequestLogEntry, RequestStatus};
    use crate::virtual_model::VirtualModelName;
    use sqlx::sqlite::SqlitePoolOptions;
    use sqlx::Row;
    use std::path::PathBuf;
    use uuid::Uuid;

    async fn fresh_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        run_migrations(&pool, &PathBuf::from("."))
            .await
            .unwrap();
        pool
    }

    fn entry_at(ts_ms: i64) -> RequestLogEntry {
        RequestLogEntry {
            id: Uuid::new_v4(),
            timestamp_ms: ts_ms,
            virtual_model_name: VirtualModelName::Sonnet,
            subscription_id: Uuid::new_v4(),
            provider_id: "anthropic".to_string(),
            endpoint_id: "ep".to_string(),
            real_model_name: "claude-x".to_string(),
            response_model_name: None,
            is_streaming: false,
            status: RequestStatus::Success,
            http_status: Some(200),
            ttft_ms: None,
            total_latency_ms: Some(100),
            upstream_input_tokens: Some(10),
            upstream_output_tokens: Some(20),
            upstream_cache_creation: None,
            upstream_cache_read: None,
            retry_count: 0,
            error_message: None,
            upstream_response_body: None,
            client_tool: None,
            client_user_agent: None,
            client_version: None,
            client_ip: None,
        }
    }

    async fn count(pool: &SqlitePool, table: &str) -> i64 {
        sqlx::query(&format!("SELECT COUNT(*) AS c FROM {}", table))
            .fetch_one(pool)
            .await
            .unwrap()
            .try_get("c")
            .unwrap()
    }

    #[tokio::test]
    async fn cleanup_deletes_old_requests_but_keeps_stats() {
        let pool = fresh_pool().await;
        let now = now_ms();
        // 一条很老 (100 天前), 一条新鲜
        let old = entry_at(now - 100 * DAY_MS);
        let fresh = entry_at(now);
        flush_batch(&pool, vec![old, fresh]).await.expect("flush");
        assert_eq!(count(&pool, "requests").await, 2);
        assert_eq!(count(&pool, "request_stats_daily").await, 2);

        // retention=30 天 → 老的应被删
        delete_older_than(&pool, 30).await.unwrap();

        assert_eq!(count(&pool, "requests").await, 1, "100 天前那条应被删");
        assert_eq!(
            count(&pool, "request_stats_daily").await,
            2,
            "stats 表不动, 历史聚合保留"
        );
    }

    #[tokio::test]
    async fn cleanup_zero_means_forever() {
        let pool = fresh_pool().await;
        let now = now_ms();
        flush_batch(
            &pool,
            vec![entry_at(now - 1000 * DAY_MS), entry_at(now)],
        )
        .await
        .unwrap();
        assert_eq!(count(&pool, "requests").await, 2);

        delete_older_than(&pool, 0).await.unwrap();

        assert_eq!(
            count(&pool, "requests").await,
            2,
            "retention=0 应不删任何行"
        );
    }

    #[tokio::test]
    async fn cleanup_only_deletes_past_cutoff() {
        let pool = fresh_pool().await;
        let now = now_ms();
        // 5/15/35 天前
        flush_batch(
            &pool,
            vec![
                entry_at(now - 5 * DAY_MS),
                entry_at(now - 15 * DAY_MS),
                entry_at(now - 35 * DAY_MS),
            ],
        )
        .await
        .unwrap();

        // retention=30 天 → 只有 35 天那条被删
        delete_older_than(&pool, 30).await.unwrap();
        assert_eq!(count(&pool, "requests").await, 2);

        // 改 retention=10 天 → 15 天那条也被删, 留 5 天
        delete_older_than(&pool, 10).await.unwrap();
        assert_eq!(count(&pool, "requests").await, 1);
    }
}
