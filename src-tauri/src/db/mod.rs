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
    (
        3,
        include_str!("../../migrations/003_add_events_and_diagnostics.sql"),
    ),
    (
        4,
        include_str!("../../migrations/004_add_thinking_block_field_name.sql"),
    ),
    (
        5,
        include_str!("../../migrations/005_drop_thinking_block_field_name.sql"),
    ),
    (
        6,
        include_str!("../../migrations/006_add_request_stats_daily.sql"),
    ),
    (
        7,
        include_str!("../../migrations/007_drop_supports_thinking_blocks.sql"),
    ),
    (
        8,
        include_str!("../../migrations/008_add_subscription_oauth.sql"),
    ),
    (
        9,
        include_str!("../../migrations/009_add_client_info.sql"),
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
/// 3. 检测 v5 half-finished 残留 (subscriptions_new 存在但 subscriptions 缺失) → 自动完成 RENAME
/// 4. 读取当前版本号
/// 5. 按顺序应用版本号 > 当前版本的 migration, 每跑完一项写一行版本记录
/// 6. seed 默认数据 (始终幂等)
///
/// migration 跑在单一 acquired connection 上, 让 v5 里的 `PRAGMA foreign_keys=OFF` 能贯穿整段 SQL —
/// 用 pool.execute 时连接池可能切换连接导致 PRAGMA 失效, 进而触发 ALTER TABLE RENAME 在 FK=ON
/// 下与 virtual_model_bindings 引用冲突, 形成"DROP 已 commit, RENAME 失败"的半成品状态。
pub async fn run_migrations(pool: &SqlitePool, _resource_dir: &Path) -> AppResult<()> {
    let mut conn = pool.acquire().await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _schema_version (
            version INTEGER PRIMARY KEY,
            applied_at INTEGER NOT NULL
        )",
    )
    .execute(&mut *conn)
    .await?;

    let has_subscriptions: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='subscriptions'",
    )
    .fetch_one(&mut *conn)
    .await?;
    let already_versioned: (i64,) = sqlx::query_as("SELECT count(*) FROM _schema_version")
        .fetch_one(&mut *conn)
        .await?;

    if has_subscriptions.0 > 0 && already_versioned.0 == 0 {
        // 老 DB (1.2.0 及之前): subscriptions 已建好但还没有版本号表。
        // 标定为 v=1, 后面会从 v=2 开始应用增量 migration。
        info!("legacy v1 schema detected, baselining at v=1");
        sqlx::query("INSERT OR IGNORE INTO _schema_version (version, applied_at) VALUES (?, ?)")
            .bind(1_i64)
            .bind(chrono::Utc::now().timestamp_millis())
            .execute(&mut *conn)
            .await?;
    }

    let has_subscriptions_new: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='subscriptions_new'",
    )
    .fetch_one(&mut *conn)
    .await?;
    if has_subscriptions_new.0 > 0 && has_subscriptions.0 == 0 {
        // v5 half-finished: 上次启动跑到 DROP TABLE subscriptions 已 commit,
        // 但 ALTER TABLE subscriptions_new RENAME TO subscriptions 失败 (sqlx 连接池切换 +
        // PRAGMA foreign_keys=OFF 失效)。subscriptions_new 里是用户的真实订阅数据,
        // 不能丢; 我们在这里自动完成 RENAME 并记录 v=5。
        //
        // 限制: 此自愈只识别表名 `subscriptions_new`, 写死 v=5。v7 也是重建表 migration 但
        // 沿用同名临时表, 故 v7 half-finished 也会被当 v5 处理 (rename → v=5), 然后 main
        // loop 重跑 v6 + v7 (v7 SQL 在 v7-schema 表上重跑是幂等的)。未来再加重建表
        // migration 时, 临时表请用 `subscriptions_v<N>_new` 之类带版本号的名字, 并扩展此
        // 处的识别+版本写入逻辑, 否则会与 v5 自愈互相混淆。
        info!("detected v5 half-finished migration, completing rename");
        sqlx::query("PRAGMA foreign_keys=OFF")
            .execute(&mut *conn)
            .await?;
        sqlx::query("ALTER TABLE subscriptions_new RENAME TO subscriptions")
            .execute(&mut *conn)
            .await?;
        sqlx::query("INSERT OR IGNORE INTO _schema_version (version, applied_at) VALUES (?, ?)")
            .bind(5_i64)
            .bind(chrono::Utc::now().timestamp_millis())
            .execute(&mut *conn)
            .await?;
        sqlx::query("PRAGMA foreign_keys=ON")
            .execute(&mut *conn)
            .await?;
    }

    let current: (Option<i64>,) = sqlx::query_as("SELECT MAX(version) FROM _schema_version")
        .fetch_one(&mut *conn)
        .await?;
    let current_version = current.0.unwrap_or(0) as u32;

    for (v, sql) in MIGRATIONS {
        if *v <= current_version {
            continue;
        }
        info!(version = v, "applying migration");
        for stmt in split_sql_statements(sql) {
            sqlx::query(&stmt).execute(&mut *conn).await?;
        }
        sqlx::query("INSERT OR IGNORE INTO _schema_version (version, applied_at) VALUES (?, ?)")
            .bind(*v as i64)
            .bind(chrono::Utc::now().timestamp_millis())
            .execute(&mut *conn)
            .await?;
    }

    drop(conn);

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
    use uuid::Uuid;

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

    async fn has_table(pool: &SqlitePool, table: &str) -> bool {
        let row: (i64,) = sqlx::query_as(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?",
        )
        .bind(table)
        .fetch_one(pool)
        .await
        .unwrap();
        row.0 > 0
    }

    #[tokio::test]
    async fn fresh_db_applies_all_migrations() {
        let pool = in_memory_pool().await;
        let dir = std::path::PathBuf::from(".");
        run_migrations(&pool, &dir).await.expect("migrate fresh");

        let versions = applied_versions(&pool).await;
        assert_eq!(versions, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert!(!has_column(&pool, "subscriptions", "supports_thinking_blocks").await);
        assert!(!has_column(&pool, "subscriptions", "thinking_block_field_name").await);
        assert!(has_column(&pool, "requests", "upstream_response_body").await);
        assert!(has_table(&pool, "events").await);
        assert!(has_table(&pool, "request_stats_daily").await);
        assert!(has_column(&pool, "subscriptions", "auth_type").await);
        assert!(has_column(&pool, "subscriptions", "oauth_metadata").await);
    }

    #[tokio::test]
    async fn legacy_v1_db_baselines_then_applies_increments() {
        let pool = in_memory_pool().await;
        // 模拟 v1 老 DB: 只跑 001, 不写 _schema_version
        for stmt in split_sql_statements(MIGRATIONS[0].1) {
            sqlx::query(&stmt).execute(&pool).await.unwrap();
        }
        // 此时 subscriptions 存在, _schema_version 不存在

        let dir = std::path::PathBuf::from(".");
        run_migrations(&pool, &dir).await.expect("migrate legacy");

        let versions = applied_versions(&pool).await;
        assert_eq!(versions, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]); // baseline v=1, 然后跑增量
        assert!(!has_column(&pool, "subscriptions", "supports_thinking_blocks").await);
        assert!(!has_column(&pool, "subscriptions", "thinking_block_field_name").await);
        assert!(has_column(&pool, "requests", "upstream_response_body").await);
        assert!(has_table(&pool, "events").await);
        assert!(has_table(&pool, "request_stats_daily").await);
    }

    #[tokio::test]
    async fn rerunning_migrations_is_idempotent() {
        let pool = in_memory_pool().await;
        let dir = std::path::PathBuf::from(".");
        run_migrations(&pool, &dir).await.expect("first run");
        run_migrations(&pool, &dir).await.expect("second run");
        run_migrations(&pool, &dir).await.expect("third run");

        let versions = applied_versions(&pool).await;
        assert_eq!(versions, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]); // 没有重复写
    }

    /// 在 v4 schema 状态下插一条订阅 (含已 v7 移除的 supports_thinking_blocks 列)。
    /// 仅供 `detects_v5_half_finished_and_completes_rename` 在 MIGRATIONS[..4] 应用后调用,
    /// 不可在 v>=5 schema 上跑——subscriptions 表自 v7 起已无该列, INSERT 会失败。
    async fn insert_pre_v4_subscription(pool: &SqlitePool, provider_id: &str) {
        sqlx::query(
            "INSERT INTO subscriptions (id, provider_id, endpoint_id, display_name, api_key,
                model_slot_opus, model_slot_sonnet, model_slot_haiku,
                enabled, is_auth_failed, last_error_message, created_at, updated_at,
                base_url, messages_path, auth_header_name, auth_header_format,
                required_headers, forward_headers, model_discovery,
                provider_display_name, provider_icon, is_user_defined,
                supports_thinking_blocks)
             VALUES (?, ?, 'ep', 'name', 'k',
                     'a','b','c', 1, 0, NULL, 0, 0,
                     '', '', '', 'bearer', '{}', '[]', '{}',
                     'pname', 'icon', 0, 0)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(provider_id)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn detects_v5_half_finished_and_completes_rename() {
        let pool = in_memory_pool().await;
        sqlx::query(
            "CREATE TABLE _schema_version (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        for (v, sql) in &MIGRATIONS[..4] {
            for stmt in split_sql_statements(sql) {
                sqlx::query(&stmt).execute(&pool).await.unwrap();
            }
            sqlx::query("INSERT INTO _schema_version (version, applied_at) VALUES (?, 0)")
                .bind(*v as i64)
                .execute(&pool)
                .await
                .unwrap();
        }
        insert_pre_v4_subscription(&pool, "deepseek").await;

        // 模拟 v5 跑到 DROP TABLE 已 commit、ALTER RENAME 失败的半成品状态:
        // 拿 v5 SQL 的前 4 条 (PRAGMA off, CREATE _new, INSERT, DROP), 跳过 ALTER + PRAGMA on。
        let v5_stmts = split_sql_statements(MIGRATIONS[4].1);
        for stmt in &v5_stmts[..4] {
            sqlx::query(stmt).execute(&pool).await.unwrap();
        }
        assert!(has_table(&pool, "subscriptions_new").await);
        assert!(!has_table(&pool, "subscriptions").await);

        run_migrations(&pool, &std::path::PathBuf::from("."))
            .await
            .expect("migrate from half-finished v5");

        assert!(has_table(&pool, "subscriptions").await);
        assert!(!has_table(&pool, "subscriptions_new").await);
        assert_eq!(applied_versions(&pool).await, vec![1, 2, 3, 4, 5, 6, 7, 8, 9]);

        let count: (i64,) =
            sqlx::query_as("SELECT count(*) FROM subscriptions WHERE provider_id='deepseek'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count.0, 1, "subscriptions_new 里的订阅数据应在自动 RENAME 后保留");
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
