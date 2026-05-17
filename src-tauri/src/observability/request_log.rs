//! 批量写入请求日志（设计稿 §9.3）。
//!
//! 生产端：请求 pipeline 发送 `RequestLogEntry`。
//! 消费端：独立 tokio 任务累积到 50 条或 5 秒 flush 一次。

use std::collections::{HashMap, VecDeque};

use chrono::Utc;
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::virtual_model::VirtualModelName;

pub(crate) const DAY_MS: i64 = 86_400_000;

#[derive(Default)]
struct StatsCounters {
    request_count: i64,
    success_count: i64,
    error_count: i64,
    timeout_count: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
    total_duration_ms_sum: i64,
    total_duration_ms_count: i64,
    ttft_ms_sum: i64,
    ttft_ms_count: i64,
    retry_count_sum: i64,
}

/// 把 Unix ms 向下取整到 UTC 当天 0 点的 ms。
/// 用 div_euclid 而非 / 是因为 i64 除法对负数会向 0 截断, 测试可能传非 1970 后的小值。
pub(crate) fn floor_to_utc_day(ts_ms: i64) -> i64 {
    ts_ms.div_euclid(DAY_MS) * DAY_MS
}

#[derive(Debug, Clone)]
pub struct RequestLogEntry {
    pub id: Uuid,
    pub timestamp_ms: i64,
    pub virtual_model_name: VirtualModelName,
    pub subscription_id: Uuid,
    pub provider_id: String,
    pub endpoint_id: String,
    pub real_model_name: String,
    /// 上游响应里的 message.model 原值(改写前)。错误/超时为 None。
    pub response_model_name: Option<String>,
    pub is_streaming: bool,
    pub status: RequestStatus,
    pub http_status: Option<u16>,
    pub ttft_ms: Option<u64>,
    pub total_latency_ms: Option<u64>,
    pub upstream_input_tokens: Option<u32>,
    pub upstream_output_tokens: Option<u32>,
    pub upstream_cache_creation: Option<u32>,
    pub upstream_cache_read: Option<u32>,
    pub retry_count: u32,
    pub error_message: Option<String>,
    /// 仅错误路径填充, 截断至 4KB
    pub upstream_response_body: Option<String>,
    /// 客户端识别 (Claude Code / Zed / Codex CLI / ...), None 表示未识别 → 前端展示 "unk"
    pub client_tool: Option<&'static str>,
    /// 客户端原始 User-Agent (识别成功时也保留, 用于详情抽屉)
    pub client_user_agent: Option<String>,
    /// 从 UA 或 stainless headers 提取的版本号
    pub client_version: Option<String>,
    /// TCP 对端 IP (来自 axum ConnectInfo, 非 X-Forwarded-For). listen_all=true 时是核心排查信息.
    pub client_ip: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum RequestStatus {
    Success,
    Error,
    Timeout,
}

impl RequestStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Error => "error",
            Self::Timeout => "timeout",
        }
    }
}

const FLUSH_SIZE: usize = 50;
const FLUSH_INTERVAL: Duration = Duration::from_secs(5);
const BUFFER_MAX: usize = 1000;

pub async fn run_consumer(
    pool: SqlitePool,
    mut rx: mpsc::Receiver<RequestLogEntry>,
    app: AppHandle,
) {
    let mut buffer: VecDeque<RequestLogEntry> = VecDeque::with_capacity(FLUSH_SIZE);
    let mut ticker = interval(FLUSH_INTERVAL);

    loop {
        tokio::select! {
            maybe_entry = rx.recv() => {
                match maybe_entry {
                    Some(entry) => {
                        if buffer.len() >= BUFFER_MAX {
                            buffer.pop_front();
                        }
                        buffer.push_back(entry);
                        if buffer.len() >= FLUSH_SIZE {
                            flush(&pool, &mut buffer, &app).await;
                        }
                    }
                    None => {
                        flush(&pool, &mut buffer, &app).await;
                        break;
                    }
                }
            }
            _ = ticker.tick() => {
                if !buffer.is_empty() {
                    flush(&pool, &mut buffer, &app).await;
                }
            }
        }
    }
}

