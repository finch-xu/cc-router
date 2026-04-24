//! 请求日志查询 command。简单 offset/limit 分页，按 timestamp 倒序。
//! 支持按 virtual_model_name / provider_id / status 筛选。

use serde::{Deserialize, Serialize};
use sqlx::Row;
use tauri::State;

use crate::error::AppResult;
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct RequestLogDto {
    pub id: String,
    pub timestamp: i64,
    pub virtual_model_name: String,
    pub subscription_id: String,
    pub provider_id: String,
    pub endpoint_id: String,
    pub real_model_name: String,
    pub is_streaming: bool,
    pub status: String,
    pub http_status: Option<i64>,
    pub total_latency_ms: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub error_message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ListRequestsResult {
    pub items: Vec<RequestLogDto>,
    pub total: i64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RequestLogFilters {
    pub virtual_model_name: Option<String>,
    pub provider_id: Option<String>,
    pub status: Option<String>,
}

#[tauri::command]
pub async fn list_requests(
    state: State<'_, AppState>,
    page: u32,
    page_size: u32,
    filters: Option<RequestLogFilters>,
) -> AppResult<ListRequestsResult> {
    let page = page.max(1);
    let page_size = page_size.clamp(1, 200);
    let offset = (page - 1) as i64 * page_size as i64;
    let filters = filters.unwrap_or_default();

    // 动态构建 WHERE 子句。列名是白名单字面量，值走 bind，无注入风险。
    let mut conditions: Vec<&'static str> = Vec::new();
    if filters.virtual_model_name.is_some() {
        conditions.push("virtual_model_name = ?");
    }
    if filters.provider_id.is_some() {
        conditions.push("provider_id = ?");
    }
    if filters.status.is_some() {
        conditions.push("status = ?");
    }
    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };

    let count_sql = format!("SELECT COUNT(*) AS c FROM requests{}", where_clause);
    let mut count_q = sqlx::query(&count_sql);
    if let Some(v) = &filters.virtual_model_name {
        count_q = count_q.bind(v);
    }
    if let Some(v) = &filters.provider_id {
        count_q = count_q.bind(v);
    }
    if let Some(v) = &filters.status {
        count_q = count_q.bind(v);
    }
    let total: i64 = count_q.fetch_one(&state.db).await?.try_get("c")?;

    let select_sql = format!(
        "SELECT id, timestamp, virtual_model_name, subscription_id, provider_id, endpoint_id,
                real_model_name, is_streaming, status, http_status, total_latency_ms,
                upstream_input_tokens, upstream_output_tokens,
                upstream_cache_creation, upstream_cache_read, error_message
         FROM requests{}
         ORDER BY timestamp DESC
         LIMIT ? OFFSET ?",
        where_clause
    );
    let mut select_q = sqlx::query(&select_sql);
    if let Some(v) = &filters.virtual_model_name {
        select_q = select_q.bind(v);
    }
    if let Some(v) = &filters.provider_id {
        select_q = select_q.bind(v);
    }
    if let Some(v) = &filters.status {
        select_q = select_q.bind(v);
    }
    let rows = select_q
        .bind(page_size as i64)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;

    let items = rows
        .into_iter()
        .map(|r| RequestLogDto {
            id: r.try_get("id").unwrap_or_default(),
            timestamp: r.try_get("timestamp").unwrap_or(0),
            virtual_model_name: r.try_get("virtual_model_name").unwrap_or_default(),
            subscription_id: r.try_get("subscription_id").unwrap_or_default(),
            provider_id: r.try_get("provider_id").unwrap_or_default(),
            endpoint_id: r.try_get("endpoint_id").unwrap_or_default(),
            real_model_name: r.try_get("real_model_name").unwrap_or_default(),
            is_streaming: r.try_get::<i64, _>("is_streaming").unwrap_or(0) != 0,
            status: r.try_get("status").unwrap_or_default(),
            http_status: r.try_get("http_status").ok(),
            total_latency_ms: r.try_get("total_latency_ms").ok(),
            input_tokens: r.try_get("upstream_input_tokens").ok(),
            output_tokens: r.try_get("upstream_output_tokens").ok(),
            cache_creation_tokens: r.try_get("upstream_cache_creation").ok(),
            cache_read_tokens: r.try_get("upstream_cache_read").ok(),
            error_message: r.try_get("error_message").ok(),
        })
        .collect();

    Ok(ListRequestsResult { items, total })
}
