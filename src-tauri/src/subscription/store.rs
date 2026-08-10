use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use sqlx::{Row, SqlitePool};
use tokio::sync::RwLock;
use tracing::warn;
use uuid::Uuid;

use std::str::FromStr;

use crate::error::{AppError, AppResult};
use crate::provider::model::{AuthHeaderFormat, AuthType, BalanceDiscovery, ModelDiscovery};
use crate::subscription::model::{
    BalanceSnapshot, ModelCache, ModelInfo, ModelSlots, OAuthMetadata, SlotEfforts,
    SubscriptionRow, SubscriptionRuntime,
};

/// 启动时从 DB 加载全部订阅，并初始化运行时状态。
///
/// 单条 row 解析失败时 warn+skip 而非 fail-fast。原因: 用户从含新 AuthType variant
/// (e.g. `gemini_api_key`) 的版本降级到旧 binary 时, 旧 `AuthType::from_str` 会失败,
/// 不应让整个订阅列表加载失败导致 app 不可用。
pub async fn load_runtime(
    pool: &SqlitePool,
) -> AppResult<HashMap<Uuid, Arc<RwLock<SubscriptionRuntime>>>> {
    let rows = sqlx::query(
        "SELECT id, provider_id, endpoint_id, display_name, api_key,
                model_slot_fable, model_slot_opus, model_slot_sonnet, model_slot_haiku,
                model_slot_fallback,
                enabled, is_auth_failed, last_error_message,
                created_at, updated_at,
                base_url, messages_path, auth_header_name, auth_header_format,
                required_headers, forward_headers, model_discovery, balance_discovery,
                provider_display_name, provider_icon, is_user_defined,
                auth_type, oauth_metadata, slot_efforts
         FROM subscriptions",
    )
    .fetch_all(pool)
    .await?;

    let mut out = HashMap::new();
    for row in rows {
        let sub = match row_to_row(&row) {
            Ok(s) => s,
            Err(e) => {
                let id_str: String = row.try_get("id").unwrap_or_default();
                let display_name: String = row.try_get("display_name").unwrap_or_default();
                warn!(
                    subscription_id = %id_str,
                    display_name = %display_name,
                    error = %e,
                    "订阅 row 解析失败, 跳过该订阅 (可能是从含新 AuthType 的版本降级所致)"
                );
                continue;
            }
        };
        let cache = load_model_cache(pool, &sub.id, &sub.endpoint_id).await?;
        let balance = load_balance_cache(pool, &sub.id).await?;
        let mut rt = SubscriptionRuntime::from_row(sub);
        rt.model_cache = cache;
        rt.balance_cache = balance;
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
    // balance_discovery 列在 migration 010 加, 老订阅或不支持的 provider 是 NULL.
    // 非空时含必填字段 (url/parser), 反序列化失败视为该订阅暂时不支持余额查询 (warn skip).
    let balance_discovery_json: Option<String> = row.try_get("balance_discovery")?;
    let balance_discovery: Option<BalanceDiscovery> = match balance_discovery_json {
        None => None,
        Some(json) => match serde_json::from_str::<BalanceDiscovery>(&json) {
            Ok(bd) => Some(bd),
            Err(e) => {
                warn!(error = %e, "balance_discovery JSON 解析失败, 余额查询将不可用");
                None
            }
        },
    };
    let auth_type_str: String = row.try_get("auth_type")?;
    let auth_type = AuthType::from_str(&auth_type_str).map_err(AppError::internal)?;
    let oauth_metadata_json: String = row.try_get("oauth_metadata")?;
    let oauth_metadata: OAuthMetadata = serde_json::from_str(&oauth_metadata_json)
        .map_err(|e| AppError::internal(format!("oauth_metadata JSON 解析失败: {e}")))?;
    // slot_efforts 列在 migration 013 加, DEFAULT '{}' (= 全 auto)。
    // 走 balance_discovery 式的宽容降级而不是 oauth_metadata 式的硬错误: 硬错误会让
    // load_runtime 跳过整条订阅 (订阅从 UI 消失), 而 effort 覆盖丢失只是回退到透传客户端值 —
    // 用整条订阅不可用去换一个可选配置字段的完整性不值得。SlotEfforts 全字段 optional,
    // 只有列里根本不是 JSON object 时才会走到这里 (基本只可能是手工改库)。
    let slot_efforts_json: String = row.try_get("slot_efforts")?;
    let slot_efforts: SlotEfforts = match serde_json::from_str::<SlotEfforts>(&slot_efforts_json) {
        Ok(se) => se,
        Err(e) => {
            warn!(
                error = %e,
                raw = %slot_efforts_json,
                "slot_efforts JSON 解析失败, 该订阅所有槽位回退到 auto (透传客户端 effort)"
            );
            SlotEfforts::default()
        }
    };
    Ok(SubscriptionRow {
        id,
        provider_id: row.try_get("provider_id")?,
        endpoint_id: row.try_get("endpoint_id")?,
        display_name: row.try_get("display_name")?,
        api_key: row.try_get("api_key")?,
        auth_type,
        oauth_metadata,
        model_slots: ModelSlots {
            fable: row.try_get("model_slot_fable")?,
            opus: row.try_get("model_slot_opus")?,
            sonnet: row.try_get("model_slot_sonnet")?,
            haiku: row.try_get("model_slot_haiku")?,
            fallback: row.try_get("model_slot_fallback")?,
        },
        slot_efforts,
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
        balance_discovery,
        provider_display_name: row.try_get("provider_display_name")?,
        provider_icon: row.try_get("provider_icon")?,
        is_user_defined: {
            let v: i64 = row.try_get("is_user_defined")?;
            v != 0
        },
    })
}

fn ms_to_dt(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms).single().unwrap_or_else(Utc::now)
}

