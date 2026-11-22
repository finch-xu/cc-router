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
    /// 工具调用总次数 (tool_use + server_tool_use 块数之和)
    pub total_tool_use_count: i64,
    /// 至少发起一次工具调用的请求数; 前端算占比时分母用 success_count
    pub tool_use_request_count: i64,
    /// 等长的上一周期 (7 天 → 再往前 7 天; 今天 → 昨天 0 点到昨天此刻)。AllTime 没有。
    pub previous: Option<PeriodTotalsDto>,
}

/// 「较上一周期」用的精简汇总。字段名与 [`OverallStatsDto`] 同名同义。
#[derive(Debug, Serialize, PartialEq)]
pub struct PeriodTotalsDto {
    pub total_requests: i64,
    pub success_count: i64,
    pub avg_duration_ms: Option<f64>,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_creation_tokens: i64,
    pub total_cache_read_tokens: i64,
    pub tool_use_request_count: i64,
}

impl StatsRange {
    /// 上一周期的本地日 key 区间 `[since, until)`; Today / AllTime 不走聚合表。
    fn previous_days(self, now: i64) -> Option<(String, String)> {
        let n = match self {
            Self::Last7Days => 7,
            Self::Last30Days => 30,
            Self::Last90Days => 90,
            Self::Today | Self::AllTime => return None,
        };
        Some((local_days_ago_key(now, 2 * n - 1), local_days_ago_key(now, n - 1)))
    }
}

/// 聚合表上 `[since_day, until_day)` 的汇总。
pub(crate) async fn daily_totals_between(
    pool: &sqlx::SqlitePool,
    since_day: &str,
    until_day: &str,
) -> Result<PeriodTotalsDto, sqlx::Error> {
    let r = sqlx::query(
        "SELECT COALESCE(SUM(request_count), 0) AS n,
                COALESCE(SUM(success_count), 0) AS ok,
                COALESCE(SUM(input_tokens), 0) AS i,
                COALESCE(SUM(output_tokens), 0) AS o,
                COALESCE(SUM(cache_creation_tokens), 0) AS cw,
                COALESCE(SUM(cache_read_tokens), 0) AS cr,
                COALESCE(SUM(total_duration_ms_sum), 0) AS ds,
                COALESCE(SUM(total_duration_ms_count), 0) AS dc,
                COALESCE(SUM(tool_use_request_count), 0) AS t
         FROM request_stats_daily WHERE day >= ? AND day < ?",
    )
    .bind(since_day)
    .bind(until_day)
    .fetch_one(pool)
    .await?;
    totals_from_row(&r)
}

/// `requests` 原始表上 `[since_ms, until_ms)` 的汇总 (「今天」对比「昨天同一时段」用)。
pub(crate) async fn raw_totals_between(
    pool: &sqlx::SqlitePool,
    since_ms: i64,
    until_ms: i64,
) -> Result<PeriodTotalsDto, sqlx::Error> {
    let r = sqlx::query(
        "SELECT COUNT(*) AS n,
                COALESCE(SUM(status = 'success'), 0) AS ok,
                COALESCE(SUM(upstream_input_tokens), 0) AS i,
                COALESCE(SUM(upstream_output_tokens), 0) AS o,
                COALESCE(SUM(upstream_cache_creation), 0) AS cw,
                COALESCE(SUM(upstream_cache_read), 0) AS cr,
                COALESCE(SUM(total_latency_ms), 0) AS ds,
                COUNT(total_latency_ms) AS dc,
                COALESCE(SUM(tool_use_count > 0), 0) AS t
         FROM requests WHERE timestamp >= ? AND timestamp < ?",
    )
    .bind(since_ms)
    .bind(until_ms)
    .fetch_one(pool)
    .await?;
    totals_from_row(&r)
}

