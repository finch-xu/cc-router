//! 定时清理 `requests` 与 `events` 表中超过 `log_retention_days` 的旧记录。
//!
//! 不动 `request_stats_daily` / `receipt_stats_daily` —— 两张按天聚合表永久保留,
//! 让历史用量统计与小票不受日志清理影响。
//!
//! settings.log_retention_days 语义:
//! - 0 = 永久保留 (跳过删除), **默认值** (settings/model.rs::default_retention)
//! - N > 0 = 删除 timestamp < now_ms - N*86400000 的行
//!
//! 历史遗留: 旧版前端「永久」选项发的是 36500 (100 年), 功能等价;
//! 前端显示时把 >= 36500 归一化为「永久」。

use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::sync::{mpsc, RwLock};
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};

use crate::observability::events::{self, EventEntry};
use crate::observability::request_log::{now_ms, DAY_MS};
use crate::settings::model::Settings;

const TICK_INTERVAL: Duration = Duration::from_secs(6 * 3600); // 6 小时

pub(crate) const SIZE_GUARD_BATCH: i64 = 1000;
pub(crate) const SIZE_GUARD_MAX_ROUNDS: usize = 200;
pub(crate) const SIZE_GUARD_TARGET_RATIO: f64 = 0.9;
const VACUUM_DELETED_ROWS_THRESHOLD: u64 = 10_000;
const VACUUM_FREELIST_RATIO_THRESHOLD: f64 = 0.25;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeletedRows {
    pub requests: u64,
    pub events: u64,
}

impl DeletedRows {
    pub fn total(self) -> u64 {
        self.requests + self.events
    }
}

pub async fn run(pool: SqlitePool, settings: Arc<RwLock<Settings>>, event_tx: mpsc::Sender<EventEntry>) {
    // 启动后立刻跑一次, 让 retention 改动尽快生效; 只有这一次会考虑 VACUUM
    let deleted = sweep_once(&pool, &settings, &event_tx).await;
    maybe_vacuum(&pool, deleted).await;

    let mut ticker = interval(TICK_INTERVAL);
    ticker.tick().await; // 第一 tick 立即返回, 跳过 (我们刚跑完 sweep)
    loop {
        ticker.tick().await;
        sweep_once(&pool, &settings, &event_tx).await;
    }
}

async fn pragma_u64(pool: &SqlitePool, name: &str) -> Result<u64, sqlx::Error> {
    // name 是本模块内的字面量白名单 (page_count / freelist_count / page_size), 不来自用户
    let v: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("PRAGMA {name}")))
        .fetch_one(pool)
        .await?;
    Ok(v.max(0) as u64)
}

/// 实际占用字节 = (page_count - freelist_count) * page_size。
/// 不用文件大小: DELETE 后页只进 freelist, 文件不缩, 按文件大小判会一直删到空。
pub(crate) async fn occupied_bytes(pool: &SqlitePool) -> Result<u64, sqlx::Error> {
    let page_count = pragma_u64(pool, "page_count").await?;
    let freelist = pragma_u64(pool, "freelist_count").await?;
    let page_size = pragma_u64(pool, "page_size").await?;
    Ok(page_count.saturating_sub(freelist) * page_size)
}

async fn delete_oldest_batch(pool: &SqlitePool, table: &str) -> Result<u64, sqlx::Error> {
    // table 只会是 "requests" / "events" 字面量
    let sql = format!(
        "DELETE FROM {table} WHERE id IN (SELECT id FROM {table} ORDER BY timestamp ASC LIMIT ?)"
    );
    Ok(sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(SIZE_GUARD_BATCH)
        .execute(pool)
        .await?
        .rows_affected())
}

/// 体积安全网: 占用超过 `limit_mb` 时, 从最老开始按批删 requests / events, 直到 ≤ 90% 上限。
/// 只删两张明细表; 两表都删 0 行 (只剩聚合表) 或达轮数上限即停。
pub(crate) async fn enforce_size_limit(
    pool: &SqlitePool,
    limit_mb: u32,
) -> Result<DeletedRows, sqlx::Error> {
    let limit_bytes = limit_mb as u64 * 1024 * 1024;
    let mut deleted = DeletedRows::default();
    if occupied_bytes(pool).await? <= limit_bytes {
        return Ok(deleted);
    }
    let target = (limit_bytes as f64 * SIZE_GUARD_TARGET_RATIO) as u64;
    for _ in 0..SIZE_GUARD_MAX_ROUNDS {
        let r = delete_oldest_batch(pool, "requests").await?;
        let e = delete_oldest_batch(pool, "events").await?;
        deleted.requests += r;
        deleted.events += e;
        if r == 0 && e == 0 {
            break;
        }
        if occupied_bytes(pool).await? <= target {
            break;
        }
    }
    Ok(deleted)
}

