//! 事件流查询 command。简单 offset/limit 分页, 按 timestamp 倒序。
//! 支持按 kind / subscription_id / severity 筛选, 是 Logs 页三 tab 的统一入口。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use tauri::State;

use crate::error::AppResult;
use crate::observability::events::{EventKind, Severity};
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct EventDto {
    pub id: String,
    pub timestamp: i64,
    pub kind: EventKind,
    pub severity: Severity,
    pub subscription_id: Option<String>,
    pub request_id: Option<String>,
    pub summary: String,
    /// 解析后的结构化 payload。前端按 kind 决定如何展示。
    pub payload: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct ListEventsResult {
    pub items: Vec<EventDto>,
    pub total: i64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct EventFilters {
    pub kind: Option<EventKind>,
    pub subscription_id: Option<String>,
    pub severity: Option<Severity>,
}

#[tauri::command]
pub async fn list_events(
    state: State<'_, AppState>,
    page: u32,
    page_size: u32,
    filters: Option<EventFilters>,
) -> AppResult<ListEventsResult> {
    let page = page.max(1);
    let page_size = page_size.clamp(1, 200);
    let offset = (page - 1) as i64 * page_size as i64;
    let filters = filters.unwrap_or_default();

    let mut conditions: Vec<&'static str> = Vec::new();
    if filters.kind.is_some() {
        conditions.push("kind = ?");
    }
    if filters.subscription_id.is_some() {
        conditions.push("subscription_id = ?");
    }
    if filters.severity.is_some() {
        conditions.push("severity = ?");
    }
    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };

    let kind_str = filters.kind.map(|k| k.as_str().to_string());
    let severity_str = filters.severity.map(|s| s.as_str().to_string());

    let count_sql = format!("SELECT COUNT(*) AS c FROM events{}", where_clause);
    let total: i64 = bind_filters(
        sqlx::query(&count_sql),
        kind_str.as_deref(),
        filters.subscription_id.as_deref(),
        severity_str.as_deref(),
    )
    .fetch_one(&state.db)
    .await?
    .try_get("c")?;

    let select_sql = format!(
        "SELECT id, timestamp, kind, severity, subscription_id, request_id, summary, payload
         FROM events{}
         ORDER BY timestamp DESC
         LIMIT ? OFFSET ?",
        where_clause
    );
    let rows = bind_filters(
        sqlx::query(&select_sql),
        kind_str.as_deref(),
        filters.subscription_id.as_deref(),
        severity_str.as_deref(),
    )
    .bind(page_size as i64)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    let items = rows
        .into_iter()
        .map(|r| {
            let kind: String = r.try_get("kind").unwrap_or_default();
            let severity: String = r.try_get("severity").unwrap_or_default();
            let payload_text: Option<String> = r.try_get("payload").ok();
            EventDto {
                id: r.try_get("id").unwrap_or_default(),
                timestamp: r.try_get("timestamp").unwrap_or(0),
                kind: parse_kind(&kind),
                severity: parse_severity(&severity),
                subscription_id: r.try_get("subscription_id").ok(),
                request_id: r.try_get("request_id").ok(),
                summary: r.try_get("summary").unwrap_or_default(),
                payload: payload_text.and_then(|s| serde_json::from_str(&s).ok()),
            }
        })
        .collect();

    Ok(ListEventsResult { items, total })
}

fn bind_filters<'q>(
    mut q: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    kind: Option<&'q str>,
    subscription_id: Option<&'q str>,
    severity: Option<&'q str>,
) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
    if let Some(v) = kind {
        q = q.bind(v);
    }
    if let Some(v) = subscription_id {
        q = q.bind(v);
    }
    if let Some(v) = severity {
        q = q.bind(v);
    }
    q
}

fn parse_kind(s: &str) -> EventKind {
    match s {
        "request" => EventKind::Request,
        "subscription_state_change" => EventKind::SubscriptionStateChange,
        _ => EventKind::SystemError,
    }
}

fn parse_severity(s: &str) -> Severity {
    match s {
        "info" => Severity::Info,
        "warn" => Severity::Warn,
        _ => Severity::Error,
    }
}
