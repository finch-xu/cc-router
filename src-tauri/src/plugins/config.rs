//! 系统插件的配置 (`plugin_configs` 表): 启用状态、先后顺序、用户参数 (设计 §3.2)。
//!
//! 没有行 = 按类型默认值 ([`default_enabled`])。安装时不写行, 用户第一次切换才写; 卸载时删除。
//! 主题 / 小票不读这张表 (它们是「选中才生效」, 没有启用的概念)。

use std::collections::HashMap;

use chrono::Utc;
use sqlx::{Row, SqlitePool};

use crate::error::AppResult;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginConfig {
    pub enabled: bool,
    pub priority: i64,
    /// JSON 文本 (P1 恒为 `{}`)
    pub params: String,
}

/// 装上后、用户还没切换过时是否生效。厂商 / 计价是被动的 (只是多一个选项 / 只影响显示), 默认生效;
/// 规则 / 通知会改请求或往外发, 默认停用 (P2 / P3 加进来时在这里登记)。
pub fn default_enabled(plugin_type: &str) -> bool {
    matches!(plugin_type, "provider" | "pricing")
}

pub async fn load_all(pool: &SqlitePool) -> AppResult<HashMap<String, PluginConfig>> {
    let rows = sqlx::query("SELECT plugin_id, enabled, priority, params FROM plugin_configs").fetch_all(pool).await?;
    let mut out = HashMap::new();
    for r in rows {
        out.insert(
            r.try_get::<String, _>("plugin_id")?,
            PluginConfig {
                enabled: r.try_get::<i64, _>("enabled")? != 0,
                priority: r.try_get("priority")?,
                params: r.try_get("params")?,
            },
        );
    }
    Ok(out)
}

/// 生效状态: 有配置行用行里的, 否则用类型默认值。
pub fn is_enabled(configs: &HashMap<String, PluginConfig>, plugin_id: &str, plugin_type: &str) -> bool {
    configs.get(plugin_id).map(|c| c.enabled).unwrap_or_else(|| default_enabled(plugin_type))
}

pub async fn set_enabled(pool: &SqlitePool, plugin_id: &str, enabled: bool) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO plugin_configs (plugin_id, enabled, updated_at) VALUES (?, ?, ?)
         ON CONFLICT(plugin_id) DO UPDATE SET enabled = excluded.enabled, updated_at = excluded.updated_at",
    )
    .bind(plugin_id)
    .bind(enabled as i64)
    .bind(Utc::now().timestamp_millis())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete(pool: &SqlitePool, plugin_id: &str) -> AppResult<()> {
    sqlx::query("DELETE FROM plugin_configs WHERE plugin_id = ?").bind(plugin_id).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(include_str!("../../migrations/032_add_plugin_configs.sql")).execute(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn defaults_then_explicit_toggle_then_delete() {
        let pool = pool().await;
        let c = load_all(&pool).await.unwrap();
        assert!(is_enabled(&c, "relay", "provider"), "厂商插件默认生效");
        assert!(!is_enabled(&c, "x", "rules-to-come"), "未登记类型默认停用");

        set_enabled(&pool, "relay", false).await.unwrap();
        let c = load_all(&pool).await.unwrap();
        assert!(!is_enabled(&c, "relay", "provider"));
        set_enabled(&pool, "relay", true).await.unwrap();
        assert!(is_enabled(&load_all(&pool).await.unwrap(), "relay", "provider"));

        delete(&pool, "relay").await.unwrap();
        assert!(load_all(&pool).await.unwrap().is_empty());
    }
}