fn totals_from_row(r: &sqlx::sqlite::SqliteRow) -> Result<PeriodTotalsDto, sqlx::Error> {
    Ok(PeriodTotalsDto {
        total_requests: r.try_get("n")?,
        success_count: r.try_get("ok")?,
        avg_duration_ms: avg(r.try_get("ds")?, r.try_get("dc")?),
        total_input_tokens: r.try_get("i")?,
        total_output_tokens: r.try_get("o")?,
        total_cache_creation_tokens: r.try_get("cw")?,
        total_cache_read_tokens: r.try_get("cr")?,
        tool_use_request_count: r.try_get("t")?,
    })
}

async fn previous_totals(
    pool: &sqlx::SqlitePool,
    range: StatsRange,
    now: i64,
) -> Result<Option<PeriodTotalsDto>, sqlx::Error> {
    if range == StatsRange::Today {
        let start = local_days_ago_start_ms(now, 1);
        let today_start = local_days_ago_start_ms(now, 0);
        // 昨天 0 点 → 昨天的「此刻」(按经过的时长算, DST 当天差一小时可以接受)
        return raw_totals_between(pool, start, start + (now - today_start)).await.map(Some);
    }
    match range.previous_days(now) {
        Some((since, until)) => daily_totals_between(pool, &since, &until).await.map(Some),
        None => Ok(None),
    }
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
            COALESCE(SUM(total_duration_ms_count), 0) AS dur_count,
            COALESCE(SUM(tool_use_count), 0) AS total_tool_use_count,
            COALESCE(SUM(tool_use_request_count), 0) AS tool_use_request_count
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
    // 实测 SQLite 走 idx_timestamp 过滤 timestamp 后用临时 B-tree 排序 (无可用的 latency
    // 索引, 加一个也不会被这条查询用上); AllTime + 大表场景下是整表排序, 按 spec 接受。
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
        total_tool_use_count: row.try_get("total_tool_use_count")?,
        tool_use_request_count: row.try_get("tool_use_request_count")?,
        previous: previous_totals(&state.db, range, now_ms()).await?,
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
    pub tool_use_count: i64,
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
                    SUM(total_duration_ms_count) AS dur_count,
                    SUM(tool_use_count) AS tool_use_count
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
                    SUM(s.total_duration_ms_count) AS dur_count,
                    SUM(s.tool_use_count) AS tool_use_count
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
                tool_use_count: r.try_get("tool_use_count")?,
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

#[derive(Debug, Serialize)]
pub struct ToolBreakdownDto {
    pub tool_name: String,
    /// requests.client_tool 原值; 哨兵 `__unknown__` 映射为 None (前端渲染「未识别」)
    pub client_tool: Option<String>,
    pub call_count: i64,
}

/// 纯 DB 查询, 供 command 与单测共用。`since_day` 空串 = AllTime。
pub(crate) async fn query_tool_breakdown(
    pool: &sqlx::SqlitePool,
    since_day: &str,
    limit: u32,
) -> Result<Vec<ToolBreakdownDto>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT tool_name, client_tool, SUM(call_count) AS call_count
         FROM tool_stats_daily
         WHERE day >= ?
         GROUP BY tool_name, client_tool
         ORDER BY call_count DESC, tool_name ASC
         LIMIT ?",
    )
    .bind(since_day)
    .bind(limit.clamp(1, 100) as i64)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|r| {
            let client: String = r.try_get("client_tool")?;
            Ok(ToolBreakdownDto {
                tool_name: r.try_get("tool_name")?,
                client_tool: if client == crate::observability::request_log::TOOL_STATS_UNKNOWN_CLIENT {
                    None
                } else {
                    Some(client)
                },
                call_count: r.try_get("call_count")?,
            })
        })
        .collect()
}

#[tauri::command]
pub async fn get_tool_breakdown(
    state: State<'_, AppState>,
    range: StatsRange,
    limit: u32,
) -> AppResult<Vec<ToolBreakdownDto>> {
    query_tool_breakdown(&state.db, &range.since_day(), limit)
        .await
        .map_err(Into::into)
}

