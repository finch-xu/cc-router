//! 统一事件流。`events` 表承载三类事件:
//! - `request`                    每条请求结束的摘要(详情仍读 requests 表)
//! - `subscription_state_change`  订阅健康状态机转换
//! - `system_error`               系统级故障(DB / yaml / 端口监听 等)
//!
//! 写入也走 mpsc + 批量 flush 模式, 与 request_log 一致但独立 channel/consumer。

use std::collections::VecDeque;

use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};
use tracing::{debug, warn};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Request,
    SubscriptionStateChange,
    SystemError,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::SubscriptionStateChange => "subscription_state_change",
            Self::SystemError => "system_error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warn,
    Error,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone)]
pub struct EventEntry {
    pub id: Uuid,
    pub timestamp_ms: i64,
    pub kind: EventKind,
    pub severity: Severity,
    pub subscription_id: Option<Uuid>,
    pub request_id: Option<Uuid>,
    pub summary: String,
    pub payload: Option<Value>,
}

const FLUSH_SIZE: usize = 50;
const FLUSH_INTERVAL: Duration = Duration::from_secs(5);
const BUFFER_MAX: usize = 1000;

pub async fn run_consumer(
    pool: SqlitePool,
    mut rx: mpsc::Receiver<EventEntry>,
    app: AppHandle,
) {
    let mut buffer: VecDeque<EventEntry> = VecDeque::with_capacity(FLUSH_SIZE);
    let mut ticker = interval(FLUSH_INTERVAL);

    loop {
        tokio::select! {
            maybe_entry = rx.recv() => {
                match maybe_entry {
                    Some(entry) => {
                        if buffer.len() >= BUFFER_MAX {
                            buffer.pop_front();
                        }
                        buffer.push_back(entry);
                        if buffer.len() >= FLUSH_SIZE {
                            flush(&pool, &mut buffer, &app).await;
                        }
                    }
                    None => {
                        flush(&pool, &mut buffer, &app).await;
                        break;
                    }
                }
            }
            _ = ticker.tick() => {
                if !buffer.is_empty() {
                    flush(&pool, &mut buffer, &app).await;
                }
            }
        }
    }
}

async fn flush(pool: &SqlitePool, buffer: &mut VecDeque<EventEntry>, app: &AppHandle) {
    if buffer.is_empty() {
        return;
    }
    let batch: Vec<EventEntry> = buffer.drain(..).collect();
    debug!(count = batch.len(), "flushing events");

    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            warn!(?e, "无法开启事件事务, 放回 buffer");
            buffer.extend(batch);
            let _ = app.emit("event_log_write_failed", e.to_string());
            return;
        }
    };

    for entry in batch {
        let payload_text = entry.payload.as_ref().map(|v| v.to_string());
        let result = sqlx::query(
            "INSERT INTO events (id, timestamp, kind, severity, subscription_id, request_id, summary, payload)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(entry.id.to_string())
        .bind(entry.timestamp_ms)
        .bind(entry.kind.as_str())
        .bind(entry.severity.as_str())
        .bind(entry.subscription_id.map(|id| id.to_string()))
        .bind(entry.request_id.map(|id| id.to_string()))
        .bind(entry.summary)
        .bind(payload_text)
        .execute(&mut *tx)
        .await;
        if let Err(e) = result {
            warn!(?e, "写入单条事件失败");
        }
    }
    if let Err(e) = tx.commit().await {
        warn!(?e, "提交事件事务失败");
        let _ = app.emit("event_log_write_failed", e.to_string());
    }
    let _ = app.emit("events_flushed", ());
}

/// 兜底入队: try_send 失败仅 warn, 不阻塞调用方
pub fn record(tx: &mpsc::Sender<EventEntry>, entry: EventEntry) {
    if let Err(e) = tx.try_send(entry) {
        warn!(?e, "events channel 写入失败, 丢弃");
    }
}

pub fn record_request(
    tx: &mpsc::Sender<EventEntry>,
    request_id: Uuid,
    subscription_id: Uuid,
    severity: Severity,
    summary: impl Into<String>,
) {
    record(
        tx,
        EventEntry {
            id: Uuid::new_v4(),
            timestamp_ms: Utc::now().timestamp_millis(),
            kind: EventKind::Request,
            severity,
            subscription_id: Some(subscription_id),
            request_id: Some(request_id),
            summary: summary.into(),
            payload: None,
        },
    );
}

pub fn record_state_change(
    tx: &mpsc::Sender<EventEntry>,
    subscription_id: Uuid,
    summary: impl Into<String>,
    payload: Value,
) {
    record(
        tx,
        EventEntry {
            id: Uuid::new_v4(),
            timestamp_ms: Utc::now().timestamp_millis(),
            kind: EventKind::SubscriptionStateChange,
            severity: Severity::Warn,
            subscription_id: Some(subscription_id),
            request_id: None,
            summary: summary.into(),
            payload: Some(payload),
        },
    );
}

pub fn record_system_error(
    tx: &mpsc::Sender<EventEntry>,
    summary: impl Into<String>,
    payload: Option<Value>,
) {
    record(
        tx,
        EventEntry {
            id: Uuid::new_v4(),
            timestamp_ms: Utc::now().timestamp_millis(),
            kind: EventKind::SystemError,
            severity: Severity::Error,
            subscription_id: None,
            request_id: None,
            summary: summary.into(),
            payload,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_kind_serializes_snake_case() {
        assert_eq!(EventKind::Request.as_str(), "request");
        assert_eq!(EventKind::SubscriptionStateChange.as_str(), "subscription_state_change");
        assert_eq!(EventKind::SystemError.as_str(), "system_error");
    }

    #[test]
    fn severity_serializes_snake_case() {
        assert_eq!(Severity::Info.as_str(), "info");
        assert_eq!(Severity::Warn.as_str(), "warn");
        assert_eq!(Severity::Error.as_str(), "error");
    }

    #[test]
    fn record_helpers_construct_correct_kind() {
        // 不依赖 AppHandle / DB, 只验证 helper 构造 EventEntry 的字段
        let (tx, mut rx) = mpsc::channel::<EventEntry>(8);
        record_system_error(&tx, "test", Some(serde_json::json!({"a": 1})));
        record_request(
            &tx,
            Uuid::new_v4(),
            Uuid::new_v4(),
            Severity::Error,
            "req summary",
        );
        record_state_change(
            &tx,
            Uuid::new_v4(),
            "sub: Healthy → AuthFailed",
            serde_json::json!({"from": "healthy", "to": "auth_failed"}),
        );
        // 三条都应能从 channel 拿到 (try_send 不阻塞)
        let e1 = rx.try_recv().expect("system_error 入队");
        assert_eq!(e1.kind, EventKind::SystemError);
        assert_eq!(e1.severity, Severity::Error);
        let e2 = rx.try_recv().expect("request 入队");
        assert_eq!(e2.kind, EventKind::Request);
        let e3 = rx.try_recv().expect("state_change 入队");
        assert_eq!(e3.kind, EventKind::SubscriptionStateChange);
        assert_eq!(e3.severity, Severity::Warn);
    }
}
