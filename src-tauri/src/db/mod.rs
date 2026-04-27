pub mod paths;

use std::path::Path;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{ConnectOptions, SqlitePool};
use tokio::fs;
use tracing::info;

use crate::error::AppResult;

/// 按版本号顺序的 migration 表。新增 schema 变更时,在末尾追加 (next_version, include_str!(...))。
/// 启动时按版本号顺序应用未跑过的 migration; 已跑过的 (在 `_schema_version` 表里) 跳过。
const MIGRATIONS: &[(u32, &str)] = &[
    (1, include_str!("../../migrations/001_init.sql")),
    (
        2,
        include_str!("../../migrations/002_add_supports_thinking_blocks.sql"),
    ),
];

pub async fn init_pool(db_path: &Path) -> AppResult<SqlitePool> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).await?;
    }

    let mut opts = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal);

    opts = opts.log_statements(tracing::log::LevelFilter::Trace);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await?;

    Ok(pool)
}

/// 应用 schema migrations。
///
/// 流程:
/// 1. 确保 `_schema_version` 表存在 (始终幂等)
/// 2. 检测老 DB (subscriptions 已存在但 `_schema_version` 为空) → 标定为 v=1 baseline
/// 3. 读取当前版本号
/// 4. 按顺序应用版本号 > 当前版本的 migration, 每跑完一项写一行版本记录
/// 5. seed 默认数据 (始终幂等)
pub async fn run_migrations(pool: &SqlitePool, _resource_dir: &Path) -> AppResult<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _schema_version (
            version INTEGER PRIMARY KEY,
            applied_at INTEGER NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    let has_subscriptions: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='subscriptions'",
    )
    .fetch_one(pool)
    .await?;
    let already_versioned: (i64,) = sqlx::query_as("SELECT count(*) FROM _schema_version")
        .fetch_one(pool)
        .await?;

    if has_subscriptions.0 > 0 && already_versioned.0 == 0 {
        // 老 DB (1.2.0 及之前): subscriptions 已建好但还没有版本号表。
        // 标定为 v=1, 后面会从 v=2 开始应用增量 migration。
        info!("legacy v1 schema detected, baselining at v=1");
        sqlx::query("INSERT OR IGNORE INTO _schema_version (version, applied_at) VALUES (?, ?)")
            .bind(1_i64)
            .bind(chrono::Utc::now().timestamp_millis())
            .execute(pool)
            .await?;
    }

    let current: (Option<i64>,) = sqlx::query_as("SELECT MAX(version) FROM _schema_version")
        .fetch_one(pool)
        .await?;
    let current_version = current.0.unwrap_or(0) as u32;

    for (v, sql) in MIGRATIONS {
        if *v <= current_version {
            continue;
        }
        info!(version = v, "applying migration");
        for stmt in split_sql_statements(sql) {
            sqlx::query(&stmt).execute(pool).await?;
        }
        sqlx::query("INSERT OR IGNORE INTO _schema_version (version, applied_at) VALUES (?, ?)")
            .bind(*v as i64)
            .bind(chrono::Utc::now().timestamp_millis())
            .execute(pool)
            .await?;
    }

    seed_virtual_model_config(pool).await?;
    seed_onboarding(pool).await?;
    Ok(())
}

