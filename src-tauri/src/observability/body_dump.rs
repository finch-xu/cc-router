//! 调试模式下三段 body 落盘（debug-dumps）。
//!
//! 当用户在设置里开启 `debug_mode` 后, pipeline / sse 在每次出站 attempt 处把:
//! - 客户端发到 cc-router 的原始请求体
//! - cc-router 改写后真正发给上游的请求体
//! - 上游真实响应体（流式 = raw SSE 字节流; 非流式 = JSON）
//!
//! 通过 mpsc channel 投递到本模块 consumer, 每段写一个 .txt 文件到
//! `<app_data_dir>/debug-dumps/`. 文件名格式:
//!
//! ```text
//! YYYY-MM-DD_HH-MM-SS-mmm_<attempt_uuid>_<kind>.txt
//! ```
//!
//! 字典序 = 时间序; attempt_uuid 关联同一次 attempt 的三件套; kind 是
//! `client_request` / `upstream_request` / `upstream_response`.
//!
//! 写盘失败只 `warn!` 不影响主流程, consumer 不停, 与现有 request_log 同哲学.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use tokio::fs;
use tokio::sync::mpsc;
use tracing::warn;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyDumpKind {
    Client,
    UpstreamRequest,
    UpstreamResponse,
}

impl BodyDumpKind {
    fn suffix(&self) -> &'static str {
        match self {
            BodyDumpKind::Client => "client_request",
            BodyDumpKind::UpstreamRequest => "upstream_request",
            BodyDumpKind::UpstreamResponse => "upstream_response",
        }
    }
}

#[derive(Debug)]
pub struct BodyDumpEntry {
    pub attempt_id: Uuid,
    pub kind: BodyDumpKind,
    pub timestamp: DateTime<Local>,
    pub body: Vec<u8>,
}

impl BodyDumpEntry {
    pub fn new(attempt_id: Uuid, kind: BodyDumpKind, body: Vec<u8>) -> Self {
        Self {
            attempt_id,
            kind,
            timestamp: Local::now(),
            body,
        }
    }
}

pub fn dump_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("debug-dumps")
}

/// 时间戳放最前: 字典序 = 时间序, 用户在 Finder 里按 name 排序就能找最新一条.
fn build_filename(entry: &BodyDumpEntry) -> String {
    format!(
        "{}_{}_{}.txt",
        entry.timestamp.format("%Y-%m-%d_%H-%M-%S-%3f"),
        entry.attempt_id,
        entry.kind.suffix(),
    )
}

/// 后台 consumer: 串行接 entry → 写文件. 启动时主动 `create_dir_all` 兜底.
pub async fn run_consumer(mut rx: mpsc::Receiver<BodyDumpEntry>, dir: PathBuf) {
    if let Err(e) = fs::create_dir_all(&dir).await {
        warn!(?e, ?dir, "无法创建 debug-dumps 目录, body dump 将逐条尝试");
    }

    while let Some(entry) = rx.recv().await {
        let path = dir.join(build_filename(&entry));
        if let Err(e) = fs::write(&path, &entry.body).await {
            warn!(?e, ?path, "写 debug dump 失败, 跳过该条");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use tempfile::TempDir;

    fn fixed_timestamp() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 4, 30, 15, 23, 45).unwrap()
            + chrono::Duration::milliseconds(123)
    }

    #[test]
    fn filename_format_includes_timestamp_uuid_kind() {
        let entry = BodyDumpEntry {
            attempt_id: Uuid::nil(),
            kind: BodyDumpKind::Client,
            timestamp: fixed_timestamp(),
            body: b"x".to_vec(),
        };
        let name = build_filename(&entry);
        assert!(name.starts_with("2026-04-30_15-23-45-123_"));
        assert!(name.contains(&Uuid::nil().to_string()));
        assert!(name.ends_with("_client_request.txt"));
    }

    #[test]
    fn each_kind_uses_distinct_suffix() {
        assert_eq!(BodyDumpKind::Client.suffix(), "client_request");
        assert_eq!(BodyDumpKind::UpstreamRequest.suffix(), "upstream_request");
        assert_eq!(BodyDumpKind::UpstreamResponse.suffix(), "upstream_response");
    }

    #[tokio::test]
    async fn consumer_writes_three_files_for_one_attempt() {
        let dir = TempDir::new().unwrap();
        let dump = dir.path().join("debug-dumps");

        let (tx, rx) = mpsc::channel::<BodyDumpEntry>(8);
        let dump_clone = dump.clone();
        let handle = tokio::spawn(async move {
            run_consumer(rx, dump_clone).await;
        });

        let attempt = Uuid::new_v4();
        for kind in [
            BodyDumpKind::Client,
            BodyDumpKind::UpstreamRequest,
            BodyDumpKind::UpstreamResponse,
        ] {
            tx.send(BodyDumpEntry::new(attempt, kind, b"payload".to_vec()))
                .await
                .unwrap();
        }
        drop(tx);
        handle.await.unwrap();

        let mut entries: Vec<_> = std::fs::read_dir(&dump)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        entries.sort();
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().any(|n| n.ends_with("_client_request.txt")));
        assert!(entries.iter().any(|n| n.ends_with("_upstream_request.txt")));
        assert!(entries.iter().any(|n| n.ends_with("_upstream_response.txt")));

        for name in &entries {
            assert!(name.contains(&attempt.to_string()));
        }
    }

    #[tokio::test]
    async fn consumer_survives_write_failure() {
        // 用一个已存在的"文件"路径冒充目录, 让 create_dir_all + write 都失败,
        // 验证 consumer 不 panic 也不 break 还能继续吃下一条.
        let tmp = TempDir::new().unwrap();
        let bad_path = tmp.path().join("not-a-dir");
        std::fs::write(&bad_path, b"i am a file not a dir").unwrap();

        let (tx, rx) = mpsc::channel::<BodyDumpEntry>(4);
        let bad_clone = bad_path.clone();
        let handle = tokio::spawn(async move {
            run_consumer(rx, bad_clone).await;
        });

        // 投两条, 都会 write 失败但 consumer 不应退出
        tx.send(BodyDumpEntry::new(
            Uuid::new_v4(),
            BodyDumpKind::Client,
            b"x".to_vec(),
        ))
        .await
        .unwrap();
        tx.send(BodyDumpEntry::new(
            Uuid::new_v4(),
            BodyDumpKind::UpstreamResponse,
            b"y".to_vec(),
        ))
        .await
        .unwrap();
        // 关 tx → consumer 的 rx.recv 返 None → loop 自然退出
        drop(tx);

        // 没有 panic 即视为通过
        handle.await.unwrap();
    }

    #[test]
    fn entry_new_sets_now_timestamp() {
        let before = Local::now();
        let entry = BodyDumpEntry::new(Uuid::new_v4(), BodyDumpKind::Client, vec![]);
        let after = Local::now();
        assert!(entry.timestamp >= before);
        assert!(entry.timestamp <= after);
    }
}
