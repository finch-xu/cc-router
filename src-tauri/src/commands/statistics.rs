//! 统计聚合查询 commands。
//!
//! 数据来源:
//! - `request_stats_daily`: 按 (day, virtual_model_name, subscription_id) 三维聚合, key 是**本地日历日**
//!   `YYYY-MM-DD` (migration 019 起, 与限额一样按机器本地日历切桶), 永久保留, 不受 log_retention_days 影响。
//! - `requests`: p95 延迟 (聚合表存不下分位数) 与「今天」的按小时序列 (聚合表最细只到天) 走原始表,
//!   老数据被 cleanup 删除后这两项会失真 (retention 默认永久, 影响有限)。
//!
//! 时间过滤统一用 `since_day` (本地日 key 下限, inclusive, 字符串比较)。AllTime → ""。

use serde::{Deserialize, Serialize};
use sqlx::Row;
use tauri::State;

use crate::error::AppResult;
use crate::observability::request_log::{
    local_day_key, local_days_ago_key, local_days_ago_start_ms, now_ms,
};
use crate::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatsRange {
    Today,
    Last7Days,
    Last30Days,
    Last90Days,
    AllTime,
}

impl StatsRange {
    /// 聚合表过滤下限 (本地日 key, inclusive)。空串 ≤ 任何日期, 即 AllTime。
    fn since_day(self) -> String {
        let now = now_ms();
        match self {
            Self::Today => local_day_key(now),
            Self::Last7Days => local_days_ago_key(now, 6),
            Self::Last30Days => local_days_ago_key(now, 29),
            Self::Last90Days => local_days_ago_key(now, 89),
            Self::AllTime => String::new(),
        }
    }