fn opt_to_json<T: serde::Serialize>(v: Option<&T>) -> serde_json::Result<Option<String>> {
    v.map(serde_json::to_string).transpose()
}

pub async fn insert(pool: &SqlitePool, sub: &SubscriptionRow) -> AppResult<()> {
    let required_json = serde_json::to_string(&sub.required_headers)?;
    let forward_json = serde_json::to_string(&sub.forward_headers)?;
    let discovery_json = serde_json::to_string(&sub.model_discovery)?;
    let balance_discovery_json = opt_to_json(sub.balance_discovery.as_ref())?;
    let oauth_json = serde_json::to_string(&sub.oauth_metadata)?;
    let slot_efforts_json = serde_json::to_string(&sub.slot_efforts)?;
    sqlx::query(
        "INSERT INTO subscriptions (id, provider_id, endpoint_id, display_name, api_key,
            model_slot_fable, model_slot_opus, model_slot_sonnet, model_slot_haiku,
            model_slot_fallback,
            enabled, is_auth_failed, last_error_message, created_at, updated_at,
            base_url, messages_path, auth_header_name, auth_header_format,
            required_headers, forward_headers, model_discovery, balance_discovery,
            provider_display_name, provider_icon, is_user_defined,
            auth_type, oauth_metadata, slot_efforts)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                 ?, ?, ?, ?,
                 ?, ?, ?, ?,
                 ?, ?, ?,
                 ?, ?, ?)",
    )
    .bind(sub.id.to_string())
    .bind(&sub.provider_id)
    .bind(&sub.endpoint_id)
    .bind(&sub.display_name)
    .bind(&sub.api_key)
    .bind(&sub.model_slots.fable)
    .bind(&sub.model_slots.opus)
    .bind(&sub.model_slots.sonnet)
    .bind(&sub.model_slots.haiku)
    .bind(&sub.model_slots.fallback)
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
    .bind(balance_discovery_json)
    .bind(&sub.provider_display_name)
    .bind(&sub.provider_icon)
    .bind(sub.is_user_defined as i64)
    .bind(sub.auth_type.as_str())
    .bind(oauth_json)
    .bind(slot_efforts_json)
    .execute(pool)
    .await?;
    Ok(())
}

