//! Tauri emit → broadcast → SSE 桥.
//!
//! bootstrap 时对 BRIDGED_EVENTS 逐个 `listen_any`, 把 payload (JSON 文本) 原样转投
//! broadcast; /ui/api/events 订阅后按 `event: <name>\ndata: <payload>` 推给浏览器.
//! 发射调用点一处不改; 新增事件名必须加进 BRIDGED_EVENTS (单测扫源码锁住).

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::Stream;
use tauri::{AppHandle, Listener};
use tokio::sync::broadcast::{self, error::RecvError};

use crate::state::AppState;

/// 一条转投给网页端的事件. payload 是 `app.emit` 时序列化后的 JSON 文本.
#[derive(Debug, Clone)]
pub struct UiEvent {
    pub name: String,
    pub payload: String,
}

/// broadcast 容量. 网页端消费慢时旧事件被覆盖 (RecvError::Lagged), SSE 侧跳过继续.
pub const UI_EVENT_CAPACITY: usize = 256;

/// 桌面前端 `listen()` 过的全部事件名. 顺序无意义.
pub const BRIDGED_EVENTS: &[&str] = &[
    "subscription_state_changed",
    "subscription_quota_reached",
    "log_write_failed",
    "event_log_write_failed",
    "events_flushed",
    "route_attempt_started",
    "route_attempt_finished",
    crate::commands::updater::PROGRESS_EVENT,
];

/// 在 bootstrap 里调一次. listen_any 的回调在 Tauri 事件线程上跑, 只做一次 send.
pub fn install_bridge(app: &AppHandle, tx: broadcast::Sender<UiEvent>) {
    for name in BRIDGED_EVENTS {
        let tx = tx.clone();
        let name_owned = (*name).to_string();
        app.listen_any(*name, move |event| {
            // 无订阅者时 send 返回 Err, 正常情况 (网页没打开), 忽略
            let _ = tx.send(UiEvent {
                name: name_owned.clone(),
                payload: event.payload().to_string(),
            });
        });
    }
}

/// 取下一条事件; Lagged 跳过继续, Closed 返回 None.
pub async fn next_event(
    mut rx: broadcast::Receiver<UiEvent>,
) -> Option<(UiEvent, broadcast::Receiver<UiEvent>)> {
    loop {
        match rx.recv().await {
            Ok(ev) => return Some((ev, rx)),
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => return None,
        }
    }
}

fn event_stream(
    rx: broadcast::Receiver<UiEvent>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    futures::stream::unfold(rx, |rx| async move {
        let (ev, rx) = next_event(rx).await?;
        Some((Ok(Event::default().event(ev.name).data(ev.payload)), rx))
    })
}

/// GET /ui/api/events
pub async fn sse_handler(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.ui_events.subscribe();
    Sse::new(event_stream(rx))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("keepalive"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// 扫描 src/ 下所有 Tauri 事件发射调用点 (含跨行空白), 每个字面量都必须在
    /// BRIDGED_EVENTS 里. 跳过 proxy/transform/ (那里的 self 版同名方法是 SSE
    /// 转换器内部方法, 不是 Tauri 事件). 需要 (needle) 在运行时拼出来, 避免
    /// 本文件自身包含那段字面量而把扫描器扫到自己身上.
    fn scan_emit_literals(dir: &Path, out: &mut Vec<String>) {
        let needle = format!(".{}(", "emit");
        for entry in std::fs::read_dir(dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                if p.ends_with("transform") {
                    continue;
                }
                scan_emit_literals(&p, out);
                continue;
            }
            if p.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let src = std::fs::read_to_string(&p).unwrap();
            let mut cursor = 0;
            while let Some(i) = src[cursor..].find(needle.as_str()) {
                let after = cursor + i + needle.len();
                let rest = src[after..].trim_start();
                if let Some(stripped) = rest.strip_prefix('"') {
                    if let Some(end) = stripped.find('"') {
                        out.push(stripped[..end].to_string());
                    }
                }
                cursor = after;
            }
        }
    }

    #[test]
    fn every_emitted_literal_is_bridged() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        scan_emit_literals(&root, &mut found);
        assert!(!found.is_empty(), "扫描器没找到任何发射调用字面量, 扫描逻辑坏了");
        for name in found {
            assert!(
                BRIDGED_EVENTS.contains(&name.as_str()),
                "事件 `{name}` 有发射调用但未加入 BRIDGED_EVENTS"
            );
        }
        assert!(BRIDGED_EVENTS.contains(&crate::commands::updater::PROGRESS_EVENT));
    }

    #[tokio::test]
    async fn next_event_skips_lagged_and_ends_on_close() {
        let (tx, rx) = tokio::sync::broadcast::channel::<UiEvent>(2);
        for i in 0..5 {
            let _ = tx.send(UiEvent {
                name: format!("e{i}"),
                payload: "null".into(),
            });
        }
        // 容量 2: rx 先收到 Lagged, 应跳过后拿到 e3
        let (ev, rx) = next_event(rx).await.expect("should yield after lag");
        assert_eq!(ev.name, "e3");
        let (ev, rx) = next_event(rx).await.unwrap();
        assert_eq!(ev.name, "e4");
        drop(tx);
        assert!(next_event(rx).await.is_none(), "sender 关闭后结束");
    }
}
