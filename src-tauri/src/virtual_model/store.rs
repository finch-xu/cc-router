use std::collections::HashMap;

use sqlx::{Row, SqliteConnection, SqlitePool};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::virtual_model::model::{RoutingMode, VirtualModelConfig, VirtualModelName};

pub async fn load_all(
    pool: &SqlitePool,
) -> AppResult<HashMap<VirtualModelName, VirtualModelConfig>> {
    let mut out = HashMap::new();

    let names: Vec<String> = sqlx::query_scalar("SELECT virtual_model_name FROM virtual_model_config ORDER BY virtual_model_name")
        .fetch_all(pool).await?;
    for name in VirtualModelName::all().into_iter().chain(names.iter().filter_map(|name| VirtualModelName::custom(name))) {
        let mode_str: Option<String> = sqlx::query_scalar(
            "SELECT mode FROM virtual_model_config WHERE virtual_model_name = ?",
        )
        .bind(name.as_str())
        .fetch_optional(pool)
        .await?;
        let mode = match mode_str.as_deref() {
            Some("round_robin") => RoutingMode::RoundRobin,
            Some("sticky") => RoutingMode::Sticky,
            _ => RoutingMode::Sequential,
        };

        let rows = sqlx::query(
            "SELECT subscription_id, upstream_model_id FROM virtual_model_bindings
             WHERE virtual_model_name = ? ORDER BY position ASC",
        )
        .bind(name.as_str())
        .fetch_all(pool)
        .await?;

        let mut subscription_ids = Vec::with_capacity(rows.len());
        let mut upstream_models = HashMap::new();
        for row in rows {
            let id_str: String = row.try_get("subscription_id")?;
            let id = Uuid::parse_str(&id_str)
                .map_err(|e| AppError::internal(format!("invalid uuid in binding: {e}")))?;
            let upstream: Option<String> = row.try_get("upstream_model_id")?;
            if name.is_custom() {
                let Some(upstream) = upstream.filter(|value| !value.trim().is_empty()) else { continue };
                upstream_models.insert(id, upstream);
            }
            subscription_ids.push(id);
        }

        out.insert(
            name.clone(),
            VirtualModelConfig {
                name,
                mode,
                subscription_ids,
                upstream_models,
                last_used_index: 0,
            },
        );
    }

    Ok(out)
}

