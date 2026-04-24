//! 请求日志查询 command。简单 offset/limit 分页，按 timestamp 倒序。

use serde::Serialize;
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

#[tauri::command]
pub async fn list_requests(
    state: State<'_, AppState>,
    page: u32,
    page_size: u32,
) -> AppResult<ListRequestsResult> {
    let page = page.max(1);
    let page_size = page_size.clamp(1, 200);
    let offset = (page - 1) as i64 * page_size as i64;

    let total: i64 = sqlx::query("SELECT COUNT(*) AS c FROM requests")
        .fetch_one(&state.db)
        .await?
        .try_get("c")?;

    let rows = sqlx::query(
        "SELECT id, timestamp, virtual_model_name, subscription_id, provider_id, endpoint_id,
                real_model_name, is_streaming, status, http_status, total_latency_ms,
                upstream_input_tokens, upstream_output_tokens,
                upstream_cache_creation, upstream_cache_read, error_message
         FROM requests
         ORDER BY timestamp DESC
         LIMIT ? OFFSET ?",
    )
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
