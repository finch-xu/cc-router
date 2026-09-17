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
}
