//! 批量写入请求日志（设计稿 §9.3）。
//!
//! 生产端：请求 pipeline 发送 `RequestLogEntry`。
//! 消费端：独立 tokio 任务累积到 50 条或 5 秒 flush 一次。

use std::collections::VecDeque;

use chrono::Utc;
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::virtual_model::VirtualModelName;

#[derive(Debug, Clone)]
pub struct RequestLogEntry {
    pub id: Uuid,
    pub timestamp_ms: i64,
    pub virtual_model_name: VirtualModelName,
    pub subscription_id: Uuid,
    pub provider_id: String,
    pub endpoint_id: String,
    pub real_model_name: String,
    /// 上游响应里的 message.model 原值(改写前)。错误/超时为 None。
    pub response_model_name: Option<String>,
    pub is_streaming: bool,
    pub status: RequestStatus,
    pub http_status: Option<u16>,
    pub ttft_ms: Option<u64>,
    pub total_latency_ms: Option<u64>,
    pub upstream_input_tokens: Option<u32>,
    pub upstream_output_tokens: Option<u32>,
    pub upstream_cache_creation: Option<u32>,
    pub upstream_cache_read: Option<u32>,
    pub retry_count: u32,
    pub error_message: Option<String>,
    /// 仅错误路径填充, 截断至 4KB
    pub upstream_response_body: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum RequestStatus {
    Success,
    Error,
    Timeout,
}

impl RequestStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Error => "error",
            Self::Timeout => "timeout",
        }
    }
}

const FLUSH_SIZE: usize = 50;
const FLUSH_INTERVAL: Duration = Duration::from_secs(5);
const BUFFER_MAX: usize = 1000;

pub async fn run_consumer(
    pool: SqlitePool,
    mut rx: mpsc::Receiver<RequestLogEntry>,
    app: AppHandle,
) {
    let mut buffer: VecDeque<RequestLogEntry> = VecDeque::with_capacity(FLUSH_SIZE);
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

async fn flush(pool: &SqlitePool, buffer: &mut VecDeque<RequestLogEntry>, app: &AppHandle) {
    if buffer.is_empty() {
        return;
    }
    let batch: Vec<RequestLogEntry> = buffer.drain(..).collect();
    debug!(count = batch.len(), "flushing request logs");

    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            warn!(?e, "无法开启事务, 放回 buffer");
            buffer.extend(batch);
            let _ = app.emit("log_write_failed", e.to_string());
            return;
        }
    };

    for entry in batch {
        let result = sqlx::query(
            "INSERT INTO requests (id, timestamp, virtual_model_name, subscription_id,
                provider_id, endpoint_id, real_model_name, response_model_name,
                is_streaming, status,
                http_status, ttft_ms, total_latency_ms,
                upstream_input_tokens, upstream_output_tokens,
                upstream_cache_creation, upstream_cache_read,
                retry_count, error_message, upstream_response_body)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(entry.id.to_string())
        .bind(entry.timestamp_ms)
        .bind(entry.virtual_model_name.as_str())
        .bind(entry.subscription_id.to_string())
        .bind(entry.provider_id)
        .bind(entry.endpoint_id)
        .bind(entry.real_model_name)
        .bind(entry.response_model_name)
        .bind(entry.is_streaming as i64)
        .bind(entry.status.as_str())
        .bind(entry.http_status.map(|v| v as i64))
        .bind(entry.ttft_ms.map(|v| v as i64))
        .bind(entry.total_latency_ms.map(|v| v as i64))
        .bind(entry.upstream_input_tokens.map(|v| v as i64))
        .bind(entry.upstream_output_tokens.map(|v| v as i64))
        .bind(entry.upstream_cache_creation.map(|v| v as i64))
        .bind(entry.upstream_cache_read.map(|v| v as i64))
        .bind(entry.retry_count as i64)
        .bind(entry.error_message)
        .bind(entry.upstream_response_body)
        .execute(&mut *tx)
        .await;
        if let Err(e) = result {
            warn!(?e, "写入单条请求日志失败");
        }
    }
    if let Err(e) = tx.commit().await {
        warn!(?e, "提交请求日志事务失败");
        let _ = app.emit("log_write_failed", e.to_string());
    }
}

pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}
