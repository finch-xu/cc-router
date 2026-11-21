//! 一条请求的摘要 [`RequestSummary`]: 实时路由的事件行与请求日志列表的行是同一个形状。
//!
//! 两个来源, 一套口径 (输入归一 / 首字取值 / 工具名解析只写一次):
//! - [`from_entry`]: 请求日志消费端每收到一条 entry (= 一次上游 attempt) 时生成, 发 `live_request`
//!   事件 (已桥接给网页界面) 并放进内存环形缓冲;
//! - [`from_row`]: 从 `requests` 表读出 (请求日志列表、实时路由首屏回填)。
//!
//! 实时路由首屏 [`get_live_requests`] = 库里最近一小时 ∪ 缓冲里还没 flush 的条目, 所以实时路由的
//! 每一行都在请求日志里, 重启后也不是空的。
//! 纪律同请求日志: 这里出任何问题都只能少一条实时数据, 不能影响请求处理与日志投递。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;

use serde::Serialize;
use sqlx::Row;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::error::AppResult;
use crate::observability::request_log::{RequestLogEntry, RequestStatus};
use crate::provider::model::AuthType;
use crate::proxy::purpose::{PurposeKind, RequestPurpose};
use crate::state::AppState;

pub const EVENT: &str = "live_request";
/// 页面最长的时间窗是 1 小时; 再加一个条数上限防内存失控
const KEEP_MS: i64 = 60 * 60 * 1000;
const KEEP_MAX: usize = 5000;
const ERROR_LIMIT: usize = 200;

#[derive(Debug, Clone, Serialize)]
pub struct LiveSession {
    /// "claude" / "codex"
    pub client: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RequestSummary {
    pub id: String,
    pub timestamp: i64,
    pub virtual_model: String,
    pub subscription_id: String,
    pub provider_id: String,
    pub model: String,
    /// success / error / timeout
    pub status: &'static str,
    pub http_status: Option<u16>,
    pub ttft_ms: Option<u64>,
    pub total_ms: Option<u64>,
    /// 未命中缓存的输入 (各协议已归一: 不含缓存读写)
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub client_tool: Option<String>,
    pub effort: Option<String>,
    pub stop_reason: Option<String>,
    pub tool_use_names: Vec<String>,
    pub tools_offered: Option<u32>,
    pub retry_count: u32,
    pub error: Option<String>,
    /// None = 迁移 030 之前写入的老记录, 没有用途信息
    pub purpose: Option<RequestPurpose>,
    pub session: Option<LiveSession>,
    /// 同一次入站请求的所有 attempt 共用; 老记录为 None
    pub trace_id: Option<String>,
}

/// 未命中缓存的输入。Anthropic 透传的 input 本来就不含缓存; 其余协议的 input 是含缓存的总数
/// (同 session_monitor::proxy_input)。订阅已删时 auth 未知, 按透传处理。
fn uncached_input(auth: Option<AuthType>, raw: u64, read: u64, write: u64) -> u64 {
    match auth {
        Some(AuthType::ApiKey) | None => raw,
        Some(_) => raw.saturating_sub(read + write),
    }
}

fn parse_tool_names(json: Option<&str>) -> Vec<String> {
    json.and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|n| n != crate::proxy::tool_log::TRUNCATED_MARKER && !n.is_empty())
        .collect()
}

fn error_text(ok: bool, message: Option<&str>) -> Option<String> {
    if ok {
        return None;
    }
    message.map(|e| truncate(e, ERROR_LIMIT))
}

fn status_str(s: &str) -> &'static str {
    match s {
        "success" => "success",
        "timeout" => "timeout",
        _ => "error",
    }
}

#[derive(Default)]
pub struct LiveFeed {
    buf: Mutex<VecDeque<RequestSummary>>,
}

impl LiveFeed {
    fn push(&self, item: RequestSummary) {
        let Ok(mut buf) = self.buf.lock() else { return };
        let cutoff = item.timestamp - KEEP_MS;
        while buf.front().is_some_and(|r| r.timestamp < cutoff) || buf.len() >= KEEP_MAX {
            buf.pop_front();
        }
        buf.push_back(item);
    }

