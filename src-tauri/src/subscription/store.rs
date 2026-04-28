use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use sqlx::{Row, SqlitePool};
use tokio::sync::RwLock;
use uuid::Uuid;

use std::str::FromStr;

use crate::error::{AppError, AppResult};
use crate::provider::model::{AuthHeaderFormat, ModelDiscovery};
use crate::subscription::model::{
    ModelCache, ModelInfo, ModelSlots, SubscriptionRow, SubscriptionRuntime,
};

/// 启动时从 DB 加载全部订阅，并初始化运行时状态。
pub async fn load_runtime(
    pool: &SqlitePool,
) -> AppResult<HashMap<Uuid, Arc<RwLock<SubscriptionRuntime>>>> {
    let rows = sqlx::query(
        "SELECT id, provider_id, endpoint_id, display_name, api_key,
                model_slot_opus, model_slot_sonnet, model_slot_haiku,
                enabled, is_auth_failed, last_error_message,
                created_at, updated_at,
                base_url, messages_path, auth_header_name, auth_header_format,
                required_headers, forward_headers, model_discovery,
                provider_display_name, provider_icon, is_user_defined,
                supports_thinking_blocks, thinking_block_field_name
         FROM subscriptions",
    )
    .fetch_all(pool)
    .await?;

    let mut out = HashMap::new();
    for row in rows {
        let sub = row_to_row(&row)?;
        let cache = load_model_cache(pool, &sub.id, &sub.endpoint_id).await?;
        let mut rt = SubscriptionRuntime::from_row(sub);
        rt.model_cache = cache;
        out.insert(rt.row.id, Arc::new(RwLock::new(rt)));
    }
    Ok(out)
}

fn row_to_row(row: &sqlx::sqlite::SqliteRow) -> AppResult<SubscriptionRow> {
    let id_str: String = row.try_get("id")?;
    let id = Uuid::parse_str(&id_str)
        .map_err(|e| AppError::internal(format!("无效 uuid: {e}")))?;
    let auth_fmt_str: String = row.try_get("auth_header_format")?;
    let required_json: String = row.try_get("required_headers")?;
    let forward_json: String = row.try_get("forward_headers")?;
    let discovery_json: String = row.try_get("model_discovery")?;
    let required_headers: BTreeMap<String, String> = serde_json::from_str(&required_json)
        .map_err(|e| AppError::internal(format!("required_headers JSON 解析失败: {e}")))?;
    let forward_headers: Vec<String> = serde_json::from_str(&forward_json)
        .map_err(|e| AppError::internal(format!("forward_headers JSON 解析失败: {e}")))?;
    // ModelDiscovery 各字段均带 #[serde(default)], 空对象 "{}" 自然得到 default。
    let model_discovery: ModelDiscovery = serde_json::from_str(&discovery_json)
        .map_err(|e| AppError::internal(format!("model_discovery JSON 解析失败: {e}")))?;
    Ok(SubscriptionRow {
        id,
        provider_id: row.try_get("provider_id")?,
        endpoint_id: row.try_get("endpoint_id")?,
        display_name: row.try_get("display_name")?,
        api_key: row.try_get("api_key")?,
        model_slots: ModelSlots {
            opus: row.try_get("model_slot_opus")?,
            sonnet: row.try_get("model_slot_sonnet")?,
            haiku: row.try_get("model_slot_haiku")?,
        },
        enabled: {
            let v: i64 = row.try_get("enabled")?;
            v != 0
        },
        is_auth_failed: {
            let v: i64 = row.try_get("is_auth_failed")?;
            v != 0
        },
        last_error_message: row.try_get("last_error_message")?,
        created_at: ms_to_dt(row.try_get::<i64, _>("created_at")?),
        updated_at: ms_to_dt(row.try_get::<i64, _>("updated_at")?),
        base_url: row.try_get("base_url")?,
        messages_path: row.try_get("messages_path")?,
        auth_header_name: row.try_get("auth_header_name")?,
        auth_header_format: AuthHeaderFormat::from_str(&auth_fmt_str)
            .map_err(AppError::internal)?,
        required_headers,
        forward_headers,
        model_discovery,
        provider_display_name: row.try_get("provider_display_name")?,
        provider_icon: row.try_get("provider_icon")?,
        is_user_defined: {
            let v: i64 = row.try_get("is_user_defined")?;
            v != 0
        },
        supports_thinking_blocks: {
            let v: i64 = row.try_get("supports_thinking_blocks")?;
            v != 0
        },
        thinking_block_field_name: row.try_get("thinking_block_field_name")?,
    })
}

fn ms_to_dt(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms).single().unwrap_or_else(Utc::now)
}

