//! 实时路由页的数据源: 每条请求日志条目 (= 一次上游 attempt) 进消费端时
//! 1) 发 `live_request` 事件 (已桥接给网页界面);
//! 2) 放进内存环形缓冲, 页面首屏用 [`get_live_requests`] 回填最近一小时。
//!
//! 缓冲不落库、app 重启即清空 —— 用途 / 会话这些字段本来也不落库。
//! 纪律同请求日志: 这里出任何问题都只能少一条实时数据, 不能影响请求处理与日志投递。

use std::collections::VecDeque;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::error::AppResult;
use crate::observability::request_log::{RequestLogEntry, RequestStatus};
use crate::provider::model::AuthType;
use crate::proxy::purpose::RequestPurpose;

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
pub struct LiveRequest {
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
    pub client_tool: Option<&'static str>,
    pub effort: Option<String>,
    pub stop_reason: Option<String>,
    pub tool_use_names: Vec<String>,
    pub tools_offered: Option<u32>,
    pub retry_count: u32,
    pub error: Option<String>,
    pub purpose: RequestPurpose,
    pub session: Option<LiveSession>,
}

#[derive(Default)]
pub struct LiveFeed {
    buf: Mutex<VecDeque<LiveRequest>>,
}

impl LiveFeed {
    fn push(&self, item: LiveRequest) {
        let Ok(mut buf) = self.buf.lock() else { return };
        let cutoff = item.timestamp - KEEP_MS;
        while buf.front().is_some_and(|r| r.timestamp < cutoff) || buf.len() >= KEEP_MAX {
            buf.pop_front();
        }
        buf.push_back(item);
    }

    fn since(&self, since_ms: i64) -> Vec<LiveRequest> {
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

pub fn from_entry(entry: &RequestLogEntry, auth_type: Option<AuthType>) -> LiveRequest {
    let read = u64::from(entry.upstream_cache_read.unwrap_or_default());
    let write = u64::from(entry.upstream_cache_creation.unwrap_or_default());
    let raw_input = u64::from(entry.upstream_input_tokens.unwrap_or_default());
    // Anthropic 透传: input 本来就不含缓存; 其余协议的 input 是含缓存的总数 (同 session_monitor::proxy_input)
    let input_tokens = match auth_type {
        Some(AuthType::ApiKey) | None => raw_input,
        Some(_) => raw_input.saturating_sub(read + write),
    };
    let tool_use_names = entry
        .tool_calls
        .tool_use_names
        .as_deref()
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|n| n != crate::proxy::tool_log::TRUNCATED_MARKER)
        .collect();
    LiveRequest {
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
        input_tokens,
        output_tokens: u64::from(entry.upstream_output_tokens.unwrap_or_default()),
        cache_read_tokens: read,
        cache_write_tokens: write,
        client_tool: entry.client_tool,
        effort: entry.effective_effort.clone(),
        stop_reason: entry.tool_calls.stop_reason.clone(),
        tool_use_names,
        tools_offered: entry.tool_calls.tools_offered_count,
        retry_count: entry.retry_count,
        error: (!matches!(entry.status, RequestStatus::Success))
            .then(|| entry.error_message.as_deref().map(|e| truncate(e, ERROR_LIMIT)))
            .flatten(),
        purpose: entry.tool_calls.purpose.clone(),
        session: entry.monitor_session.as_ref().map(|(client, id)| LiveSession {
            client: client.name().to_owned(),
            id: id.clone(),
        }),
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

/// 最近 `since_ms` 之后的实时请求 (缺省 = 缓冲里全部, 最多 1 小时)。
#[tauri::command]
pub async fn get_live_requests(app: AppHandle, since_ms: Option<i64>) -> AppResult<Vec<LiveRequest>> {
    Ok(app
        .try_state::<LiveFeed>()
        .map(|f| f.since(since_ms.unwrap_or(0)))
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(ts: i64) -> LiveRequest {
        LiveRequest {
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
            purpose: RequestPurpose::default(),
            session: None,
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