/// 启动 sweep 后调用: 删除量达阈值或 freelist 占比过高时 VACUUM 一次。返回是否执行了。
/// 失败只 warn (可能有事务在跑), 下次启动再试。
pub(crate) async fn maybe_vacuum(pool: &SqlitePool, deleted_total: u64) -> bool {
    let should = if deleted_total >= VACUUM_DELETED_ROWS_THRESHOLD {
        true
    } else {
        let page_count = pragma_u64(pool, "page_count").await.unwrap_or(0);
        let freelist = pragma_u64(pool, "freelist_count").await.unwrap_or(0);
        page_count > 0 && (freelist as f64 / page_count as f64) >= VACUUM_FREELIST_RATIO_THRESHOLD
    };
    if !should {
        return false;
    }
    match sqlx::query("VACUUM").execute(pool).await {
        Ok(_) => {
            info!(deleted_total, "VACUUM completed");
            true
        }
        Err(e) => {
            warn!(?e, "VACUUM failed, will retry on next startup");
            false
        }
    }
}

/// 删除 `requests` 与 `events` 表里 timestamp 老于 retention_days 的行 (同一保留期)。
/// retention_days = 0 表示「永久保留」, 直接返回全 0。
pub(crate) async fn delete_older_than(
    pool: &SqlitePool,
    retention_days: u32,
) -> Result<DeletedRows, sqlx::Error> {
    if retention_days == 0 {
        return Ok(DeletedRows::default());
    }
    let cutoff = now_ms() - (retention_days as i64) * DAY_MS;
    let requests = sqlx::query("DELETE FROM requests WHERE timestamp < ?")
        .bind(cutoff)
        .execute(pool)
        .await?
        .rows_affected();
    let events = sqlx::query("DELETE FROM events WHERE timestamp < ?")
        .bind(cutoff)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(DeletedRows { requests, events })
}

