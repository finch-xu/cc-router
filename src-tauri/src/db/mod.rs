pub mod paths;

use std::path::Path;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{ConnectOptions, SqlitePool};
use tokio::fs;
use tracing::info;

use crate::error::AppResult;

const INLINE_MIGRATION: &str = include_str!("../../migrations/001_init.sql");

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

/// 执行内嵌 migration。启动时检查 `subscriptions` 表是否存在，没有则执行完整 migration。
/// `resource_dir` 参数保留为后续版本化迁移预留。
pub async fn run_migrations(pool: &SqlitePool, _resource_dir: &Path) -> AppResult<()> {
    let existing: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='subscriptions'",
    )
    .fetch_one(pool)
    .await?;

    if existing.0 == 0 {
        info!("running initial migration");
        for stmt in split_sql_statements(INLINE_MIGRATION) {
            sqlx::query(&stmt).execute(pool).await?;
        }
        seed_virtual_model_config(pool).await?;
        seed_onboarding(pool).await?;
        info!("migration completed");
    }
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
    use super::split_sql_statements;

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