#[derive(Debug, Serialize, Default, Clone, Copy, PartialEq)]
pub struct ActivityBucketDto {
    pub requests: i64,
    pub failures: i64,
}

/// 订阅列表的「近 24 小时」: 每个订阅 24 个整点桶 (最旧在前, 最后一格是当前这个小时)。
#[derive(Debug, Serialize, PartialEq)]
pub struct SubscriptionActivityDto {
    pub subscription_id: String,
    pub buckets: Vec<ActivityBucketDto>,
    pub requests: i64,
    pub failures: i64,
}

pub(crate) const ACTIVITY_HOURS: i64 = 24;
const HOUR_MS: i64 = 3_600_000;

pub(crate) async fn query_subscription_activity(
    pool: &sqlx::SqlitePool,
    now: i64,
) -> Result<Vec<SubscriptionActivityDto>, sqlx::Error> {
    // 按 UTC 整点切桶: 只有半小时时区会与本地整点错开, 对「近 24 小时」的形状无影响
    let start = (now.div_euclid(HOUR_MS) - (ACTIVITY_HOURS - 1)) * HOUR_MS;
    let rows = sqlx::query(
        "SELECT subscription_id, (timestamp - ?) / 3600000 AS b,
                COUNT(*) AS n, COALESCE(SUM(status != 'success'), 0) AS f
         FROM requests WHERE timestamp >= ?
         GROUP BY subscription_id, b",
    )
    .bind(start)
    .bind(start)
    .fetch_all(pool)
    .await?;
    let mut out: Vec<SubscriptionActivityDto> = Vec::new();
    for r in rows {
        let id: String = r.try_get("subscription_id")?;
        let b: i64 = r.try_get("b")?;
        // 时钟回拨写进来的「未来」行落不进任何桶
        if !(0..ACTIVITY_HOURS).contains(&b) {
            continue;
        }
        let (n, f): (i64, i64) = (r.try_get("n")?, r.try_get("f")?);
        let pos = match out.iter().position(|a| a.subscription_id == id) {
            Some(p) => p,
            None => {
                out.push(SubscriptionActivityDto {
                    subscription_id: id,
                    buckets: vec![ActivityBucketDto::default(); ACTIVITY_HOURS as usize],
                    requests: 0,
                    failures: 0,
                });
                out.len() - 1
            }
        };
        let a = &mut out[pos];
        a.buckets[b as usize] = ActivityBucketDto { requests: n, failures: f };
        a.requests += n;
        a.failures += f;
    }
    Ok(out)
}

#[tauri::command]
pub async fn get_subscription_activity(
    state: State<'_, AppState>,
) -> AppResult<Vec<SubscriptionActivityDto>> {
    query_subscription_activity(&state.db, now_ms()).await.map_err(Into::into)
}