/// 更新订阅的 OAuth 元数据 (例如 refresh_token 被服务端旋转后落盘新值).
/// 同时清掉 is_auth_failed 与 last_error_message, 类比 update_api_key 的行为.
pub async fn update_oauth_metadata(
    pool: &SqlitePool,
    id: &Uuid,
    metadata: &OAuthMetadata,
) -> AppResult<()> {
    let oauth_json = serde_json::to_string(metadata)?;
    sqlx::query(
        "UPDATE subscriptions SET oauth_metadata = ?, is_auth_failed = 0, last_error_message = NULL, updated_at = ? WHERE id = ?",
    )
    .bind(oauth_json)
    .bind(Utc::now().timestamp_millis())
    .bind(id.to_string())
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
    let balance_discovery_json = opt_to_json(sub.balance_discovery.as_ref())?;
    let slot_efforts_json = serde_json::to_string(&sub.slot_efforts)?;
    sqlx::query(
        "UPDATE subscriptions SET
            endpoint_id = ?, display_name = ?,
            model_slot_fable = ?, model_slot_opus = ?, model_slot_sonnet = ?, model_slot_haiku = ?,
            model_slot_fallback = ?,
            slot_efforts = ?,
            enabled = ?, is_auth_failed = ?, last_error_message = ?, updated_at = ?,
            base_url = ?, messages_path = ?, auth_header_name = ?, auth_header_format = ?,
            required_headers = ?, forward_headers = ?, model_discovery = ?, balance_discovery = ?,
            provider_display_name = ?, provider_icon = ?, is_user_defined = ?
         WHERE id = ?",
    )
    .bind(&sub.endpoint_id)
    .bind(&sub.display_name)
    .bind(&sub.model_slots.fable)
    .bind(&sub.model_slots.opus)
    .bind(&sub.model_slots.sonnet)
    .bind(&sub.model_slots.haiku)
    .bind(&sub.model_slots.fallback)
    .bind(slot_efforts_json)
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
    .bind(balance_discovery_json)
    .bind(&sub.provider_display_name)
    .bind(&sub.provider_icon)
    .bind(sub.is_user_defined as i64)
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
    sqlx::query("DELETE FROM subscription_balance_cache WHERE subscription_id = ?")
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

/// 读取订阅余额缓存. 反序列化失败 (例如老 schema 不兼容) 视为无缓存, warn skip,
/// 避免单条坏数据导致整个订阅加载失败.
pub async fn load_balance_cache(
    pool: &SqlitePool,
    subscription_id: &Uuid,
) -> AppResult<Option<BalanceSnapshot>> {
    let row = sqlx::query(
        "SELECT payload_json FROM subscription_balance_cache WHERE subscription_id = ?",
    )
    .bind(subscription_id.to_string())
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else { return Ok(None) };
    let json: String = row.try_get("payload_json")?;
    match serde_json::from_str::<BalanceSnapshot>(&json) {
        Ok(snapshot) => Ok(Some(snapshot)),
        Err(e) => {
            warn!(error = %e, subscription_id = %subscription_id, "balance_cache 反序列化失败, 视为无缓存");
            Ok(None)
        }
    }
}

pub async fn save_balance_cache(
    pool: &SqlitePool,
    subscription_id: &Uuid,
    snapshot: &BalanceSnapshot,
) -> AppResult<()> {
    let json = serde_json::to_string(snapshot)?;
    sqlx::query(
        "INSERT INTO subscription_balance_cache (subscription_id, fetched_at, payload_json)
         VALUES (?, ?, ?)
         ON CONFLICT(subscription_id) DO UPDATE SET
           fetched_at = excluded.fetched_at,
           payload_json = excluded.payload_json",
    )
    .bind(subscription_id.to_string())
    .bind(snapshot.fetched_at.timestamp_millis())
    .bind(json)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription::model::SlotEfforts;
    use crate::virtual_model::SubscriptionSlot;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn migrated_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        crate::db::run_migrations(&pool, std::path::Path::new("."))
            .await
            .expect("run migrations");
        pool
    }

    /// insert → load_runtime → update_row → load_runtime 的完整往返。
    /// 主要防的是 SQL 列清单与 .bind() 顺序错位 —— 那种错误只会在运行时暴露,
    /// 且症状是"存进去的是别的字段的值", 很难从日志看出来。
    #[tokio::test]
    async fn slot_efforts_roundtrips_through_db() {
        let pool = migrated_pool().await;

        let mut row = SubscriptionRow::test_fixture("anthropic", "default");
        row.display_name = "往返测试".into();
        row.slot_efforts = SlotEfforts {
            opus: Some("max".into()),
            haiku: Some("low".into()),
            ..Default::default()
        };
        insert(&pool, &row).await.expect("insert");

        let loaded = load_runtime(&pool).await.expect("load");
        let rt = loaded.get(&row.id).expect("subscription present");
        let g = rt.read().await;
        assert_eq!(g.row.slot_efforts.get(SubscriptionSlot::Opus), Some("max"));
        assert_eq!(g.row.slot_efforts.get(SubscriptionSlot::Haiku), Some("low"));
        assert_eq!(g.row.slot_efforts.get(SubscriptionSlot::Sonnet), None);
        // 相邻字段没被 bind 顺序错位污染
        assert_eq!(g.row.display_name, "往返测试");
        assert_eq!(g.row.model_slots.sonnet, "b");
        drop(g);

        let mut updated = row.clone();
        updated.slot_efforts = SlotEfforts {
            sonnet: Some("xhigh".into()),
            ..Default::default()
        };
        // test_fixture 的 required_headers 是空 map, 顺带覆盖非空 map 经 update_row 的往返
        updated
            .required_headers
            .insert("X-DST".to_string(), "eastus2".to_string());
        update_row(&pool, &updated).await.expect("update");

        let loaded = load_runtime(&pool).await.expect("reload");
        let g = loaded.get(&row.id).unwrap().read().await;
        assert_eq!(g.row.slot_efforts.get(SubscriptionSlot::Sonnet), Some("xhigh"));
        assert_eq!(g.row.slot_efforts.get(SubscriptionSlot::Opus), None);
        assert_eq!(
            g.row.required_headers.get("X-DST").map(String::as_str),
            Some("eastus2")
        );
        assert_eq!(g.row.display_name, "往返测试");
    }

    /// 老订阅 (migration 013 的 DEFAULT '{}') 加载成全 auto。
    #[tokio::test]
    async fn legacy_row_defaults_to_all_auto() {
        let pool = migrated_pool().await;
        let row = SubscriptionRow::test_fixture("anthropic", "default");
        insert(&pool, &row).await.expect("insert");
        sqlx::query("UPDATE subscriptions SET slot_efforts = '{}'")
            .execute(&pool)
            .await
            .expect("reset to default");

        let loaded = load_runtime(&pool).await.expect("load");
        let g = loaded.get(&row.id).unwrap().read().await;
        assert_eq!(g.row.slot_efforts.get(SubscriptionSlot::Opus), None);
    }

    /// 坏 JSON 走宽容降级: 订阅仍然加载得出来 (全 auto), 而不是从列表里消失。
    #[tokio::test]
    async fn corrupt_slot_efforts_degrades_instead_of_dropping_row() {
        let pool = migrated_pool().await;
        let row = SubscriptionRow::test_fixture("anthropic", "default");
        insert(&pool, &row).await.expect("insert");
        sqlx::query("UPDATE subscriptions SET slot_efforts = 'not json at all'")
            .execute(&pool)
            .await
            .expect("corrupt");

        let loaded = load_runtime(&pool).await.expect("load");
        let rt = loaded.get(&row.id).expect("订阅不应被整条跳过");
        assert_eq!(rt.read().await.row.slot_efforts.get(SubscriptionSlot::Opus), None);
    }
}