/// 按 `;` 切分 SQL 语句，但会正确跳过：
/// - 单引号字符串里的 `;`
/// - 行注释 `-- ...` 里的 `;`
/// - 块注释 `/* ... */` 里的 `;`
fn split_sql_statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut chars = sql.chars().peekable();

    while let Some(c) = chars.next() {
        if in_line_comment {
            current.push(c);
            if c == '\n' {
                in_line_comment = false;
            }
            continue;
        }
        if in_block_comment {
            current.push(c);
            if c == '*' && chars.peek() == Some(&'/') {
                current.push(chars.next().unwrap());
                in_block_comment = false;
            }
            continue;
        }
        if in_string {
            current.push(c);
            if c == '\'' {
                // SQLite 用 '' 转义单引号
                if chars.peek() == Some(&'\'') {
                    current.push(chars.next().unwrap());
                } else {
                    in_string = false;
                }
            }
            continue;
        }
        match c {
            '-' if chars.peek() == Some(&'-') => {
                in_line_comment = true;
                current.push(c);
            }
            '/' if chars.peek() == Some(&'*') => {
                in_block_comment = true;
                current.push(c);
                current.push(chars.next().unwrap());
            }
            '\'' => {
                in_string = true;
                current.push(c);
            }
            ';' => {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    out.push(trimmed.to_string());
                }
                current.clear();
            }
            _ => current.push(c),
        }
    }
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        out.push(trimmed.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Row;
    use sqlx::sqlite::SqlitePoolOptions;

    #[test]
    fn splits_basic_statements() {
        let s = "CREATE TABLE a (id TEXT); CREATE TABLE b (id TEXT);";
        assert_eq!(split_sql_statements(s).len(), 2);
    }

    #[test]
    fn ignores_semicolon_in_line_comment() {
        let s = "CREATE TABLE a (\n  id TEXT  -- foo; bar\n);";
        let stmts = split_sql_statements(s);
        assert_eq!(stmts.len(), 1);
        assert!(stmts[0].contains("CREATE TABLE"));
    }

    #[test]
    fn ignores_semicolon_in_block_comment() {
        let s = "CREATE TABLE a (/* ; */ id TEXT);";
        assert_eq!(split_sql_statements(s).len(), 1);
    }

    #[test]
    fn ignores_semicolon_in_string() {
        let s = "INSERT INTO t VALUES ('a;b'); INSERT INTO t VALUES ('c');";
        assert_eq!(split_sql_statements(s).len(), 2);
    }

    async fn in_memory_pool() -> SqlitePool {
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db")
    }

    async fn applied_versions(pool: &SqlitePool) -> Vec<i64> {
        let rows = sqlx::query("SELECT version FROM _schema_version ORDER BY version")
            .fetch_all(pool)
            .await
            .expect("select versions");
        rows.iter()
            .map(|r| r.try_get::<i64, _>("version").unwrap())
            .collect()
    }

    async fn has_column(pool: &SqlitePool, table: &str, column: &str) -> bool {
        let rows = sqlx::query(&format!("PRAGMA table_info({})", table))
            .fetch_all(pool)
            .await
            .unwrap();
        rows.iter()
            .any(|r| r.try_get::<String, _>("name").map(|n| n == column).unwrap_or(false))
    }

    #[tokio::test]
    async fn fresh_db_applies_all_migrations() {
        let pool = in_memory_pool().await;
        let dir = std::path::PathBuf::from(".");
        run_migrations(&pool, &dir).await.expect("migrate fresh");

        let versions = applied_versions(&pool).await;
        assert_eq!(versions, vec![1, 2]);
        assert!(has_column(&pool, "subscriptions", "supports_thinking_blocks").await);
    }

    #[tokio::test]
    async fn legacy_v1_db_baselines_then_applies_v2() {
        let pool = in_memory_pool().await;
        // 模拟 v1 老 DB: 只跑 001, 不写 _schema_version
        for stmt in split_sql_statements(MIGRATIONS[0].1) {
            sqlx::query(&stmt).execute(&pool).await.unwrap();
        }
        // 此时 subscriptions 存在, _schema_version 不存在

        let dir = std::path::PathBuf::from(".");
        run_migrations(&pool, &dir).await.expect("migrate legacy");

        let versions = applied_versions(&pool).await;
        assert_eq!(versions, vec![1, 2]); // baseline 写 v=1, 然后跑 v=2
        assert!(has_column(&pool, "subscriptions", "supports_thinking_blocks").await);
    }

    #[tokio::test]
    async fn rerunning_migrations_is_idempotent() {
        let pool = in_memory_pool().await;
        let dir = std::path::PathBuf::from(".");
        run_migrations(&pool, &dir).await.expect("first run");
        run_migrations(&pool, &dir).await.expect("second run");
        run_migrations(&pool, &dir).await.expect("third run");

        let versions = applied_versions(&pool).await;
        assert_eq!(versions, vec![1, 2]); // 没有重复写
    }
}

async fn seed_virtual_model_config(pool: &SqlitePool) -> AppResult<()> {
    for name in ["model-opus", "model-sonnet", "model-haiku", "model-fallback"] {
        sqlx::query(
            "INSERT OR IGNORE INTO virtual_model_config (virtual_model_name, mode) VALUES (?, 'sequential')",
        )
        .bind(name)
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn seed_onboarding(pool: &SqlitePool) -> AppResult<()> {
    sqlx::query("INSERT OR IGNORE INTO onboarding (id, completed) VALUES (1, 0)")
        .execute(pool)
        .await?;
    Ok(())
}
