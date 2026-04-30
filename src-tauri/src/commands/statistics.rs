//! 统计聚合查询 commands。
//!
//! 数据来源:
//! - `request_stats_daily`: 按 (date_utc, virtual_model_name, subscription_id) 三维聚合,
//!   永久保留, 不受 log_retention_days 影响。
//! - `requests`: 仅 p95 延迟用 (聚合表存不下分位数), 老数据被 cleanup 删除后该指标会失真。
//!
//! 时间过滤统一用 `since_ms` (UTC 0 点 ms 下限, inclusive)。AllTime → 0。

use serde::{Deserialize, Serialize};
use sqlx::Row;
use tauri::State;

use crate::error::AppResult;
use crate::observability::request_log::{floor_to_utc_day, now_ms, DAY_MS};
use crate::state::AppState;

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatsRange {
    Today,
    Last7Days,
    Last30Days,
    Last90Days,
    AllTime,
}

impl StatsRange {
    fn since_ms(self) -> i64 {
        let today = floor_to_utc_day(now_ms());
        match self {
            Self::Today => today,
            Self::Last7Days => today - 6 * DAY_MS,
            Self::Last30Days => today - 29 * DAY_MS,
            Self::Last90Days => today - 89 * DAY_MS,
            Self::AllTime => 0,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct OverallStatsDto {
    pub total_requests: i64,
    pub success_count: i64,
    pub error_count: i64,
    pub timeout_count: i64,
    pub success_rate_pct: f64,
    pub avg_duration_ms: Option<f64>,
    pub p95_duration_ms: Option<i64>,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_creation_tokens: i64,
    pub total_cache_read_tokens: i64,
}

#[tauri::command]
pub async fn get_overall_stats(
    state: State<'_, AppState>,
    range: StatsRange,
) -> AppResult<OverallStatsDto> {
    let since = range.since_ms();
    let row = sqlx::query(
        "SELECT
            COALESCE(SUM(request_count), 0) AS total_requests,
            COALESCE(SUM(success_count), 0) AS success_count,
            COALESCE(SUM(error_count), 0)   AS error_count,
            COALESCE(SUM(timeout_count), 0) AS timeout_count,
            COALESCE(SUM(input_tokens), 0)  AS total_input_tokens,
            COALESCE(SUM(output_tokens), 0) AS total_output_tokens,
            COALESCE(SUM(cache_creation_tokens), 0) AS total_cache_creation_tokens,
            COALESCE(SUM(cache_read_tokens), 0)     AS total_cache_read_tokens,
            COALESCE(SUM(total_duration_ms_sum), 0)   AS dur_sum,
            COALESCE(SUM(total_duration_ms_count), 0) AS dur_count
         FROM request_stats_daily WHERE date_utc >= ?",
    )
    .bind(since)
    .fetch_one(&state.db)
    .await?;

    let total_requests: i64 = row.try_get("total_requests")?;
    let success_count: i64 = row.try_get("success_count")?;
    let error_count: i64 = row.try_get("error_count")?;
    let timeout_count: i64 = row.try_get("timeout_count")?;
    let dur_sum: i64 = row.try_get("dur_sum")?;
    let dur_count: i64 = row.try_get("dur_count")?;

    let success_rate_pct = if total_requests > 0 {
        (success_count as f64) / (total_requests as f64) * 100.0
    } else {
        0.0
    };
    let avg_duration_ms = if dur_count > 0 {
        Some(dur_sum as f64 / dur_count as f64)
    } else {
        None
    };

    // p95 从 requests 表算 (cleanup 删后失真, 接受)。
    // 用 LIMIT 1 OFFSET 直接取第 95 分位, 避免把所有 latency fetch 到 Rust 再排序——
    // SQLite 已能用索引扫到分位点, AllTime + 大表场景下省下整表传输开销。
    let p95_duration_ms: Option<i64> = sqlx::query_scalar(
        "SELECT total_latency_ms FROM requests
         WHERE timestamp >= ? AND total_latency_ms IS NOT NULL
         ORDER BY total_latency_ms ASC
         LIMIT 1 OFFSET MAX(
            (SELECT CAST(0.95 * COUNT(*) AS INT) - 1 FROM requests
             WHERE timestamp >= ? AND total_latency_ms IS NOT NULL),
            0
         )",
    )
    .bind(since)
    .bind(since)
    .fetch_optional(&state.db)
    .await?;

    Ok(OverallStatsDto {
        total_requests,
        success_count,
        error_count,
        timeout_count,
        success_rate_pct,
        avg_duration_ms,
        p95_duration_ms,
        total_input_tokens: row.try_get("total_input_tokens")?,
        total_output_tokens: row.try_get("total_output_tokens")?,
        total_cache_creation_tokens: row.try_get("total_cache_creation_tokens")?,
        total_cache_read_tokens: row.try_get("total_cache_read_tokens")?,
    })
}

#[derive(Debug, Serialize)]
pub struct DailySeriesPointDto {
    pub date_utc: i64,
    pub request_count: i64,
    pub success_count: i64,
    pub error_count: i64,
    pub timeout_count: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub avg_duration_ms: Option<f64>,
}

#[tauri::command]
pub async fn get_daily_series(
    state: State<'_, AppState>,
    range: StatsRange,
) -> AppResult<Vec<DailySeriesPointDto>> {
    let since = range.since_ms();
    let rows = sqlx::query(
        "SELECT date_utc,
                SUM(request_count) AS request_count,
                SUM(success_count) AS success_count,
                SUM(error_count)   AS error_count,
                SUM(timeout_count) AS timeout_count,
                SUM(input_tokens)  AS total_input_tokens,
                SUM(output_tokens) AS total_output_tokens,
                SUM(total_duration_ms_sum)   AS dur_sum,
                SUM(total_duration_ms_count) AS dur_count
         FROM request_stats_daily
         WHERE date_utc >= ?
         GROUP BY date_utc
         ORDER BY date_utc ASC",
    )
    .bind(since)
    .fetch_all(&state.db)
    .await?;

    rows.into_iter()
        .map(|r| {
            let dur_sum: i64 = r.try_get("dur_sum")?;
            let dur_count: i64 = r.try_get("dur_count")?;
            Ok(DailySeriesPointDto {
                date_utc: r.try_get("date_utc")?,
                request_count: r.try_get("request_count")?,
                success_count: r.try_get("success_count")?,
                error_count: r.try_get("error_count")?,
                timeout_count: r.try_get("timeout_count")?,
                total_input_tokens: r.try_get("total_input_tokens")?,
                total_output_tokens: r.try_get("total_output_tokens")?,
                avg_duration_ms: if dur_count > 0 {
                    Some(dur_sum as f64 / dur_count as f64)
                } else {
                    None
                },
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(Into::into)
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BreakdownBy {
    VirtualModel,
    Subscription,
}

#[derive(Debug, Serialize)]
pub struct BreakdownDto {
    pub key: String,
    pub label: String,
    pub request_count: i64,
    pub success_count: i64,
    pub error_count: i64,
    pub timeout_count: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub avg_duration_ms: Option<f64>,
}

#[tauri::command]
pub async fn get_breakdown(
    state: State<'_, AppState>,
    range: StatsRange,
    by: BreakdownBy,
) -> AppResult<Vec<BreakdownDto>> {
    let since = range.since_ms();
    let sql = match by {
        BreakdownBy::VirtualModel => {
            "SELECT virtual_model_name AS key,
                    virtual_model_name AS label,
                    SUM(request_count) AS request_count,
                    SUM(success_count) AS success_count,
                    SUM(error_count)   AS error_count,
                    SUM(timeout_count) AS timeout_count,
                    SUM(input_tokens)  AS total_input_tokens,
                    SUM(output_tokens) AS total_output_tokens,
                    SUM(total_duration_ms_sum)   AS dur_sum,
                    SUM(total_duration_ms_count) AS dur_count
             FROM request_stats_daily
             WHERE date_utc >= ?
             GROUP BY virtual_model_name
             ORDER BY request_count DESC"
        }
        BreakdownBy::Subscription => {
            // LEFT JOIN: 订阅可能已被删除, 但 stats 仍有历史数据
            "SELECT s.subscription_id AS key,
                    COALESCE(sub.display_name, '(已删除订阅)') AS label,
                    SUM(s.request_count) AS request_count,
                    SUM(s.success_count) AS success_count,
                    SUM(s.error_count)   AS error_count,
                    SUM(s.timeout_count) AS timeout_count,
                    SUM(s.input_tokens)  AS total_input_tokens,
                    SUM(s.output_tokens) AS total_output_tokens,
                    SUM(s.total_duration_ms_sum)   AS dur_sum,
                    SUM(s.total_duration_ms_count) AS dur_count
             FROM request_stats_daily s
             LEFT JOIN subscriptions sub ON sub.id = s.subscription_id
             WHERE s.date_utc >= ?
             GROUP BY s.subscription_id
             ORDER BY request_count DESC"
        }
    };

    let rows = sqlx::query(sql).bind(since).fetch_all(&state.db).await?;

    rows.into_iter()
        .map(|r| {
            let dur_sum: i64 = r.try_get("dur_sum")?;
            let dur_count: i64 = r.try_get("dur_count")?;
            Ok(BreakdownDto {
                key: r.try_get("key")?,
                label: r.try_get("label")?,
                request_count: r.try_get("request_count")?,
                success_count: r.try_get("success_count")?,
                error_count: r.try_get("error_count")?,
                timeout_count: r.try_get("timeout_count")?,
                total_input_tokens: r.try_get("total_input_tokens")?,
                total_output_tokens: r.try_get("total_output_tokens")?,
                avg_duration_ms: if dur_count > 0 {
                    Some(dur_sum as f64 / dur_count as f64)
                } else {
                    None
                },
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(Into::into)
}

#[derive(Debug, Serialize)]
pub struct HeatmapDayDto {
    pub date_utc: i64,
    pub total_tokens: i64,
    pub request_count: i64,
}

#[tauri::command]
pub async fn get_token_heatmap(
    state: State<'_, AppState>,
    days: u32,
) -> AppResult<Vec<HeatmapDayDto>> {
    let days = days.clamp(1, 730) as i64;
    let since = floor_to_utc_day(now_ms()) - (days - 1) * DAY_MS;

    let rows = sqlx::query(
        "SELECT date_utc,
                SUM(input_tokens + output_tokens) AS total_tokens,
                SUM(request_count) AS request_count
         FROM request_stats_daily
         WHERE date_utc >= ?
         GROUP BY date_utc
         ORDER BY date_utc ASC",
    )
    .bind(since)
    .fetch_all(&state.db)
    .await?;

    rows.into_iter()
        .map(|r| {
            Ok(HeatmapDayDto {
                date_utc: r.try_get("date_utc")?,
                total_tokens: r.try_get("total_tokens")?,
                request_count: r.try_get("request_count")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(Into::into)
}
