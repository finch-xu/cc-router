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
    (
        10,
        include_str!("../../migrations/010_add_subscription_balance.sql"),
    ),
    (
        11,
        include_str!("../../migrations/011_add_request_entry_kind_and_http_version.sql"),
    ),
    (
        12,
        include_str!("../../migrations/012_add_model_slot_fable.sql"),
    ),
    (
        13,
        include_str!("../../migrations/013_add_slot_efforts.sql"),
    ),
    (
        14,
        include_str!("../../migrations/014_rename_custom_provider_ids.sql"),
    ),
    (
        15,
        include_str!("../../migrations/015_add_receipt_stats_daily.sql"),
    ),
    (
        16,
        include_str!("../../migrations/016_add_model_slot_fallback.sql"),
    ),
    (
        17,
        include_str!("../../migrations/017_add_token_quotas.sql"),
    ),
    (
        18,
        include_str!("../../migrations/018_add_request_effort.sql"),
    ),
    (
        19,
        include_str!("../../migrations/019_stats_daily_local_day.sql"),
    ),
    (
        20,
        include_str!("../../migrations/020_add_forward_client_headers.sql"),
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
            sqlx::query(sqlx::AssertSqlSafe(stmt.as_str())).execute(&mut *conn).await?;
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
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!("PRAGMA table_info({})", table)))
            .fetch_all(pool)
            .await
            .unwrap();
        rows.iter()
            .any(|r| r.try_get::<String, _>("name").map(|n| n == column).unwrap_or(false))
    }

    /// 手动应用 MIGRATIONS[range] 并写版本记录 (照 v5 half-finished 测试的做法),
    /// 让针对某个版本的测试锁定在被测版本, 不被后续 migration (如 v19 重建聚合表) 改写。
    async fn apply_migrations(pool: &SqlitePool, range: std::ops::Range<usize>) {
        for (v, sql) in &MIGRATIONS[range] {
            for stmt in split_sql_statements(sql) {
                sqlx::query(sqlx::AssertSqlSafe(stmt.as_str())).execute(pool).await.unwrap();
            }
            sqlx::query("INSERT OR IGNORE INTO _schema_version (version, applied_at) VALUES (?, 0)")
                .bind(*v as i64)
                .execute(pool)
                .await
                .unwrap();
        }
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
        assert_eq!(versions, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]);
        assert!(!has_column(&pool, "subscriptions", "supports_thinking_blocks").await);
        assert!(!has_column(&pool, "subscriptions", "thinking_block_field_name").await);
        assert!(has_column(&pool, "requests", "upstream_response_body").await);
        assert!(has_table(&pool, "events").await);
        assert!(has_table(&pool, "request_stats_daily").await);
        assert!(has_table(&pool, "receipt_stats_daily").await);
        assert!(has_column(&pool, "subscriptions", "auth_type").await);
        assert!(has_column(&pool, "subscriptions", "oauth_metadata").await);
        assert!(has_column(&pool, "subscriptions", "model_slot_fallback").await);
        // v18: 请求日志的思考强度四列
        assert!(has_column(&pool, "requests", "client_effort").await);
        assert!(has_column(&pool, "requests", "effective_effort").await);
        assert!(has_column(&pool, "requests", "effort_source").await);
        assert!(has_column(&pool, "requests", "upstream_effort").await);
    }

    #[tokio::test]
    async fn legacy_v1_db_baselines_then_applies_increments() {
        let pool = in_memory_pool().await;
        // 模拟 v1 老 DB: 只跑 001, 不写 _schema_version
        for stmt in split_sql_statements(MIGRATIONS[0].1) {
            sqlx::query(sqlx::AssertSqlSafe(stmt.as_str())).execute(&pool).await.unwrap();
        }
        // 此时 subscriptions 存在, _schema_version 不存在

        let dir = std::path::PathBuf::from(".");
        run_migrations(&pool, &dir).await.expect("migrate legacy");

        let versions = applied_versions(&pool).await;
        assert_eq!(versions, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]); // baseline v=1, 然后跑增量
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
        assert_eq!(versions, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]); // 没有重复写
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
                sqlx::query(sqlx::AssertSqlSafe(stmt.as_str())).execute(&pool).await.unwrap();
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
            sqlx::query(sqlx::AssertSqlSafe(stmt.as_str())).execute(&pool).await.unwrap();
        }
        assert!(has_table(&pool, "subscriptions_new").await);
        assert!(!has_table(&pool, "subscriptions").await);

        run_migrations(&pool, &std::path::PathBuf::from("."))
            .await
            .expect("migrate from half-finished v5");

        assert!(has_table(&pool, "subscriptions").await);
        assert!(!has_table(&pool, "subscriptions_new").await);
        assert_eq!(
            applied_versions(&pool).await,
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20]
        );

        let count: (i64,) =
            sqlx::query_as("SELECT count(*) FROM subscriptions WHERE provider_id='deepseek'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count.0, 1, "subscriptions_new 里的订阅数据应在自动 RENAME 后保留");
    }

    /// v13 schema 下插一条订阅 (列清单与 store.rs::insert 对齐)。
    async fn insert_v13_subscription(pool: &SqlitePool, provider_id: &str, endpoint_id: &str) {
        sqlx::query(
            "INSERT INTO subscriptions (id, provider_id, endpoint_id, display_name, api_key,
                model_slot_fable, model_slot_opus, model_slot_sonnet, model_slot_haiku,
                enabled, is_auth_failed, last_error_message, created_at, updated_at,
                base_url, messages_path, auth_header_name, auth_header_format,
                required_headers, forward_headers, model_discovery, balance_discovery,
                provider_display_name, provider_icon, is_user_defined,
                auth_type, oauth_metadata, slot_efforts)
             VALUES (?, ?, ?, 'name', 'k',
                     'f','a','b','c', 1, 0, NULL, 0, 0,
                     '', '', '', 'bearer', '{}', '[]', '{}', NULL,
                     'pname', 'icon', 1, 'api_key', '{}', '{}')",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(provider_id)
        .bind(endpoint_id)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn v14_renames_custom_provider_ids_across_tables() {
        let pool = in_memory_pool().await;
        sqlx::query(
            "CREATE TABLE _schema_version (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // 应用 v1..=13, 手写版本记录 (照 v5 half-finished 测试的做法)
        for (v, sql) in &MIGRATIONS[..13] {
            for stmt in split_sql_statements(sql) {
                sqlx::query(sqlx::AssertSqlSafe(stmt.as_str())).execute(&pool).await.unwrap();
            }
            sqlx::query("INSERT INTO _schema_version (version, applied_at) VALUES (?, 0)")
                .bind(*v as i64)
                .execute(&pool)
                .await
                .unwrap();
        }

        // 4 张表各插旧 marker 行 + 内置 id 对照行
        insert_v13_subscription(&pool, "__custom_openai__", "__custom_openai__").await;
        insert_v13_subscription(&pool, "deepseek", "cn").await;
        sqlx::query(
            "INSERT INTO requests (id, timestamp, virtual_model_name, subscription_id,
                provider_id, endpoint_id, real_model_name, is_streaming, status)
             VALUES ('r1', 1, 'model-opus', 's1', '__custom_gemini__', '__custom_gemini__', 'm', 0, 'success'),
                    ('r2', 2, 'model-opus', 's2', 'zhipu', 'cn', 'm', 0, 'success')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO request_stats_daily (date_utc, virtual_model_name, subscription_id, provider_id)
             VALUES (0, 'model-opus', 's1', '__custom__'), (0, 'model-opus', 's2', 'zhipu')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO model_list_cache (subscription_id, endpoint_id, fetched_at, models_json)
             VALUES ('s1', '__custom_openai_chat__', 0, '[]'), ('s2', 'cn', 0, '[]')",
        )
        .execute(&pool)
        .await
        .unwrap();

        // 只应用 v14 本身 (v19 会按 requests 重建 request_stats_daily, 会改写这里手插的 marker 行)
        apply_migrations(&pool, 13..14).await;

        let fetch_one = |sql: &'static str| {
            let pool = pool.clone();
            async move {
                sqlx::query_as::<_, (String,)>(sql)
                    .fetch_one(&pool)
                    .await
                    .unwrap()
                    .0
            }
        };
        assert_eq!(
            fetch_one("SELECT provider_id FROM subscriptions WHERE display_name='name' AND provider_id LIKE 'custom%'").await,
            "custom-openai"
        );
        assert_eq!(
            fetch_one("SELECT endpoint_id FROM subscriptions WHERE provider_id='custom-openai'").await,
            "custom-openai"
        );
        assert_eq!(
            fetch_one("SELECT provider_id FROM requests WHERE id='r1'").await,
            "custom-gemini"
        );
        assert_eq!(
            fetch_one("SELECT endpoint_id FROM requests WHERE id='r1'").await,
            "custom-gemini"
        );
        assert_eq!(
            fetch_one("SELECT provider_id FROM request_stats_daily WHERE subscription_id='s1'").await,
            "custom"
        );
        assert_eq!(
            fetch_one("SELECT endpoint_id FROM model_list_cache WHERE subscription_id='s1'").await,
            "custom-openai-chat"
        );
        // 内置 id 行不受影响
        assert_eq!(
            fetch_one("SELECT provider_id FROM requests WHERE id='r2'").await,
            "zhipu"
        );
        assert_eq!(
            fetch_one("SELECT provider_id FROM subscriptions WHERE endpoint_id='cn'").await,
            "deepseek"
        );
        assert_eq!(
            fetch_one("SELECT provider_id FROM request_stats_daily WHERE subscription_id='s2'").await,
            "zhipu"
        );
    }

    #[tokio::test]
    async fn v15_backfills_receipt_stats_from_requests() {
        let pool = in_memory_pool().await;
        sqlx::query(
            "CREATE TABLE _schema_version (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        for (v, sql) in &MIGRATIONS[..14] {
            for stmt in split_sql_statements(sql) {
                sqlx::query(sqlx::AssertSqlSafe(stmt.as_str())).execute(&pool).await.unwrap();
            }
            sqlx::query("INSERT INTO _schema_version (version, applied_at) VALUES (?, 0)")
                .bind(*v as i64)
                .execute(&pool)
                .await
                .unwrap();
        }

        // 同桶两条 (r1+r2) / 隔天一条 (r3) / 同天不同 real_model 一条 (r4)
        const DAY: i64 = 86_400_000;
        sqlx::query(
            "INSERT INTO requests (id, timestamp, virtual_model_name, subscription_id,
                provider_id, endpoint_id, real_model_name, is_streaming, status,
                upstream_input_tokens, upstream_output_tokens,
                upstream_cache_creation, upstream_cache_read)
             VALUES
               ('r1', 100, 'model-opus', 's1', 'zhipu', 'cn', 'm1', 0, 'success', 10, 20, 1, 2),
               ('r2', 200, 'model-opus', 's1', 'zhipu', 'cn', 'm1', 0, 'error', 5, 5, NULL, NULL),
               ('r3', ?, 'model-opus', 's1', 'zhipu', 'cn', 'm1', 0, 'success', 7, 7, 0, 0),
               ('r4', 150, 'model-opus', 's1', 'zhipu', 'cn', 'm2', 0, 'success', 1, 1, 0, 0)",
        )
        .bind(DAY + 100)
        .execute(&pool)
        .await
        .unwrap();

        // 只应用 v15 本身 (v19 会把 receipt_stats_daily 的 date_utc 换成本地日 key)
        apply_migrations(&pool, 14..15).await;

        let rows: (i64,) = sqlx::query_as("SELECT count(*) FROM receipt_stats_daily")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows.0, 3, "3 个聚合桶: (day0,m1) / (day1,m1) / (day0,m2)");

        let bucket: (i64, i64, i64, i64, i64) = sqlx::query_as(
            "SELECT request_count, input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens
             FROM receipt_stats_daily
             WHERE date_utc = 0 AND real_model_name = 'm1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(bucket, (2, 15, 25, 1, 2), "同桶两条聚合, NULL token 按 0 计, status 不过滤");
    }

    /// v19: 聚合表按本地日历日重建。用 requests 原始行 (含跨 UTC 日边界的时间戳) + 旧表 UTC 行验证:
    /// - 重建行的 day 与 `local_day_key` (chrono::Local) 一致
    /// - 早于原始日志覆盖范围的旧行按 UTC 日期近似搬入, 跨界 UTC 日的旧行丢弃
    /// - 版本号 19 已写入; 再跑一次 run_migrations 幂等不报错
    #[tokio::test]
    async fn v19_rebuilds_daily_tables_by_local_day() {
        use crate::observability::request_log::local_day_key;
        let pool = in_memory_pool().await;
        sqlx::query(
            "CREATE TABLE _schema_version (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        apply_migrations(&pool, 0..18).await;

        const DAY: i64 = 86_400_000;
        // 原始日志: 三条落在 UTC 第 100 天附近, 其中 t2 = 该 UTC 日 23:30 (东八区已是次日 07:30)
        let base = 100 * DAY;
        let t1 = base + 3_600_000; // 01:00Z
        let t2 = base + DAY - 1_800_000; // 23:30Z
        let t3 = base + DAY + 3_600_000; // 次日 01:00Z
        sqlx::query(
            "INSERT INTO requests (id, timestamp, virtual_model_name, subscription_id,
                provider_id, endpoint_id, real_model_name, is_streaming, status,
                total_latency_ms, ttft_ms, upstream_input_tokens, upstream_output_tokens,
                upstream_cache_creation, upstream_cache_read, retry_count)
             VALUES
               ('r1', ?, 'model-opus', 's1', 'zhipu', 'cn', 'm1', 0, 'success', 100, 10, 10, 20, 1, 2, 0),
               ('r2', ?, 'model-opus', 's1', 'zhipu', 'cn', 'm1', 0, 'error', NULL, NULL, 5, 5, NULL, NULL, 1),
               ('r3', ?, 'model-opus', 's1', 'zhipu', 'cn', 'm1', 0, 'timeout', 300, NULL, 7, 7, 0, 0, 0)",
        )
        .bind(t1)
        .bind(t2)
        .bind(t3)
        .execute(&pool)
        .await
        .unwrap();
        // 旧聚合表: 一行早于覆盖范围 (UTC 第 50 天, 应搬入为 '1970-02-20'), 一行就是覆盖范围首日 (应丢弃)
        sqlx::query(
            "INSERT INTO request_stats_daily (date_utc, virtual_model_name, subscription_id, provider_id, request_count)
             VALUES (?, 'model-haiku', 's9', 'anthropic', 42), (?, 'model-haiku', 's9', 'anthropic', 7)",
        )
        .bind(50 * DAY)
        .bind(base)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO receipt_stats_daily (date_utc, virtual_model_name, subscription_id, real_model_name, provider_id, request_count)
             VALUES (?, 'model-haiku', 's9', 'mx', 'anthropic', 42)",
        )
        .bind(50 * DAY)
        .execute(&pool)
        .await
        .unwrap();

        run_migrations(&pool, &std::path::PathBuf::from("."))
            .await
            .expect("apply v19");

        // 版本号已写入
        let has19: (i64,) =
            sqlx::query_as("SELECT count(*) FROM _schema_version WHERE version = 19")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(has19.0, 1);

        // 重建行: day 与 Rust 侧 local_day_key 一致 (无论机器时区)
        let rows: Vec<(String, i64, i64, i64, i64, i64, i64, i64, i64, i64)> = sqlx::query_as(
            "SELECT day, request_count, success_count, error_count, timeout_count,
                    input_tokens, cache_creation_tokens, total_duration_ms_sum, total_duration_ms_count, retry_count_sum
             FROM request_stats_daily WHERE subscription_id = 's1' ORDER BY day",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let mut expected: std::collections::BTreeMap<String, (i64, i64, i64, i64, i64, i64, i64, i64, i64)> =
            std::collections::BTreeMap::new();
        for (ts, st, lat, inp, cc, retry) in [
            (t1, "success", Some(100), 10, 1, 0),
            (t2, "error", None, 5, 0, 1),
            (t3, "timeout", Some(300), 7, 0, 0),
        ] {
            let e = expected.entry(local_day_key(ts)).or_default();
            e.0 += 1;
            match st {
                "success" => e.1 += 1,
                "error" => e.2 += 1,
                _ => e.3 += 1,
            }
            e.4 += inp;
            e.5 += cc;
            if let Some(l) = lat {
                e.6 += l;
                e.7 += 1;
            }
            e.8 += retry;
        }
        let got: std::collections::BTreeMap<String, (i64, i64, i64, i64, i64, i64, i64, i64, i64)> = rows
            .into_iter()
            .map(|r| (r.0, (r.1, r.2, r.3, r.4, r.5, r.6, r.7, r.8, r.9)))
            .collect();
        assert_eq!(got, expected, "按本地日重建, 口径与 flush_batch 一致");

        // 旧行: 早于覆盖范围的搬入 (UTC 第 50 天 = 1970-02-20), 跨界那行丢弃
        let legacy: Vec<(String, i64)> = sqlx::query_as(
            "SELECT day, request_count FROM request_stats_daily WHERE subscription_id = 's9' ORDER BY day",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(legacy, vec![("1970-02-20".to_string(), 42)]);
        let legacy_receipt: Vec<(String, i64)> = sqlx::query_as(
            "SELECT day, request_count FROM receipt_stats_daily WHERE subscription_id = 's9'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(legacy_receipt, vec![("1970-02-20".to_string(), 42)]);

        // 小票表按 (day, vm, sub, real_model) 重建
        let receipt_rows: (i64,) =
            sqlx::query_as("SELECT count(*) FROM receipt_stats_daily WHERE subscription_id = 's1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(receipt_rows.0 as usize, expected.len());

        // 幂等: 再跑一次不报错, 行数不变
        run_migrations(&pool, &std::path::PathBuf::from("."))
            .await
            .expect("rerun is a no-op");
        let again: (i64,) = sqlx::query_as("SELECT count(*) FROM request_stats_daily")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(again.0 as usize, expected.len() + 1);
    }

    #[tokio::test]
    async fn v17_adds_token_quotas_column_and_usage_table() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        run_migrations(&pool, &std::path::PathBuf::from("."))
            .await
            .unwrap();

        // 列存在: 能 SELECT
        let v: String = sqlx::query_scalar("SELECT token_quotas FROM subscriptions LIMIT 0")
            .fetch_optional(&pool)
            .await
            .unwrap()
            .unwrap_or_else(|| "{}".to_string());
        assert_eq!(v, "{}");

        // 表存在: 能插入并读回
        sqlx::query(
            "INSERT INTO subscription_quota_usage
             (subscription_id, period, period_start_ms, input_tokens, output_tokens,
              cache_creation_tokens, cache_read_tokens, updated_at_ms)
             VALUES ('s1', 'daily', 0, 1, 2, 3, 4, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM subscription_quota_usage")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
    }
}

async fn seed_virtual_model_config(pool: &SqlitePool) -> AppResult<()> {
    for name in ["model-fable", "model-opus", "model-sonnet", "model-haiku", "model-fallback"] {
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