pub async fn save_mode_on(
    conn: &mut SqliteConnection,
    name: &VirtualModelName,
    mode: RoutingMode,
) -> AppResult<()> {
    let mode_str = match mode {
        RoutingMode::Sequential => "sequential",
        RoutingMode::RoundRobin => "round_robin",
        RoutingMode::Sticky => "sticky",
    };
    sqlx::query(
        "INSERT INTO virtual_model_config (virtual_model_name, mode)
         VALUES (?, ?)
         ON CONFLICT(virtual_model_name) DO UPDATE SET mode = excluded.mode",
    )
    .bind(name.as_str())
    .bind(mode_str)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub async fn save_bindings_on(
    conn: &mut SqliteConnection,
    name: &VirtualModelName,
    subscription_ids: &[Uuid],
) -> AppResult<()> {
    save_targets_on(conn, name, subscription_ids, &HashMap::new()).await
}

pub async fn save_targets_on(
    conn: &mut SqliteConnection,
    name: &VirtualModelName,
    subscription_ids: &[Uuid],
    upstream_models: &HashMap<Uuid, String>,
) -> AppResult<()> {
    sqlx::query("DELETE FROM virtual_model_bindings WHERE virtual_model_name = ?")
        .bind(name.as_str())
        .execute(&mut *conn)
        .await?;
    for (position, sub_id) in subscription_ids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO virtual_model_bindings (virtual_model_name, position, subscription_id, upstream_model_id)
             VALUES (?, ?, ?, ?)",
        )
        .bind(name.as_str())
        .bind(position as i64)
        .bind(sub_id.to_string())
        .bind(upstream_models.get(sub_id))
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod sticky_tests {
    use super::*;
    use crate::db::run_migrations;
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn sticky_mode_roundtrips_through_db() {
        let pool = SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        run_migrations(&pool).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        save_mode_on(&mut conn, &VirtualModelName::Sonnet, RoutingMode::Sticky).await.unwrap();
        drop(conn);
        let all = load_all(&pool).await.unwrap();
        assert_eq!(all[&VirtualModelName::Sonnet].mode, RoutingMode::Sticky);
        // serde 值
        assert_eq!(serde_json::to_string(&RoutingMode::Sticky).unwrap(), "\"sticky\"");
    }

    #[tokio::test]
    async fn on_variants_respect_the_callers_transaction() {
        let pool = SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        run_migrations(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        save_mode_on(&mut *tx, &VirtualModelName::Haiku, RoutingMode::RoundRobin).await.unwrap();
        save_bindings_on(&mut *tx, &VirtualModelName::Haiku, &[]).await.unwrap();
        tx.rollback().await.unwrap();
        let all = load_all(&pool).await.unwrap();
        assert_eq!(all[&VirtualModelName::Haiku].mode, RoutingMode::Sequential, "回滚后保持默认");
    }
}

#[cfg(test)]
mod custom_route_tests {
    use super::*;
    use crate::db::run_migrations;
    use crate::subscription::{model::SubscriptionRow, store as subscriptions};
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn same_subscription_routes_to_two_fixed_models_after_reload() {
        let pool = SqlitePoolOptions::new().max_connections(1)
            .connect("sqlite::memory:").await.unwrap();
        run_migrations(&pool).await.unwrap();
        let sub = SubscriptionRow::test_fixture("anthropic", "default");
        subscriptions::insert(&pool, &sub).await.unwrap();

        for (alias, upstream) in [("model-hy4", "hy4"), ("model-glm", "glm-5.3")] {
            let name = VirtualModelName::custom(alias).unwrap();
            let mut tx = pool.begin().await.unwrap();
            save_mode_on(&mut tx, &name, RoutingMode::Sequential).await.unwrap();
            save_targets_on(&mut tx, &name, &[sub.id], &HashMap::from([(sub.id, upstream.into())]))
                .await.unwrap();
            tx.commit().await.unwrap();
        }

        let configs = load_all(&pool).await.unwrap();
        for (client_name, upstream) in [
            ("anthropic/model-hy4", "hy4"),
            ("openai/model-glm", "glm-5.3"),
            ("model-glm", "glm-5.3"),
        ] {
            let config = VirtualModelConfig::resolve(&configs, client_name).unwrap();
            assert_eq!(config.subscription_ids, vec![sub.id]);
            assert_eq!(config.upstream_models[&sub.id], upstream);
            assert_eq!(config.name.four_slot(), None);
        }
        assert_eq!(
            VirtualModelConfig::resolve(&configs, "model-unknown").unwrap().name,
            VirtualModelName::Fallback
        );
        assert_eq!(
            VirtualModelConfig::resolve(&configs, "claude-opus-6").unwrap().name,
            VirtualModelName::Opus
        );

        subscriptions::delete(&pool, &sub.id).await.unwrap();
        let configs = load_all(&pool).await.unwrap();
        let model = VirtualModelConfig::resolve(&configs, "model-hy4").unwrap();
        assert!(model.name.is_custom());
        assert!(model.subscription_ids.is_empty());
    }

    #[test]
    fn custom_name_uses_a_string_and_cannot_replace_builtin() {
        let name = VirtualModelName::custom("model-hy4").unwrap();
        assert_eq!(serde_json::to_value(&name).unwrap(), "model-hy4");
        assert_eq!(serde_json::from_str::<VirtualModelName>(r#""model-hy4""#).unwrap(), name);
        for reserved in VirtualModelName::all() {
            assert!(VirtualModelName::custom(reserved.as_str()).is_none());
        }
        for invalid in ["model-", "model-Foo", "model-one--two", "anthropic/model-one"] {
            assert!(VirtualModelName::custom(invalid).is_none());
        }
    }
}
