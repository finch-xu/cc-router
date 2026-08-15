//! 批量写入请求日志（设计稿 §9.3）。
//!
//! 生产端：请求 pipeline 发送 `RequestLogEntry`。
//! 消费端：独立 tokio 任务累积到 50 条或 5 秒 flush 一次。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, RwLock};
use tokio::time::{interval, Duration};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::observability::events::{EventEntry, EventKind, Severity};
use crate::subscription::model::SubscriptionRuntime;
use crate::subscription::quota::QuotaPeriod;
use crate::subscription::store::{save_quota_usage_rows, usage_to_rows, QuotaUsageRow};
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

/// 小票专用聚合计数 (receipt_stats_daily), 只有小票需要的 5 列。
/// 与 StatsCounters 分开是因为聚合 key 多一维 real_model_name。
#[derive(Default)]
struct ReceiptCounters {
    request_count: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
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
    /// 请求入口: "messages" (POST /v1/messages) 或 "responses" (POST /v1/responses).
    /// 用 &'static str 不是 String 因为枚举的 as_str() 返回 'static slice, 跟 client_tool 同模式.
    pub entry_kind: Option<&'static str>,
    /// 下游 (CC ↔ cc-router) 协商的 HTTP 协议版本, 形如 "HTTP/1.1" / "HTTP/2.0".
    pub downstream_http_version: Option<String>,
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

/// Add one finished request's usage to the subscription's in-memory quota buckets.
/// Returns the first period that crossed its limit *because of this entry* (for one-shot alerting).
pub fn apply_entry_to_quota(
    rt: &mut SubscriptionRuntime,
    entry: &RequestLogEntry,
    now: DateTime<Utc>,
) -> Option<QuotaPeriod> {
    let input = entry.upstream_input_tokens.unwrap_or(0) as u64;
    let output = entry.upstream_output_tokens.unwrap_or(0) as u64;
    let cc = entry.upstream_cache_creation.unwrap_or(0) as u64;
    let cr = entry.upstream_cache_read.unwrap_or(0) as u64;
    if input + output + cc + cr == 0 {
        return None;
    }
    let before = rt.row.token_quotas.first_exceeded(&rt.quota_usage, now);
    rt.quota_usage.add(now, input, output, cc, cr);
    let after = rt.row.token_quotas.first_exceeded(&rt.quota_usage, now);
    match (before, after) {
        (None, Some(p)) => Some(p),
        _ => None,
    }
}

pub async fn run_consumer(
    pool: SqlitePool,
    mut rx: mpsc::Receiver<RequestLogEntry>,
    app: AppHandle,
    subscriptions: Arc<RwLock<HashMap<Uuid, Arc<RwLock<SubscriptionRuntime>>>>>,
    event_tx: mpsc::Sender<EventEntry>,
) {
    let mut buffer: VecDeque<RequestLogEntry> = VecDeque::with_capacity(FLUSH_SIZE);
    let mut ticker = interval(FLUSH_INTERVAL);
    let mut dirty: HashSet<Uuid> = HashSet::new();

    loop {
        tokio::select! {
            maybe_entry = rx.recv() => {
                match maybe_entry {
                    Some(entry) => {
                        let rt = subscriptions.read().await.get(&entry.subscription_id).cloned();
                        if let Some(rt) = rt {
                            let now = Utc::now();
                            let crossed = {
                                let mut g = rt.write().await;
                                apply_entry_to_quota(&mut g, &entry, now)
                            };
                            dirty.insert(entry.subscription_id);
                            if let Some(period) = crossed {
                                let display_name = rt.read().await.row.display_name.clone();
                                let ev = EventEntry {
                                    id: Uuid::new_v4(),
                                    timestamp_ms: now.timestamp_millis(),
                                    kind: EventKind::QuotaReached,
                                    severity: Severity::Warn,
                                    subscription_id: Some(entry.subscription_id),
                                    request_id: Some(entry.id),
                                    summary: format!("{display_name} 已达 {} token 限额, 暂停调度至下一周期", period.label_zh()),
                                    payload: Some(serde_json::json!({ "period": period.as_str() })),
                                };
                                if let Err(e) = event_tx.try_send(ev) {
                                    warn!(?e, subscription_id = %entry.subscription_id, "quota_reached 事件投递失败");
                                }
                                if let Err(e) = app.emit(
                                    "subscription_quota_reached",
                                    serde_json::json!({ "subscription_id": entry.subscription_id.to_string(), "period": period.as_str() }),
                                ) {
                                    warn!(?e, subscription_id = %entry.subscription_id, "quota_reached 事件投递失败");
                                }
                            }
                        }
                        if buffer.len() >= BUFFER_MAX {
                            buffer.pop_front();
                        }
                        buffer.push_back(entry);
                        if buffer.len() >= FLUSH_SIZE {
                            flush(&pool, &mut buffer, &app, &subscriptions, &mut dirty).await;
                        }
                    }
                    None => {
                        flush(&pool, &mut buffer, &app, &subscriptions, &mut dirty).await;
                        break;
                    }
                }
            }
            _ = ticker.tick() => {
                if !buffer.is_empty() {
                    flush(&pool, &mut buffer, &app, &subscriptions, &mut dirty).await;
                }
            }
        }
    }
}

async fn flush(
    pool: &SqlitePool,
    buffer: &mut VecDeque<RequestLogEntry>,
    app: &AppHandle,
    subscriptions: &Arc<RwLock<HashMap<Uuid, Arc<RwLock<SubscriptionRuntime>>>>>,
    dirty: &mut HashSet<Uuid>,
) {
    if buffer.is_empty() && dirty.is_empty() {
        return;
    }
    let batch: Vec<RequestLogEntry> = buffer.drain(..).collect();
    debug!(count = batch.len(), "flushing request logs");

    // 先在外层读锁下把 dirty id 解析成 Arc clone 的列表, 再释放外层锁, 逐个取内层读锁——
    // 与 run_consumer 的做法一致 (外层 guard 是临时值, 语句结束即释放), 不在持有外层锁时嵌套等内层锁。
    let dirty_runtimes: Vec<(Uuid, Arc<RwLock<SubscriptionRuntime>>)> = {
        let map = subscriptions.read().await;
        dirty
            .drain()
            .filter_map(|id| map.get(&id).cloned().map(|rt| (id, rt)))
            .collect()
    };
    let mut quota_rows: Vec<QuotaUsageRow> = Vec::new();
    for (id, rt) in dirty_runtimes {
        let g = rt.read().await;
        quota_rows.extend(usage_to_rows(id, &g.quota_usage));
    }

    match flush_batch(pool, batch, quota_rows).await {
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
    quota_rows: Vec<QuotaUsageRow>,
) -> Result<(), FlushError> {
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(err) => return Err(FlushError::BeginFailed { batch, err }),
    };

    // key → (provider_id, counters); provider_id 取首次写入的 entry 值。
    let mut stats_acc: HashMap<(i64, String, String), (String, StatsCounters)> = HashMap::new();
    // 小票聚合: key 多 real_model_name 一维 (receipt_stats_daily)。
    let mut receipt_acc: HashMap<(i64, String, String, String), (String, ReceiptCounters)> =
        HashMap::new();

    for entry in batch {
        let key = (
            floor_to_utc_day(entry.timestamp_ms),
            entry.virtual_model_name.as_str().to_string(),
            entry.subscription_id.to_string(),
        );

        let (_, racc) = receipt_acc
            .entry((key.0, key.1.clone(), key.2.clone(), entry.real_model_name.clone()))
            .or_insert_with(|| (entry.provider_id.clone(), ReceiptCounters::default()));
        racc.request_count += 1;
        racc.input_tokens += entry.upstream_input_tokens.unwrap_or(0) as i64;
        racc.output_tokens += entry.upstream_output_tokens.unwrap_or(0) as i64;
        racc.cache_creation_tokens += entry.upstream_cache_creation.unwrap_or(0) as i64;
        racc.cache_read_tokens += entry.upstream_cache_read.unwrap_or(0) as i64;

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
                client_tool, client_user_agent, client_version, client_ip,
                entry_kind, downstream_http_version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .bind(entry.entry_kind)
        .bind(entry.downstream_http_version)
        .execute(&mut *tx)
        .await;
        if let Err(e) = result {
            warn!(?e, "写入单条请求日志失败");
        }
    }

    // 同事务 UPSERT 聚合结果。requests + 两张 stats 表同进同退,
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

    for ((date_utc, vm, sub_id, real_model), (provider_id, acc)) in receipt_acc {
        let result = sqlx::query(
            "INSERT INTO receipt_stats_daily (
                date_utc, virtual_model_name, subscription_id, real_model_name, provider_id,
                request_count, input_tokens, output_tokens,
                cache_creation_tokens, cache_read_tokens
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (date_utc, virtual_model_name, subscription_id, real_model_name) DO UPDATE SET
                request_count = request_count + excluded.request_count,
                input_tokens = input_tokens + excluded.input_tokens,
                output_tokens = output_tokens + excluded.output_tokens,
                cache_creation_tokens = cache_creation_tokens + excluded.cache_creation_tokens,
                cache_read_tokens = cache_read_tokens + excluded.cache_read_tokens",
        )
        .bind(date_utc)
        .bind(vm)
        .bind(sub_id)
        .bind(real_model)
        .bind(provider_id)
        .bind(acc.request_count)
        .bind(acc.input_tokens)
        .bind(acc.output_tokens)
        .bind(acc.cache_creation_tokens)
        .bind(acc.cache_read_tokens)
        .execute(&mut *tx)
        .await;
        if let Err(e) = result {
            warn!(?e, "UPSERT 小票聚合失败");
        }
    }

    if let Err(e) = save_quota_usage_rows(&mut tx, &quota_rows, now_ms()).await {
        warn!(?e, "写 subscription_quota_usage 快照失败 (局部丢, 下次 flush 补写)");
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
            entry_kind: None,
            downstream_http_version: None,
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
        flush_batch(&pool, batch, vec![]).await.expect("flush ok");

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
        flush_batch(&pool, batch, vec![]).await.expect("flush ok");

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
            vec![],
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
            vec![],
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

    #[test]
    fn apply_entry_to_quota_accumulates_and_reports_first_crossing() {
        use crate::subscription::model::{SubscriptionRow, SubscriptionRuntime};
        use crate::subscription::quota::{QuotaPeriod, TokenQuotas};
        let mut row = SubscriptionRow::test_fixture("p", "e");
        row.token_quotas = TokenQuotas { daily: Some(150), ..Default::default() };
        let mut rt = SubscriptionRuntime::from_row(row);
        let now = Utc::now();
        let sub = rt.row.id;
        let mut e = make_entry(now.timestamp_millis(), VirtualModelName::Sonnet, sub, "p", RequestStatus::Success, Some(1), Some(100), Some(20));
        e.upstream_cache_creation = Some(0);
        e.upstream_cache_read = Some(0);
        assert_eq!(apply_entry_to_quota(&mut rt, &e, now), None); // 120 < 150
        assert_eq!(rt.quota_usage.bucket(QuotaPeriod::Daily).total(), 120);
        assert_eq!(apply_entry_to_quota(&mut rt, &e, now), Some(QuotaPeriod::Daily)); // 240 >= 150, 首次跨
        assert_eq!(apply_entry_to_quota(&mut rt, &e, now), None); // 已达标, 不再重复报
        // usage 为 None 的 entry (失败请求) 不改变计数
        let mut e2 = e.clone();
        e2.upstream_input_tokens = None; e2.upstream_output_tokens = None;
        let before = rt.quota_usage.bucket(QuotaPeriod::Daily).total();
        apply_entry_to_quota(&mut rt, &e2, now);
        assert_eq!(rt.quota_usage.bucket(QuotaPeriod::Daily).total(), before);
    }

    #[tokio::test]
    async fn flush_batch_persists_quota_rows() {
        use crate::subscription::quota::{QuotaPeriod, QuotaUsage};
        use crate::subscription::store::usage_to_rows;
        let pool = fresh_pool().await;
        let sub = Uuid::new_v4();
        let now = Utc::now();
        let mut u = QuotaUsage::default();
        u.add(now, 5, 6, 7, 8);
        let rows = usage_to_rows(sub, &u);
        flush_batch(&pool, vec![], rows).await.expect("flush ok");
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM subscription_quota_usage").fetch_one(&pool).await.unwrap();
        assert_eq!(n, 4);
        let total: i64 = sqlx::query_scalar(
            "SELECT input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens
             FROM subscription_quota_usage WHERE subscription_id = ? AND period = ?")
            .bind(sub.to_string()).bind(QuotaPeriod::Weekly.as_str())
            .fetch_one(&pool).await.unwrap();
        assert_eq!(total, 26);
    }

    #[tokio::test]
    async fn flush_writes_receipt_stats_with_real_model_dimension() {
        let pool = fresh_pool().await;
        let sub = Uuid::new_v4();
        let day = floor_to_utc_day(1_704_067_200_000);

        // 同 (day, vm, sub) 下两个 real_model: claude-x ×2 + other-model ×1
        let e1 = make_entry(day, VirtualModelName::Sonnet, sub, "anthropic",
                            RequestStatus::Success, None, Some(10), Some(20));
        let e2 = make_entry(day, VirtualModelName::Sonnet, sub, "anthropic",
                            RequestStatus::Error, None, Some(5), Some(5));
        let mut e3 = make_entry(day, VirtualModelName::Sonnet, sub, "anthropic",
                                RequestStatus::Success, None, Some(1), Some(1));
        e3.real_model_name = "other-model".to_string();
        flush_batch(&pool, vec![e1, e2, e3], vec![]).await.expect("first flush");

        let count: i64 = sqlx::query("SELECT COUNT(*) AS c FROM receipt_stats_daily")
            .fetch_one(&pool).await.unwrap().try_get("c").unwrap();
        assert_eq!(count, 2, "real_model_name 是聚合 key 的一维");

        let row = sqlx::query(
            "SELECT * FROM receipt_stats_daily WHERE real_model_name='claude-x'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.try_get::<i64, _>("request_count").unwrap(), 2);
        assert_eq!(row.try_get::<i64, _>("input_tokens").unwrap(), 15);
        assert_eq!(row.try_get::<i64, _>("output_tokens").unwrap(), 25);
        assert_eq!(row.try_get::<i64, _>("date_utc").unwrap(), day);
        assert_eq!(row.try_get::<String, _>("provider_id").unwrap(), "anthropic");

        // 二次 flush 相同 key → UPSERT 累加而非新行
        let e4 = make_entry(day, VirtualModelName::Sonnet, sub, "anthropic",
                            RequestStatus::Success, None, Some(3), Some(4));
        flush_batch(&pool, vec![e4], vec![]).await.expect("second flush");

        let row = sqlx::query(
            "SELECT * FROM receipt_stats_daily WHERE real_model_name='claude-x'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.try_get::<i64, _>("request_count").unwrap(), 3);
        assert_eq!(row.try_get::<i64, _>("input_tokens").unwrap(), 18);
        assert_eq!(row.try_get::<i64, _>("output_tokens").unwrap(), 29);
    }
}