async fn sweep_once(
    pool: &SqlitePool,
    settings: &Arc<RwLock<Settings>>,
    event_tx: &mpsc::Sender<EventEntry>,
) -> u64 {
    let (days, limit_mb) = {
        let s = settings.read().await;
        (s.log_retention_days, s.db_size_limit_mb)
    };
    let mut total = 0u64;
    match delete_older_than(pool, days).await {
        Ok(d) if d.total() == 0 && days == 0 => debug!("log_retention_days=0, skipping cleanup (永久保留)"),
        Ok(d) if d.total() == 0 => debug!(retention_days = days, "cleanup ran, no rows deleted"),
        Ok(d) => {
            total += d.total();
            info!(requests = d.requests, events = d.events, retention_days = days, "cleaned up old request logs / events");
        }
        Err(e) => warn!(?e, "cleanup query failed"),
    }
    match enforce_size_limit(pool, limit_mb).await {
        Ok(d) if d.total() == 0 => {}
        Ok(d) => {
            total += d.total();
            warn!(requests = d.requests, events = d.events, limit_mb, "db size limit reached, trimmed oldest rows");
            events::record_system_warn(
                event_tx,
                format!(
                    "数据库超过 {limit_mb} MB 上限, 已按时间从旧到新清理 {} 条请求日志与 {} 条事件",
                    d.requests, d.events
                ),
                Some(serde_json::json!({
                    "kind": "db_size_guard",
                    "limit_mb": limit_mb,
                    "deleted_requests": d.requests,
                    "deleted_events": d.events,
                })),
            );
        }
        Err(e) => warn!(?e, "size guard failed"),
    }
    total
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
            entry_kind: None,
            downstream_http_version: None,
            client_effort: None,
            effective_effort: None,
            effort_source: None,
            upstream_effort: None,
        }
    }

    async fn count(pool: &SqlitePool, table: &str) -> i64 {
        sqlx::query(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) AS c FROM {}", table)))
            .fetch_one(pool)
            .await
            .unwrap()
            .try_get("c")
            .unwrap()
    }

    async fn insert_event(pool: &SqlitePool, ts_ms: i64) {
        sqlx::query("INSERT INTO events (id, timestamp, kind, severity, summary) VALUES (?, ?, 'system_error', 'error', 'x')")
            .bind(Uuid::new_v4().to_string()).bind(ts_ms).execute(pool).await.unwrap();
    }

    #[tokio::test]
    async fn cleanup_deletes_old_requests_but_keeps_stats() {
        let pool = fresh_pool().await;
        let now = now_ms();
        // 一条很老 (100 天前), 一条新鲜
        let old = entry_at(now - 100 * DAY_MS);
        let fresh = entry_at(now);
        flush_batch(&pool, vec![old, fresh], vec![]).await.expect("flush");
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
            vec![],
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
            vec![],
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

    #[tokio::test]
    async fn cleanup_deletes_old_events_with_same_retention() {
        let pool = fresh_pool().await;
        let now = now_ms();
        insert_event(&pool, now - 100 * DAY_MS).await;
        insert_event(&pool, now).await;
        flush_batch(&pool, vec![entry_at(now - 100 * DAY_MS), entry_at(now)], vec![]).await.unwrap();

        let deleted = delete_older_than(&pool, 30).await.unwrap();
        assert_eq!(deleted.requests, 1);
        assert_eq!(deleted.events, 1);
        assert_eq!(deleted.total(), 2);
        assert_eq!(count(&pool, "events").await, 1);
        assert_eq!(count(&pool, "requests").await, 1);
    }

    /// 塞 2500 行带 4KB body 的记录 (约 10 MB), 上限设 4 MB → 应按 1000 一批从最老删,
    /// 降到 ≤ 3.6 MB 即停 (预期删 2 批、剩约 500 行), 且不动聚合表。
    #[tokio::test]
    async fn size_guard_deletes_oldest_rows_until_under_target() {
        let pool = fresh_pool().await;
        let now = now_ms();
        let mut batch = Vec::new();
        for i in 0..2500 {
            // i 越大越老
            let mut e = entry_at(now - i as i64 * 1000);
            e.upstream_response_body = Some("z".repeat(4000));
            batch.push(e);
        }
        flush_batch(&pool, batch, vec![]).await.unwrap();
        let before = occupied_bytes(&pool).await.unwrap();
        assert!(before > 4 * 1024 * 1024, "前置: 应超过 4 MB, 实际 {before}");
        let stats_before = count(&pool, "request_stats_daily").await;

        let deleted = enforce_size_limit(&pool, 4).await.unwrap();
        assert!(deleted.requests > 0);
        assert_eq!(deleted.requests % SIZE_GUARD_BATCH as u64, 0, "整批删除");
        let after = occupied_bytes(&pool).await.unwrap();
        assert!(after <= (4.0 * 1024.0 * 1024.0 * SIZE_GUARD_TARGET_RATIO) as u64, "after={after}");
        assert_eq!(count(&pool, "request_stats_daily").await, stats_before, "聚合表不动");

        // 没删光, 且被删的是最老的: 剩下行的 min(timestamp) 落在最新的那一段
        let remaining = count(&pool, "requests").await;
        assert!(remaining > 0 && remaining < 2500, "remaining={remaining}");
        let min_ts: i64 = sqlx::query_scalar("SELECT MIN(timestamp) FROM requests").fetch_one(&pool).await.unwrap();
        assert_eq!(min_ts, now - (remaining - 1) * 1000, "剩下的应是 i 最小 (最新) 的那 remaining 行");
    }

    #[tokio::test]
    async fn size_guard_noop_when_under_limit() {
        let pool = fresh_pool().await;
        flush_batch(&pool, vec![entry_at(now_ms())], vec![]).await.unwrap();
        let deleted = enforce_size_limit(&pool, 500).await.unwrap();
        assert_eq!(deleted, DeletedRows::default());
        assert_eq!(count(&pool, "requests").await, 1);
    }

    #[tokio::test]
    async fn size_guard_stops_when_only_aggregates_remain() {
        // 只有聚合行 (requests 被全删), 上限设成 0 MB 也不能死循环
        let pool = fresh_pool().await;
        flush_batch(&pool, vec![entry_at(now_ms())], vec![]).await.unwrap();
        sqlx::query("DELETE FROM requests").execute(&pool).await.unwrap();
        let deleted = enforce_size_limit(&pool, 0).await.unwrap();
        assert_eq!(deleted, DeletedRows::default());
        assert_eq!(count(&pool, "request_stats_daily").await, 1);
    }

    #[tokio::test]
    async fn maybe_vacuum_runs_only_when_threshold_hit() {
        let pool = fresh_pool().await;
        assert!(!maybe_vacuum(&pool, 0).await, "无删除且无 freelist → 不 VACUUM");
        assert!(maybe_vacuum(&pool, 10_000).await, "删除量达阈值 → VACUUM");
    }
}
