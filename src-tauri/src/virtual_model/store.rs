use std::collections::HashMap;

use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::virtual_model::model::{RoutingMode, VirtualModelConfig, VirtualModelName};

pub async fn load_all(
    pool: &SqlitePool,
) -> AppResult<HashMap<VirtualModelName, VirtualModelConfig>> {
    let mut out = HashMap::new();

    for name in VirtualModelName::all() {
        let mode_str: Option<String> = sqlx::query_scalar(
            "SELECT mode FROM virtual_model_config WHERE virtual_model_name = ?",
        )
        .bind(name.as_str())
        .fetch_optional(pool)
        .await?;
        let mode = match mode_str.as_deref() {
            Some("round_robin") => RoutingMode::RoundRobin,
            _ => RoutingMode::Sequential,
        };

        let rows = sqlx::query(
            "SELECT subscription_id FROM virtual_model_bindings
             WHERE virtual_model_name = ? ORDER BY position ASC",
        )
        .bind(name.as_str())
        .fetch_all(pool)
        .await?;

        let mut subscription_ids = Vec::with_capacity(rows.len());
        for row in rows {
            let id_str: String = row.try_get("subscription_id")?;
            let id = Uuid::parse_str(&id_str)
                .map_err(|e| AppError::internal(format!("invalid uuid in binding: {e}")))?;
            subscription_ids.push(id);
        }

        out.insert(
            name,
            VirtualModelConfig {
                name,
                mode,
                subscription_ids,
                last_used_index: 0,
            },
        );
    }

    Ok(out)
}

pub async fn save_mode(
    pool: &SqlitePool,
    name: VirtualModelName,
    mode: RoutingMode,
) -> AppResult<()> {
    let mode_str = match mode {
        RoutingMode::Sequential => "sequential",
        RoutingMode::RoundRobin => "round_robin",
    };
    sqlx::query(
        "INSERT INTO virtual_model_config (virtual_model_name, mode)
         VALUES (?, ?)
         ON CONFLICT(virtual_model_name) DO UPDATE SET mode = excluded.mode",
    )
    .bind(name.as_str())
    .bind(mode_str)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn save_bindings(
    pool: &SqlitePool,
    name: VirtualModelName,
    subscription_ids: &[Uuid],
) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM virtual_model_bindings WHERE virtual_model_name = ?")
        .bind(name.as_str())
        .execute(&mut *tx)
        .await?;
    for (position, sub_id) in subscription_ids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO virtual_model_bindings (virtual_model_name, position, subscription_id)
             VALUES (?, ?, ?)",
        )
        .bind(name.as_str())
        .bind(position as i64)
        .bind(sub_id.to_string())
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}