pub async fn insert(pool: &SqlitePool, sub: &SubscriptionRow) -> AppResult<()> {
    let required_json = serde_json::to_string(&sub.required_headers)?;
    let forward_json = serde_json::to_string(&sub.forward_headers)?;
    let discovery_json = serde_json::to_string(&sub.model_discovery)?;
    sqlx::query(
        "INSERT INTO subscriptions (id, provider_id, endpoint_id, display_name, api_key,
            model_slot_opus, model_slot_sonnet, model_slot_haiku,
            enabled, is_auth_failed, last_error_message, created_at, updated_at,
            base_url, messages_path, auth_header_name, auth_header_format,
            required_headers, forward_headers, model_discovery,
            provider_display_name, provider_icon, is_user_defined,
            supports_thinking_blocks, thinking_block_field_name)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                 ?, ?, ?, ?,
                 ?, ?, ?,
                 ?, ?, ?,
                 ?, ?)",
    )
    .bind(sub.id.to_string())
    .bind(&sub.provider_id)
    .bind(&sub.endpoint_id)
    .bind(&sub.display_name)
    .bind(&sub.api_key)
    .bind(&sub.model_slots.opus)
    .bind(&sub.model_slots.sonnet)
    .bind(&sub.model_slots.haiku)
    .bind(sub.enabled as i64)
    .bind(sub.is_auth_failed as i64)
    .bind(&sub.last_error_message)
    .bind(sub.created_at.timestamp_millis())
    .bind(sub.updated_at.timestamp_millis())
    .bind(&sub.base_url)
    .bind(&sub.messages_path)
    .bind(&sub.auth_header_name)
    .bind(sub.auth_header_format.as_str())
    .bind(required_json)
    .bind(forward_json)
    .bind(discovery_json)
    .bind(&sub.provider_display_name)
    .bind(&sub.provider_icon)
    .bind(sub.is_user_defined as i64)
    .bind(sub.supports_thinking_blocks as i64)
    .bind(&sub.thinking_block_field_name)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_api_key(pool: &SqlitePool, id: &Uuid, new_key: &str) -> AppResult<()> {
    sqlx::query(
        "UPDATE subscriptions SET api_key = ?, is_auth_failed = 0, last_error_message = NULL, updated_at = ? WHERE id = ?",
    )
    .bind(new_key)
    .bind(Utc::now().timestamp_millis())
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_row(pool: &SqlitePool, sub: &SubscriptionRow) -> AppResult<()> {
    let required_json = serde_json::to_string(&sub.required_headers)?;
    let forward_json = serde_json::to_string(&sub.forward_headers)?;
    let discovery_json = serde_json::to_string(&sub.model_discovery)?;
    sqlx::query(
        "UPDATE subscriptions SET
            endpoint_id = ?, display_name = ?,
            model_slot_opus = ?, model_slot_sonnet = ?, model_slot_haiku = ?,
            enabled = ?, is_auth_failed = ?, last_error_message = ?, updated_at = ?,
            base_url = ?, messages_path = ?, auth_header_name = ?, auth_header_format = ?,
            required_headers = ?, forward_headers = ?, model_discovery = ?,
            provider_display_name = ?, provider_icon = ?, is_user_defined = ?,
            supports_thinking_blocks = ?, thinking_block_field_name = ?
         WHERE id = ?",
    )
    .bind(&sub.endpoint_id)
    .bind(&sub.display_name)
    .bind(&sub.model_slots.opus)
    .bind(&sub.model_slots.sonnet)
    .bind(&sub.model_slots.haiku)
    .bind(sub.enabled as i64)
    .bind(sub.is_auth_failed as i64)
    .bind(&sub.last_error_message)
    .bind(sub.updated_at.timestamp_millis())
    .bind(&sub.base_url)
    .bind(&sub.messages_path)
    .bind(&sub.auth_header_name)
    .bind(sub.auth_header_format.as_str())
    .bind(required_json)
    .bind(forward_json)
    .bind(discovery_json)
    .bind(&sub.provider_display_name)
    .bind(&sub.provider_icon)
    .bind(sub.is_user_defined as i64)
    .bind(sub.supports_thinking_blocks as i64)
    .bind(&sub.thinking_block_field_name)
    .bind(sub.id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_enabled(pool: &SqlitePool, id: &Uuid, enabled: bool) -> AppResult<()> {
    sqlx::query(
        "UPDATE subscriptions SET enabled = ?, updated_at = ? WHERE id = ?",
    )
    .bind(enabled as i64)
    .bind(Utc::now().timestamp_millis())
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_auth_failed(
    pool: &SqlitePool,
    id: &Uuid,
    is_auth_failed: bool,
    last_error: Option<&str>,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE subscriptions SET is_auth_failed = ?, last_error_message = ?, updated_at = ? WHERE id = ?",
    )
    .bind(is_auth_failed as i64)
    .bind(last_error)
    .bind(Utc::now().timestamp_millis())
    .bind(id.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete(pool: &SqlitePool, id: &Uuid) -> AppResult<()> {
    sqlx::query("DELETE FROM subscriptions WHERE id = ?")
        .bind(id.to_string())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM model_list_cache WHERE subscription_id = ?")
        .bind(id.to_string())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM virtual_model_bindings WHERE subscription_id = ?")
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn load_model_cache(
    pool: &SqlitePool,
    subscription_id: &Uuid,
    endpoint_id: &str,
) -> AppResult<Option<ModelCache>> {
    let row = sqlx::query(
        "SELECT fetched_at, models_json FROM model_list_cache
         WHERE subscription_id = ? AND endpoint_id = ?",
    )
    .bind(subscription_id.to_string())
    .bind(endpoint_id)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else { return Ok(None) };
    let fetched_at_ms: i64 = row.try_get("fetched_at")?;
    let json: String = row.try_get("models_json")?;
    let models: Vec<ModelInfo> = serde_json::from_str(&json)?;
    Ok(Some(ModelCache {
        fetched_at: ms_to_dt(fetched_at_ms),
        models,
    }))
}

pub async fn save_model_cache(
    pool: &SqlitePool,
    subscription_id: &Uuid,
    endpoint_id: &str,
    cache: &ModelCache,
) -> AppResult<()> {
    let json = serde_json::to_string(&cache.models)?;
    sqlx::query(
        "INSERT INTO model_list_cache (subscription_id, endpoint_id, fetched_at, models_json)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(subscription_id, endpoint_id) DO UPDATE SET
           fetched_at = excluded.fetched_at,
           models_json = excluded.models_json",
    )
    .bind(subscription_id.to_string())
    .bind(endpoint_id)
    .bind(cache.fetched_at.timestamp_millis())
    .bind(json)
    .execute(pool)
    .await?;
    Ok(())
}