    fn since(&self, since_ms: i64) -> Vec<RequestSummary> {
        self.buf
            .lock()
            .map(|b| b.iter().filter(|r| r.timestamp >= since_ms).cloned().collect())
            .unwrap_or_default()
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

pub fn from_entry(entry: &RequestLogEntry, auth_type: Option<AuthType>) -> RequestSummary {
    let read = u64::from(entry.upstream_cache_read.unwrap_or_default());
    let write = u64::from(entry.upstream_cache_creation.unwrap_or_default());
    let raw_input = u64::from(entry.upstream_input_tokens.unwrap_or_default());
    RequestSummary {
        id: entry.id.to_string(),
        timestamp: entry.timestamp_ms,
        virtual_model: entry.virtual_model_name.as_str().to_owned(),
        subscription_id: entry.subscription_id.to_string(),
        provider_id: entry.provider_id.clone(),
        model: entry.response_model_name.clone().unwrap_or_else(|| entry.real_model_name.clone()),
        status: entry.status.as_str(),
        http_status: entry.http_status,
        ttft_ms: entry.first_content_ms.or(entry.ttft_ms),
        total_ms: entry.total_latency_ms,
        input_tokens: uncached_input(auth_type, raw_input, read, write),
        output_tokens: u64::from(entry.upstream_output_tokens.unwrap_or_default()),
        cache_read_tokens: read,
        cache_write_tokens: write,
        client_tool: entry.client_tool.map(str::to_owned),
        effort: entry.effective_effort.clone(),
        stop_reason: entry.tool_calls.stop_reason.clone(),
        tool_use_names: parse_tool_names(entry.tool_calls.tool_use_names.as_deref()),
        tools_offered: entry.tool_calls.tools_offered_count,
        retry_count: entry.retry_count,
        error: error_text(matches!(entry.status, RequestStatus::Success), entry.error_message.as_deref()),
        purpose: Some(entry.tool_calls.purpose.clone()),
        session: entry.monitor_session.as_ref().map(|(client, id)| LiveSession {
            client: client.name().to_owned(),
            id: id.clone(),
        }),
        trace_id: entry.trace_id.clone(),
    }
}

/// `requests` 表的列, 供 [`from_row`] 使用。SELECT 里拼上它, 列名与 [`from_row`] 读取的一一对应。
pub(crate) const SUMMARY_COLUMNS: &str = "id, timestamp, virtual_model_name, subscription_id, provider_id,
    real_model_name, response_model_name, status, http_status, ttft_ms, first_content_ms, total_latency_ms,
    upstream_input_tokens, upstream_output_tokens, upstream_cache_creation, upstream_cache_read,
    client_tool, effective_effort, stop_reason, tool_use_names, tools_offered_count, retry_count,
    error_message, purpose_kind, purpose_guessed, purpose_turn, purpose_step, purpose_prev_tools,
    session_client, session_id, trace_id";

/// 订阅 id → 鉴权类型, 用来按协议归一输入 token。
pub(crate) async fn auth_types(state: &AppState) -> HashMap<String, AuthType> {
    let map = state.subscriptions.read().await;
    let mut out = HashMap::with_capacity(map.len());
    for (id, rt) in map.iter() {
        out.insert(id.to_string(), rt.read().await.row.auth_type);
    }
    out
}

/// 从 `requests` 行构造; 任何列读不到都按缺省处理, 不报错。
pub(crate) fn from_row(r: &sqlx::sqlite::SqliteRow, auth: &HashMap<String, AuthType>) -> RequestSummary {
    let opt_i64 = |c: &str| r.try_get::<Option<i64>, _>(c).ok().flatten();
    let opt_str = |c: &str| r.try_get::<Option<String>, _>(c).ok().flatten();
    let u = |c: &str| opt_i64(c).unwrap_or(0).max(0) as u64;
    let subscription_id: String = r.try_get("subscription_id").unwrap_or_default();
    let (read, write) = (u("upstream_cache_read"), u("upstream_cache_creation"));
    let status = status_str(&r.try_get::<String, _>("status").unwrap_or_default());
    let purpose = opt_str("purpose_kind").map(|k| RequestPurpose {
        kind: PurposeKind::parse(&k).unwrap_or_default(),
        guessed: opt_i64("purpose_guessed").unwrap_or(0) != 0,
        turn: opt_i64("purpose_turn").and_then(|v| u32::try_from(v).ok()),
        step: opt_i64("purpose_step").and_then(|v| u32::try_from(v).ok()),
        prev_tools: parse_tool_names(opt_str("purpose_prev_tools").as_deref()),
    });
    let session = match (opt_str("session_client"), opt_str("session_id")) {
        (Some(client), Some(id)) => Some(LiveSession { client, id }),
        _ => None,
    };
    RequestSummary {
        id: r.try_get("id").unwrap_or_default(),
        timestamp: r.try_get("timestamp").unwrap_or(0),
        virtual_model: r.try_get("virtual_model_name").unwrap_or_default(),
        provider_id: r.try_get("provider_id").unwrap_or_default(),
        model: opt_str("response_model_name").or_else(|| opt_str("real_model_name")).unwrap_or_default(),
        status,
        http_status: opt_i64("http_status").and_then(|v| u16::try_from(v).ok()),
        ttft_ms: opt_i64("first_content_ms").or_else(|| opt_i64("ttft_ms")).map(|v| v.max(0) as u64),
        total_ms: opt_i64("total_latency_ms").map(|v| v.max(0) as u64),
        input_tokens: uncached_input(auth.get(&subscription_id).copied(), u("upstream_input_tokens"), read, write),
        output_tokens: u("upstream_output_tokens"),
        cache_read_tokens: read,
        cache_write_tokens: write,
        client_tool: opt_str("client_tool"),
        effort: opt_str("effective_effort"),
        stop_reason: opt_str("stop_reason"),
        tool_use_names: parse_tool_names(opt_str("tool_use_names").as_deref()),
        tools_offered: opt_i64("tools_offered_count").and_then(|v| u32::try_from(v).ok()),
        retry_count: opt_i64("retry_count").and_then(|v| u32::try_from(v).ok()).unwrap_or(0),
        error: error_text(status == "success", opt_str("error_message").as_deref()),
        purpose,
        session,
        trace_id: opt_str("trace_id"),
        subscription_id,
    }
}

/// 消费端每收到一条日志条目调一次。
pub fn record(app: &AppHandle, entry: &RequestLogEntry, auth_type: Option<AuthType>) {
    let item = from_entry(entry, auth_type);
    let _ = app.emit(EVENT, &item);
    if let Some(feed) = app.try_state::<LiveFeed>() {
        feed.push(item);
    }
}

/// 最近 `since_ms` 之后的请求 (最多 1 小时): 库里的 ∪ 缓冲里还没 flush 的, 按时间正序。
#[tauri::command]
pub async fn get_live_requests(
    app: AppHandle,
    state: State<'_, AppState>,
    since_ms: Option<i64>,
) -> AppResult<Vec<RequestSummary>> {
    let floor = chrono::Utc::now().timestamp_millis() - KEEP_MS;
    let since = since_ms.unwrap_or(0).max(floor);
    let auth = auth_types(&state).await;
    let sql = format!(
        "SELECT {SUMMARY_COLUMNS} FROM requests WHERE timestamp >= ? ORDER BY timestamp DESC LIMIT ?"
    );
    // 查库失败就只用缓冲 (与改动前一致), 不让页面整个报错
    let mut rows: Vec<RequestSummary> = match sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(since)
        .bind(KEEP_MAX as i64)
        .fetch_all(&state.db)
        .await
    {
        Ok(rows) => rows.iter().map(|r| from_row(r, &auth)).collect(),
        Err(e) => {
            tracing::warn!(?e, "实时路由回填查库失败, 只用内存缓冲");
            Vec::new()
        }
    };
    let seen: HashSet<String> = rows.iter().map(|r| r.id.clone()).collect();
    if let Some(feed) = app.try_state::<LiveFeed>() {
        rows.extend(feed.since(since).into_iter().filter(|r| !seen.contains(&r.id)));
    }
    rows.sort_by_key(|r| r.timestamp);
    Ok(rows)
}

/// 测试用的最小摘要 (tui_contract / 命令单测构造 DTO 用)
#[cfg(test)]
pub(crate) fn sample_summary(id: &str) -> RequestSummary {
    RequestSummary {
        id: id.to_owned(),
        timestamp: 0,
        virtual_model: String::new(),
        subscription_id: String::new(),
        provider_id: String::new(),
        model: String::new(),
        status: "success",
        http_status: None,
        ttft_ms: None,
        total_ms: None,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        client_tool: None,
        effort: None,
        stop_reason: None,
        tool_use_names: vec![],
        tools_offered: None,
        retry_count: 0,
        error: None,
        purpose: None,
        session: None,
        trace_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(ts: i64) -> RequestSummary {
        RequestSummary {
            id: ts.to_string(),
            timestamp: ts,
            virtual_model: "sonnet".into(),
            subscription_id: String::new(),
            provider_id: String::new(),
            model: String::new(),
            status: "success",
            http_status: Some(200),
            ttft_ms: None,
            total_ms: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            client_tool: None,
            effort: None,
            stop_reason: None,
            tool_use_names: vec![],
            tools_offered: None,
            retry_count: 0,
            error: None,
            purpose: Some(RequestPurpose::default()),
            session: None,
            trace_id: None,
        }
    }

    #[test]
    fn buffer_drops_entries_older_than_an_hour_and_caps_length() {
        let feed = LiveFeed::default();
        feed.push(item(0));
        feed.push(item(KEEP_MS + 1));
        assert_eq!(feed.since(0).len(), 1);
        for i in 0..(KEEP_MAX as i64 + 10) {
            feed.push(item(KEEP_MS + 2 + i));
        }
        assert_eq!(feed.since(0).len(), KEEP_MAX);
        assert_eq!(feed.since(KEEP_MS + 2 + KEEP_MAX as i64).len(), 10);
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate("检查检查", 4), "检…");
        assert_eq!(truncate("abc", 4), "abc");
    }
}