#[derive(Debug, Serialize, PartialEq)]
pub struct PurposeCountDto {
    /// `requests.purpose_kind`; None = 迁移 030 之前写入的行 (没有用途)
    pub kind: Option<String>,
    pub count: i64,
    /// 其中由指纹推断 (`purpose_guessed = 1`) 的条数
    pub guessed: i64,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct ClientMixDto {
    /// `requests.client_tool`; None = 未识别
    pub client_tool: Option<String>,
    pub requests: i64,
    /// input + output (不含缓存两项, 同热力图口径)
    pub tokens: i64,
}

/// 统计页里只能从 `requests` 原始表算的几组分布 (受日志保留期限制)。
#[derive(Debug, Serialize, PartialEq)]
pub struct RequestMixDto {
    /// 原始表里落在范围内的请求数; 前端与聚合表的 total_requests 比, 得出覆盖率
    pub covered_requests: i64,
    pub purposes: Vec<PurposeCountDto>,
    pub clients: Vec<ClientMixDto>,
    /// 成功请求总耗时分桶, 上界 (ms, 不含) 见 [`LATENCY_EDGES_MS`], 最后一格是「更久」
    pub latency_buckets: Vec<i64>,
}

pub const LATENCY_EDGES_MS: [i64; 5] = [2_000, 5_000, 10_000, 30_000, 60_000];

pub(crate) async fn query_request_mix(
    pool: &sqlx::SqlitePool,
    since_ms: i64,
) -> Result<RequestMixDto, sqlx::Error> {
    let covered_requests: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM requests WHERE timestamp >= ?")
        .bind(since_ms)
        .fetch_one(pool)
        .await?;

    let purposes = sqlx::query(
        "SELECT purpose_kind AS k, COUNT(*) AS n, COALESCE(SUM(purpose_guessed = 1), 0) AS g
         FROM requests WHERE timestamp >= ?
         GROUP BY purpose_kind ORDER BY n DESC",
    )
    .bind(since_ms)
    .fetch_all(pool)
    .await?
    .iter()
    .map(|r| Ok(PurposeCountDto { kind: r.try_get("k")?, count: r.try_get("n")?, guessed: r.try_get("g")? }))
    .collect::<Result<Vec<_>, sqlx::Error>>()?;

    let clients = sqlx::query(
        "SELECT client_tool AS c, COUNT(*) AS n,
                COALESCE(SUM(upstream_input_tokens), 0) + COALESCE(SUM(upstream_output_tokens), 0) AS t
         FROM requests WHERE timestamp >= ?
         GROUP BY client_tool ORDER BY t DESC, n DESC",
    )
    .bind(since_ms)
    .fetch_all(pool)
    .await?
    .iter()
    .map(|r| Ok(ClientMixDto { client_tool: r.try_get("c")?, requests: r.try_get("n")?, tokens: r.try_get("t")? }))
    .collect::<Result<Vec<_>, sqlx::Error>>()?;

    let [e0, e1, e2, e3, e4] = LATENCY_EDGES_MS;
    let r = sqlx::query(
        "SELECT COALESCE(SUM(total_latency_ms < ?), 0) AS b0,
                COALESCE(SUM(total_latency_ms >= ? AND total_latency_ms < ?), 0) AS b1,
                COALESCE(SUM(total_latency_ms >= ? AND total_latency_ms < ?), 0) AS b2,
                COALESCE(SUM(total_latency_ms >= ? AND total_latency_ms < ?), 0) AS b3,
                COALESCE(SUM(total_latency_ms >= ? AND total_latency_ms < ?), 0) AS b4,
                COALESCE(SUM(total_latency_ms >= ?), 0) AS b5
         FROM requests
         WHERE timestamp >= ? AND status = 'success' AND total_latency_ms IS NOT NULL",
    )
    .bind(e0)
    .bind(e0).bind(e1)
    .bind(e1).bind(e2)
    .bind(e2).bind(e3)
    .bind(e3).bind(e4)
    .bind(e4)
    .bind(since_ms)
    .fetch_one(pool)
    .await?;
    let latency_buckets = (0..6)
        .map(|i| r.try_get::<i64, _>(format!("b{i}").as_str()))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(RequestMixDto { covered_requests, purposes, clients, latency_buckets })
}

#[tauri::command]
pub async fn get_request_mix(state: State<'_, AppState>, range: StatsRange) -> AppResult<RequestMixDto> {
    query_request_mix(&state.db, range.since_ms()).await.map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::run_migrations;
    use sqlx::sqlite::SqlitePoolOptions;
    use sqlx::SqlitePool;

    async fn fresh_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        run_migrations(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn tool_breakdown_orders_by_count_filters_range_and_maps_unknown() {
        let pool = fresh_pool().await;
        let today = local_day_key(now_ms());
        let old = "2000-01-01";
        for (day, client, name, n) in [
            (today.as_str(), "claude_code", "Read", 5),
            (today.as_str(), "claude_code", "Bash", 9),
            (today.as_str(), "__unknown__", "Read", 2),
            // 与 today 行故意用不同 tool_name (Edit≠Read): 查询按 (tool_name, client_tool) 聚合,
            // 若沿用 "Read" 会与 today 的 Read/claude_code 行跨日合并成 105, 使下面的 all[0]==100
            // 断言失真 —— GROUP BY 不含 day 是刻意设计 (统计范围内工具总调用数), 这里改测试数据而非查询。
            (old, "claude_code", "Edit", 100),
        ] {
            sqlx::query("INSERT INTO tool_stats_daily (day, client_tool, tool_name, call_count) VALUES (?, ?, ?, ?)")
                .bind(day).bind(client).bind(name).bind(n).execute(&pool).await.unwrap();
        }
        let rows = query_tool_breakdown(&pool, &StatsRange::Last7Days.since_day(), 10).await.unwrap();
        assert_eq!(rows.len(), 3, "旧日期不在 7 天内");
        assert_eq!((rows[0].tool_name.as_str(), rows[0].call_count), ("Bash", 9));
        assert_eq!(rows[0].client_tool.as_deref(), Some("claude_code"));
        assert_eq!((rows[1].tool_name.as_str(), rows[1].call_count), ("Read", 5));
        assert_eq!(rows[2].client_tool, None, "__unknown__ 哨兵映射为 None");

        let all = query_tool_breakdown(&pool, "", 2).await.unwrap();
        assert_eq!(all.len(), 2, "limit 生效");
        assert_eq!(all[0].call_count, 100);
    }
    #[allow(clippy::too_many_arguments)]
    async fn insert_req(pool: &SqlitePool, id: &str, ts: i64, sub: &str, status: &str, latency: Option<i64>,
        tokens: (i64, i64), purpose: Option<(&str, bool)>, client: Option<&str>) {
        sqlx::query(
            "INSERT INTO requests (id, timestamp, virtual_model_name, subscription_id, provider_id, endpoint_id,
                real_model_name, is_streaming, status, total_latency_ms, upstream_input_tokens, upstream_output_tokens,
                purpose_kind, purpose_guessed, client_tool, tool_use_count)
             VALUES (?, ?, 'model-opus', ?, 'p', 'e', 'm', 1, ?, ?, ?, ?, ?, ?, ?, 1)",
        )
        .bind(id).bind(ts).bind(sub).bind(status).bind(latency).bind(tokens.0).bind(tokens.1)
        .bind(purpose.map(|p| p.0)).bind(purpose.map(|p| p.1 as i64)).bind(client)
        .execute(pool).await.unwrap();
    }

    #[tokio::test]
    async fn subscription_activity_buckets_last_24_hours() {
        let pool = fresh_pool().await;
        let now = 1_800_000_000_000_i64 + 1_234_567; // 不在整点
        let hour = 3_600_000;
        insert_req(&pool, "a", now - 10, "s1", "success", None, (0, 0), None, None).await;
        insert_req(&pool, "b", now - 20, "s1", "error", None, (0, 0), None, None).await;
        insert_req(&pool, "c", now - 5 * hour, "s1", "success", None, (0, 0), None, None).await;
        insert_req(&pool, "d", now - 2 * hour, "s2", "timeout", None, (0, 0), None, None).await;
        insert_req(&pool, "old", now - 30 * hour, "s1", "success", None, (0, 0), None, None).await;
        let mut rows = query_subscription_activity(&pool, now).await.unwrap();
        rows.sort_by(|a, b| a.subscription_id.cmp(&b.subscription_id));
        assert_eq!(rows.len(), 2);
        let s1 = &rows[0];
        assert_eq!(s1.buckets.len(), 24);
        assert_eq!(s1.buckets[23], ActivityBucketDto { requests: 2, failures: 1 }, "当前小时在最后一格");
        assert_eq!(s1.buckets[18].requests, 1);
        assert_eq!((s1.requests, s1.failures), (3, 1), "30 小时前的不计");
        assert_eq!(rows[1].buckets[21], ActivityBucketDto { requests: 1, failures: 1 }, "timeout 也算失败");
    }

    #[tokio::test]
    async fn request_mix_groups_purpose_client_and_latency() {
        let pool = fresh_pool().await;
        let now = now_ms();
        insert_req(&pool, "1", now, "s", "success", Some(1_500), (10, 5), Some(("main", false)), Some("claude_code")).await;
        insert_req(&pool, "2", now, "s", "success", Some(7_000), (1, 1), Some(("agent", true)), Some("claude_code")).await;
        insert_req(&pool, "3", now, "s", "success", Some(90_000), (100, 0), Some(("agent", true)), Some("codex")).await;
        insert_req(&pool, "4", now, "s", "error", Some(500), (0, 0), None, None).await;
        insert_req(&pool, "old", 1_000, "s", "success", Some(1), (0, 0), Some(("main", false)), None).await;
        let mix = query_request_mix(&pool, now - 60_000).await.unwrap();
        assert_eq!(mix.covered_requests, 4);
        assert_eq!(mix.purposes[0], PurposeCountDto { kind: Some("agent".into()), count: 2, guessed: 2 });
        assert!(mix.purposes.iter().any(|p| p.kind.is_none() && p.count == 1), "迁移前的行 = 无用途");
        assert_eq!(mix.clients[0], ClientMixDto { client_tool: Some("codex".into()), requests: 1, tokens: 100 });
        assert_eq!(mix.latency_buckets, vec![1, 0, 1, 0, 0, 1], "失败请求不进耗时分布");
    }

    #[tokio::test]
    async fn previous_period_uses_equal_length_window() {
        let pool = fresh_pool().await;
        let now = now_ms();
        for (back, n) in [(0u32, 5i64), (6, 1), (7, 2), (13, 3), (14, 100)] {
            sqlx::query("INSERT INTO request_stats_daily (day, virtual_model_name, subscription_id, provider_id, request_count, success_count) VALUES (?, 'v', 's', 'p', ?, ?)")
                .bind(local_days_ago_key(now, back)).bind(n).bind(n).execute(&pool).await.unwrap();
        }
        let prev = previous_totals(&pool, StatsRange::Last7Days, now).await.unwrap().unwrap();
        assert_eq!(prev.total_requests, 5, "前 7 天 = 往前第 7..=13 天");
        assert!(previous_totals(&pool, StatsRange::AllTime, now).await.unwrap().is_none());
        insert_req(&pool, "y", local_days_ago_start_ms(now, 1) + 1, "s", "success", Some(100), (0, 0), None, None).await;
        let y = previous_totals(&pool, StatsRange::Today, now).await.unwrap().unwrap();
        assert_eq!((y.total_requests, y.tool_use_request_count), (1, 1));
    }
}

/// 统计页的费用卡片 (设计 §5): 按当前价格表从 `receipt_stats_daily` 现算。
#[derive(Debug, Serialize)]
pub struct CostSummaryDto {
    /// 有没有启用的计价插件; false 时界面不显示任何费用, 只给一行「安装计价插件后显示费用」
    pub has_pricing: bool,
    pub totals: crate::plugins::pricing::CostTotals,
    /// 按订阅, 按真实花费 + 等价费用 (各币种合计成 micros 的绝对值) 降序
    pub by_subscription: Vec<SubscriptionCostDto>,
    /// 没有匹配价格的 (厂商, 模型), 按请求数降序
    pub unpriced: Vec<UnpricedModelDto>,
    pub priced_requests: i64,
    pub total_requests: i64,
    /// 参与计价的插件 (按优先级), 界面写明价格来自哪里
    pub sources: Vec<crate::plugins::pricing::PricingSource>,
}

#[derive(Debug, Serialize)]
pub struct SubscriptionCostDto {
    pub subscription_id: String,
    /// None = 订阅已删除
    pub display_name: Option<String>,
    pub totals: crate::plugins::pricing::CostTotals,
}

#[derive(Debug, Serialize)]
pub struct UnpricedModelDto {
    pub provider_id: String,
    pub model: String,
    pub requests: i64,
}

#[tauri::command]
pub async fn get_cost_summary(state: State<'_, AppState>, range: StatsRange) -> AppResult<CostSummaryDto> {
    use crate::plugins::pricing::{cost_of, subscription_contexts, CostTotals, Tokens};
    use std::collections::HashMap;

    let pricing = state.pricing();
    let mut out = CostSummaryDto {
        has_pricing: !pricing.is_empty(),
        totals: CostTotals::default(),
        by_subscription: Vec::new(),
        unpriced: Vec::new(),
        priced_requests: 0,
        total_requests: 0,
        sources: pricing.sources(),
    };
    if pricing.is_empty() {
        return Ok(out);
    }
    let rows = sqlx::query(
        "SELECT subscription_id, provider_id, real_model_name,
                SUM(request_count) AS requests,
                SUM(input_tokens) AS input_tokens, SUM(output_tokens) AS output_tokens,
                SUM(cache_creation_tokens) AS cache_creation_tokens, SUM(cache_read_tokens) AS cache_read_tokens
         FROM receipt_stats_daily
         WHERE day >= ?
         GROUP BY subscription_id, provider_id, real_model_name",
    )
    .bind(range.since_day())
    .fetch_all(&state.db)
    .await?;

    let contexts = subscription_contexts(&state).await;
    let names: HashMap<String, String> = {
        let subs = state.subscriptions.read().await;
        let mut m = HashMap::with_capacity(subs.len());
        for (id, rt) in subs.iter() {
            m.insert(id.to_string(), rt.read().await.row.display_name.clone());
        }
        m
    };
    let mut by_sub: HashMap<String, CostTotals> = HashMap::new();
    let mut unpriced: HashMap<(String, String), i64> = HashMap::new();
    for r in rows {
        let sub: String = r.try_get("subscription_id")?;
        let provider: String = r.try_get("provider_id")?;
        let model: String = r.try_get("real_model_name")?;
        let requests: i64 = r.try_get("requests")?;
        let n = |k: &str| -> AppResult<u64> { Ok(r.try_get::<i64, _>(k)?.max(0) as u64) };
        let tokens = Tokens {
            input_raw: n("input_tokens")?,
            output: n("output_tokens")?,
            cache_read: n("cache_read_tokens")?,
            cache_write: n("cache_creation_tokens")?,
        };
        out.total_requests += requests;
        let (auth_type, billing) = contexts.get(&sub).copied().unzip();
        match cost_of(&pricing, &provider, &model, auth_type, billing.unwrap_or_default(), tokens) {
            Some(line) => {
                out.priced_requests += requests;
                out.totals.add(&line);
                by_sub.entry(sub).or_default().add(&line);
            }
            None => *unpriced.entry((provider, model)).or_default() += requests,
        }
    }
    let magnitude = |t: &CostTotals| t.usd_micros + t.cny_micros + t.equiv_usd_micros + t.equiv_cny_micros;
    out.by_subscription = by_sub
        .into_iter()
        .map(|(id, totals)| SubscriptionCostDto { display_name: names.get(&id).cloned(), subscription_id: id, totals })
        .collect();
    out.by_subscription.sort_by(|a, b| magnitude(&b.totals).cmp(&magnitude(&a.totals)));
    out.unpriced = unpriced
        .into_iter()
        .map(|((provider_id, model), requests)| UnpricedModelDto { provider_id, model, requests })
        .collect();
    out.unpriced.sort_by(|a, b| b.requests.cmp(&a.requests).then_with(|| a.model.cmp(&b.model)));
    Ok(out)
}