async fn flush(pool: &SqlitePool, buffer: &mut VecDeque<RequestLogEntry>, app: &AppHandle) {
    if buffer.is_empty() {
        return;
    }
    let batch: Vec<RequestLogEntry> = buffer.drain(..).collect();
    debug!(count = batch.len(), "flushing request logs");

    match flush_batch(pool, batch).await {
        Ok(()) => {}
        Err(FlushError::BeginFailed { batch, err }) => {
            warn!(?err, "无法开启事务, 放回 buffer");
            buffer.extend(batch);
            let _ = app.emit("log_write_failed", err.to_string());
        }
        Err(FlushError::CommitFailed(err)) => {
            warn!(?err, "提交请求日志事务失败");
            let _ = app.emit("log_write_failed", err.to_string());
        }
    }
}

#[derive(Debug)]
pub(crate) enum FlushError {
    BeginFailed {
        batch: Vec<RequestLogEntry>,
        err: sqlx::Error,
    },
    CommitFailed(sqlx::Error),
}

/// 不依赖 AppHandle 的纯 DB 部分, 便于单测。语义:
/// - begin 失败 → 整批退还给 caller (BeginFailed)
/// - 单条 INSERT/UPSERT 失败 → 仅 warn (局部丢条)
/// - commit 失败 → 整批已 drain, 不退还 (CommitFailed)
pub(crate) async fn flush_batch(
    pool: &SqlitePool,
    batch: Vec<RequestLogEntry>,
) -> Result<(), FlushError> {
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(err) => return Err(FlushError::BeginFailed { batch, err }),
    };

    // key → (provider_id, counters); provider_id 取首次写入的 entry 值。
    let mut stats_acc: HashMap<(i64, String, String), (String, StatsCounters)> = HashMap::new();

    for entry in batch {
        let key = (
            floor_to_utc_day(entry.timestamp_ms),
            entry.virtual_model_name.as_str().to_string(),
            entry.subscription_id.to_string(),
        );
        let (_, acc) = stats_acc
            .entry(key)
            .or_insert_with(|| (entry.provider_id.clone(), StatsCounters::default()));
        acc.request_count += 1;
        match entry.status {
            RequestStatus::Success => acc.success_count += 1,
            RequestStatus::Error => acc.error_count += 1,
            RequestStatus::Timeout => acc.timeout_count += 1,
        }
        acc.input_tokens += entry.upstream_input_tokens.unwrap_or(0) as i64;
        acc.output_tokens += entry.upstream_output_tokens.unwrap_or(0) as i64;
        acc.cache_creation_tokens += entry.upstream_cache_creation.unwrap_or(0) as i64;
        acc.cache_read_tokens += entry.upstream_cache_read.unwrap_or(0) as i64;
        if let Some(ms) = entry.total_latency_ms {
            acc.total_duration_ms_sum += ms as i64;
            acc.total_duration_ms_count += 1;
        }
        if let Some(ms) = entry.ttft_ms {
            acc.ttft_ms_sum += ms as i64;
            acc.ttft_ms_count += 1;
        }
        acc.retry_count_sum += entry.retry_count as i64;

        let result = sqlx::query(
            "INSERT INTO requests (id, timestamp, virtual_model_name, subscription_id,
                provider_id, endpoint_id, real_model_name, response_model_name,
                is_streaming, status,
                http_status, ttft_ms, total_latency_ms,
                upstream_input_tokens, upstream_output_tokens,
                upstream_cache_creation, upstream_cache_read,
                retry_count, error_message, upstream_response_body,
                client_tool, client_user_agent, client_version, client_ip)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(entry.id.to_string())
        .bind(entry.timestamp_ms)
        .bind(entry.virtual_model_name.as_str())
        .bind(entry.subscription_id.to_string())
        .bind(entry.provider_id)
        .bind(entry.endpoint_id)
        .bind(entry.real_model_name)
        .bind(entry.response_model_name)
        .bind(entry.is_streaming as i64)
        .bind(entry.status.as_str())
        .bind(entry.http_status.map(|v| v as i64))
        .bind(entry.ttft_ms.map(|v| v as i64))
        .bind(entry.total_latency_ms.map(|v| v as i64))
        .bind(entry.upstream_input_tokens.map(|v| v as i64))
        .bind(entry.upstream_output_tokens.map(|v| v as i64))
        .bind(entry.upstream_cache_creation.map(|v| v as i64))
        .bind(entry.upstream_cache_read.map(|v| v as i64))
        .bind(entry.retry_count as i64)
        .bind(entry.error_message)
        .bind(entry.upstream_response_body)
        .bind(entry.client_tool)
        .bind(entry.client_user_agent)
        .bind(entry.client_version)
        .bind(entry.client_ip)
        .execute(&mut *tx)
        .await;
        if let Err(e) = result {
            warn!(?e, "写入单条请求日志失败");
        }
    }

    // 同事务 UPSERT 聚合结果。requests + stats 同进同退,
    // 即使 cleanup 把 requests 老数据删了, stats 仍然完整。
    for ((date_utc, vm, sub_id), (provider_id, acc)) in stats_acc {
        let result = sqlx::query(
            "INSERT INTO request_stats_daily (
                date_utc, virtual_model_name, subscription_id, provider_id,
                request_count, success_count, error_count, timeout_count,
                input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
                total_duration_ms_sum, total_duration_ms_count, ttft_ms_sum, ttft_ms_count,
                retry_count_sum
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (date_utc, virtual_model_name, subscription_id) DO UPDATE SET
                request_count = request_count + excluded.request_count,
                success_count = success_count + excluded.success_count,
                error_count = error_count + excluded.error_count,
                timeout_count = timeout_count + excluded.timeout_count,
                input_tokens = input_tokens + excluded.input_tokens,
                output_tokens = output_tokens + excluded.output_tokens,
                cache_creation_tokens = cache_creation_tokens + excluded.cache_creation_tokens,
                cache_read_tokens = cache_read_tokens + excluded.cache_read_tokens,
                total_duration_ms_sum = total_duration_ms_sum + excluded.total_duration_ms_sum,
                total_duration_ms_count = total_duration_ms_count + excluded.total_duration_ms_count,
                ttft_ms_sum = ttft_ms_sum + excluded.ttft_ms_sum,
                ttft_ms_count = ttft_ms_count + excluded.ttft_ms_count,
                retry_count_sum = retry_count_sum + excluded.retry_count_sum",
        )
        .bind(date_utc)
        .bind(vm)
        .bind(sub_id)
        .bind(provider_id)
        .bind(acc.request_count)
        .bind(acc.success_count)
        .bind(acc.error_count)
        .bind(acc.timeout_count)
        .bind(acc.input_tokens)
        .bind(acc.output_tokens)
        .bind(acc.cache_creation_tokens)
        .bind(acc.cache_read_tokens)
        .bind(acc.total_duration_ms_sum)
        .bind(acc.total_duration_ms_count)
        .bind(acc.ttft_ms_sum)
        .bind(acc.ttft_ms_count)
        .bind(acc.retry_count_sum)
        .execute(&mut *tx)
        .await;
        if let Err(e) = result {
            warn!(?e, "UPSERT 统计聚合失败");
        }
    }

    tx.commit().await.map_err(FlushError::CommitFailed)?;
    Ok(())
}

pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::run_migrations;
    use sqlx::sqlite::SqlitePoolOptions;
    use sqlx::Row;
    use std::path::PathBuf;

    async fn fresh_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        run_migrations(&pool, &PathBuf::from("."))
            .await
            .expect("migrate");
        pool
    }

    fn make_entry(
        ts_ms: i64,
        vm: VirtualModelName,
        sub_id: Uuid,
        provider: &str,
        status: RequestStatus,
        latency_ms: Option<u64>,
        in_tokens: Option<u32>,
        out_tokens: Option<u32>,
    ) -> RequestLogEntry {
        RequestLogEntry {
            id: Uuid::new_v4(),
            timestamp_ms: ts_ms,
            virtual_model_name: vm,
            subscription_id: sub_id,
            provider_id: provider.to_string(),
            endpoint_id: "ep".to_string(),
            real_model_name: "claude-x".to_string(),
            response_model_name: None,
            is_streaming: false,
            status,
            http_status: Some(200),
            ttft_ms: None,
            total_latency_ms: latency_ms,
            upstream_input_tokens: in_tokens,
            upstream_output_tokens: out_tokens,
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

    #[test]
    fn floor_to_utc_day_works() {
        // 1700000000000 ms = 2023-11-14 22:13:20 UTC
        let day = floor_to_utc_day(1_700_000_000_000);
        // 2023-11-14 00:00:00 UTC = 1699920000000
        assert_eq!(day, 1_699_920_000_000);
        // 该日整点不变
        assert_eq!(floor_to_utc_day(1_699_920_000_000), 1_699_920_000_000);
        // 该日最后一刻仍归当天
        assert_eq!(floor_to_utc_day(1_699_920_000_000 + DAY_MS - 1), 1_699_920_000_000);
        // 跨日
        assert_eq!(floor_to_utc_day(1_699_920_000_000 + DAY_MS), 1_699_920_000_000 + DAY_MS);
    }

    #[tokio::test]
    async fn flush_inserts_requests_and_upserts_stats_atomically() {
        let pool = fresh_pool().await;
        let sub = Uuid::new_v4();
        // 同一天 (2024-01-01) / sonnet / 同一订阅, 灌 5 条
        let day_start = floor_to_utc_day(1_704_067_200_000); // 2024-01-01 00:00:00 UTC
        let mut batch = Vec::new();
        for i in 0..5 {
            batch.push(make_entry(
                day_start + i * 1000,
                VirtualModelName::Sonnet,
                sub,
                "anthropic",
                if i < 4 {
                    RequestStatus::Success
                } else {
                    RequestStatus::Error
                },
                Some(100 + i as u64),
                Some(10),
                Some(20),
            ));
        }
        flush_batch(&pool, batch).await.expect("flush ok");

        // requests 表应有 5 行
        let req_count: i64 = sqlx::query("SELECT COUNT(*) AS c FROM requests")
            .fetch_one(&pool)
            .await
            .unwrap()
            .try_get("c")
            .unwrap();
        assert_eq!(req_count, 5);

        // stats 表应有 1 行, request_count=5, success=4, error=1
        let row = sqlx::query("SELECT * FROM request_stats_daily")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.try_get::<i64, _>("request_count").unwrap(), 5);
        assert_eq!(row.try_get::<i64, _>("success_count").unwrap(), 4);
        assert_eq!(row.try_get::<i64, _>("error_count").unwrap(), 1);
        assert_eq!(row.try_get::<i64, _>("input_tokens").unwrap(), 50);
        assert_eq!(row.try_get::<i64, _>("output_tokens").unwrap(), 100);
        // 5 条延迟样本: 100+101+102+103+104 = 510
        assert_eq!(row.try_get::<i64, _>("total_duration_ms_sum").unwrap(), 510);
        assert_eq!(row.try_get::<i64, _>("total_duration_ms_count").unwrap(), 5);
        assert_eq!(row.try_get::<i64, _>("date_utc").unwrap(), day_start);
        assert_eq!(
            row.try_get::<String, _>("virtual_model_name").unwrap(),
            "model-sonnet"
        );
    }

    #[tokio::test]
    async fn flush_groups_by_three_dimensions() {
        let pool = fresh_pool().await;
        let sub_a = Uuid::new_v4();
        let sub_b = Uuid::new_v4();
        let day = floor_to_utc_day(1_704_067_200_000);
        let next_day = day + DAY_MS;

        let batch = vec![
            // (day, sonnet, sub_a) ×2
            make_entry(day, VirtualModelName::Sonnet, sub_a, "anthropic",
                       RequestStatus::Success, Some(100), Some(10), Some(20)),
            make_entry(day, VirtualModelName::Sonnet, sub_a, "anthropic",
                       RequestStatus::Success, Some(200), Some(10), Some(20)),
            // (day, opus, sub_a) ×1
            make_entry(day, VirtualModelName::Opus, sub_a, "anthropic",
                       RequestStatus::Success, Some(300), Some(10), Some(20)),
            // (day, sonnet, sub_b) ×1
            make_entry(day, VirtualModelName::Sonnet, sub_b, "zhipu",
                       RequestStatus::Success, Some(400), Some(10), Some(20)),
            // (next_day, sonnet, sub_a) ×1
            make_entry(next_day, VirtualModelName::Sonnet, sub_a, "anthropic",
                       RequestStatus::Success, Some(500), Some(10), Some(20)),
        ];
        flush_batch(&pool, batch).await.expect("flush ok");

        let stats_count: i64 =
            sqlx::query("SELECT COUNT(*) AS c FROM request_stats_daily")
                .fetch_one(&pool).await.unwrap().try_get("c").unwrap();
        assert_eq!(stats_count, 4, "应有 4 个 (date,vm,sub) 唯一组合");

        // (day, sonnet, sub_a) 这一行 request_count=2
        let row = sqlx::query(
            "SELECT request_count FROM request_stats_daily
             WHERE date_utc=? AND virtual_model_name=? AND subscription_id=?",
        )
        .bind(day)
        .bind("model-sonnet")
        .bind(sub_a.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.try_get::<i64, _>("request_count").unwrap(), 2);
    }

    #[tokio::test]
    async fn flush_upserts_existing_stats_row() {
        let pool = fresh_pool().await;
        let sub = Uuid::new_v4();
        let day = floor_to_utc_day(1_704_067_200_000);

        // 第一次 flush 2 条
        flush_batch(
            &pool,
            vec![
                make_entry(day, VirtualModelName::Haiku, sub, "moonshot",
                           RequestStatus::Success, Some(50), Some(5), Some(10)),
                make_entry(day, VirtualModelName::Haiku, sub, "moonshot",
                           RequestStatus::Success, Some(50), Some(5), Some(10)),
            ],
        )
        .await
        .expect("first flush");

        // 第二次 flush 3 条 (相同 key)
        flush_batch(
            &pool,
            vec![
                make_entry(day, VirtualModelName::Haiku, sub, "moonshot",
                           RequestStatus::Error, Some(60), Some(5), Some(10)),
                make_entry(day, VirtualModelName::Haiku, sub, "moonshot",
                           RequestStatus::Success, None, Some(5), Some(10)),
                make_entry(day, VirtualModelName::Haiku, sub, "moonshot",
                           RequestStatus::Timeout, None, None, None),
            ],
        )
        .await
        .expect("second flush");

        // stats 仍只有 1 行, 累加结果
        let row = sqlx::query("SELECT * FROM request_stats_daily")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.try_get::<i64, _>("request_count").unwrap(), 5);
        assert_eq!(row.try_get::<i64, _>("success_count").unwrap(), 3);
        assert_eq!(row.try_get::<i64, _>("error_count").unwrap(), 1);
        assert_eq!(row.try_get::<i64, _>("timeout_count").unwrap(), 1);
        // 4 条有 token (10+10+10+10), 1 条 None=0
        assert_eq!(row.try_get::<i64, _>("input_tokens").unwrap(), 20);
        assert_eq!(row.try_get::<i64, _>("output_tokens").unwrap(), 40);
        // 3 条有 latency: 50+50+60=160
        assert_eq!(row.try_get::<i64, _>("total_duration_ms_sum").unwrap(), 160);
        assert_eq!(row.try_get::<i64, _>("total_duration_ms_count").unwrap(), 3);
    }
}