    /// 查 `requests` 原始表时用的瞬时下限 (ms)。
    fn since_ms(self) -> i64 {
        let now = now_ms();
        match self {
            Self::Today => local_days_ago_start_ms(now, 0),
            Self::Last7Days => local_days_ago_start_ms(now, 6),
            Self::Last30Days => local_days_ago_start_ms(now, 29),
            Self::Last90Days => local_days_ago_start_ms(now, 89),
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
    let since_day = range.since_day();
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
         FROM request_stats_daily WHERE day >= ?",
    )
    .bind(&since_day)
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
    let since_ms = range.since_ms();
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
    .bind(since_ms)
    .bind(since_ms)
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

/// 时间序列的一个桶。`range != Today` 时是一天 (`hour = None`);
/// `range == Today` 时是一小时 (`hour = Some(0..=23)`, 本地整点)。
#[derive(Debug, Serialize)]
pub struct DailySeriesPointDto {
    /// 本地日历日 `YYYY-MM-DD`
    pub day: String,
    /// 本地小时 (0–23), 仅按小时分桶时有值
    pub hour: Option<i64>,
    pub request_count: i64,
    pub success_count: i64,
    pub error_count: i64,
    pub timeout_count: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_creation_tokens: i64,
    pub total_cache_read_tokens: i64,
    pub avg_duration_ms: Option<f64>,
}

/// 「今天」按小时分桶: 聚合表最细只到天, 走 `requests` 原始表 (受 log_retention_days 影响,
/// 默认永久保留)。SQL 里 strftime(...,'localtime') 按机器本地时区取小时, 半时区 (印度/尼泊尔) 也正确。
/// 其余 range 走聚合表按天。后端只返回有数据的桶, 前端负责补零填满。
#[tauri::command]
pub async fn get_daily_series(
    state: State<'_, AppState>,
    range: StatsRange,
) -> AppResult<Vec<DailySeriesPointDto>> {
    if range == StatsRange::Today {
        return hourly_series_today(&state.db, range.since_ms(), &range.since_day()).await;
    }
    let since_day = range.since_day();
    let rows = sqlx::query(
        "SELECT day,
                SUM(request_count) AS request_count,
                SUM(success_count) AS success_count,
                SUM(error_count)   AS error_count,
                SUM(timeout_count) AS timeout_count,
                SUM(input_tokens)  AS total_input_tokens,
                SUM(output_tokens) AS total_output_tokens,
                SUM(cache_creation_tokens) AS total_cache_creation_tokens,
                SUM(cache_read_tokens)     AS total_cache_read_tokens,
                SUM(total_duration_ms_sum)   AS dur_sum,
                SUM(total_duration_ms_count) AS dur_count
         FROM request_stats_daily
         WHERE day >= ?
         GROUP BY day
         ORDER BY day ASC",
    )
    .bind(&since_day)
    .fetch_all(&state.db)
    .await?;

    rows.into_iter()
        .map(|r| {
            let dur_sum: i64 = r.try_get("dur_sum")?;
            let dur_count: i64 = r.try_get("dur_count")?;
            Ok(DailySeriesPointDto {
                day: r.try_get("day")?,
                hour: None,
                request_count: r.try_get("request_count")?,
                success_count: r.try_get("success_count")?,
                error_count: r.try_get("error_count")?,
                timeout_count: r.try_get("timeout_count")?,
                total_input_tokens: r.try_get("total_input_tokens")?,
                total_output_tokens: r.try_get("total_output_tokens")?,
                total_cache_creation_tokens: r.try_get("total_cache_creation_tokens")?,
                total_cache_read_tokens: r.try_get("total_cache_read_tokens")?,
                avg_duration_ms: avg(dur_sum, dur_count),
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(Into::into)
}

async fn hourly_series_today(
    db: &sqlx::SqlitePool,
    since_ms: i64,
    today: &str,
) -> AppResult<Vec<DailySeriesPointDto>> {
    // timestamp >= 本地今日 0 点瞬时 走索引; day = today 再兜一层 (DST 边界)。
    let rows = sqlx::query(
        "SELECT strftime('%Y-%m-%d', timestamp / 1000, 'unixepoch', 'localtime') AS day,
                CAST(strftime('%H', timestamp / 1000, 'unixepoch', 'localtime') AS INTEGER) AS hour,
                COUNT(*) AS request_count,
                SUM(status = 'success') AS success_count,
                SUM(status = 'error')   AS error_count,
                SUM(status = 'timeout') AS timeout_count,
                COALESCE(SUM(upstream_input_tokens), 0)    AS total_input_tokens,
                COALESCE(SUM(upstream_output_tokens), 0)   AS total_output_tokens,
                COALESCE(SUM(upstream_cache_creation), 0)  AS total_cache_creation_tokens,
                COALESCE(SUM(upstream_cache_read), 0)      AS total_cache_read_tokens,
                COALESCE(SUM(total_latency_ms), 0) AS dur_sum,
                COUNT(total_latency_ms)            AS dur_count
         FROM requests
         WHERE timestamp >= ? AND day = ?
         GROUP BY day, hour
         ORDER BY hour ASC",
    )
    .bind(since_ms)
    .bind(today)
    .fetch_all(db)
    .await?;

    rows.into_iter()
        .map(|r| {
            let dur_sum: i64 = r.try_get("dur_sum")?;
            let dur_count: i64 = r.try_get("dur_count")?;
            Ok(DailySeriesPointDto {
                day: r.try_get("day")?,
                hour: Some(r.try_get("hour")?),
                request_count: r.try_get("request_count")?,
                success_count: r.try_get("success_count")?,
                error_count: r.try_get("error_count")?,
                timeout_count: r.try_get("timeout_count")?,
                total_input_tokens: r.try_get("total_input_tokens")?,
                total_output_tokens: r.try_get("total_output_tokens")?,
                total_cache_creation_tokens: r.try_get("total_cache_creation_tokens")?,
                total_cache_read_tokens: r.try_get("total_cache_read_tokens")?,
                avg_duration_ms: avg(dur_sum, dur_count),
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(Into::into)
}

fn avg(sum: i64, count: i64) -> Option<f64> {
    if count > 0 {
        Some(sum as f64 / count as f64)
    } else {
        None
    }
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
    pub total_cache_creation_tokens: i64,
    pub total_cache_read_tokens: i64,
    pub avg_duration_ms: Option<f64>,
}

#[tauri::command]
pub async fn get_breakdown(
    state: State<'_, AppState>,
    range: StatsRange,
    by: BreakdownBy,
) -> AppResult<Vec<BreakdownDto>> {
    let since_day = range.since_day();
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
                    SUM(cache_creation_tokens) AS total_cache_creation_tokens,
                    SUM(cache_read_tokens)     AS total_cache_read_tokens,
                    SUM(total_duration_ms_sum)   AS dur_sum,
                    SUM(total_duration_ms_count) AS dur_count
             FROM request_stats_daily
             WHERE day >= ?
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
                    SUM(s.cache_creation_tokens) AS total_cache_creation_tokens,
                    SUM(s.cache_read_tokens)     AS total_cache_read_tokens,
                    SUM(s.total_duration_ms_sum)   AS dur_sum,
                    SUM(s.total_duration_ms_count) AS dur_count
             FROM request_stats_daily s
             LEFT JOIN subscriptions sub ON sub.id = s.subscription_id
             WHERE s.day >= ?
             GROUP BY s.subscription_id
             ORDER BY request_count DESC"
        }
    };

    let rows = sqlx::query(sql).bind(&since_day).fetch_all(&state.db).await?;

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
                total_cache_creation_tokens: r.try_get("total_cache_creation_tokens")?,
                total_cache_read_tokens: r.try_get("total_cache_read_tokens")?,
                avg_duration_ms: avg(dur_sum, dur_count),
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(Into::into)
}

#[derive(Debug, Serialize)]
pub struct HeatmapDayDto {
    /// 本地日历日 `YYYY-MM-DD`
    pub day: String,
    /// input + output tokens (不含缓存两项: OpenAI 系 input 已含 cached, 相加会双计)
    pub total_tokens: i64,
    pub request_count: i64,
}

#[tauri::command]
pub async fn get_token_heatmap(
    state: State<'_, AppState>,
    days: u32,
) -> AppResult<Vec<HeatmapDayDto>> {
    let days = days.clamp(1, 730);
    let since_day = local_days_ago_key(now_ms(), days - 1);

    let rows = sqlx::query(
        "SELECT day,
                SUM(input_tokens + output_tokens) AS total_tokens,
                SUM(request_count) AS request_count
         FROM request_stats_daily
         WHERE day >= ?
         GROUP BY day
         ORDER BY day ASC",
    )
    .bind(&since_day)
    .fetch_all(&state.db)
    .await?;

    rows.into_iter()
        .map(|r| {
            Ok(HeatmapDayDto {
                day: r.try_get("day")?,
                total_tokens: r.try_get("total_tokens")?,
                request_count: r.try_get("request_count")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(Into::into)
}
