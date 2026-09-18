//! 主循环: 把键盘、250ms 定时器、事件流、后台 HTTP 任务的结果汇成 [`Action`] 喂给 [`App`],
//! 再把 `App` 吐出来的 [`Cmd`] 变成后台任务。界面从不等网络: 所有 HTTP 调用都 `spawn` 出去。
//!
//! **双档渲染**: 平时只在有事件时重画 (空闲 CPU 接近零); 有动效在播时每 16ms 画一帧, 播完自动回落。

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use ratatui::crossterm::event::{Event, EventStream};
use serde_json::json;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use crate::action::{Action, Cmd, OverviewData};
use crate::app::App;
use crate::client::dto::Subscription;
use crate::client::{commands, Client, ClientError};

const TICK: Duration = Duration::from_millis(250);
const FRAME: Duration = Duration::from_millis(16);

pub fn unix_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// 事件流断线后的重连间隔: 1s → 2s → 5s, 之后一直 5s (spec §4.4)。
pub fn backoff(attempt: u32) -> Duration {
    Duration::from_secs(match attempt {
        0 => 1,
        1 => 2,
        _ => 5,
    })
}

async fn fetch_overview(client: &Client) -> Result<OverviewData, ClientError> {
    let today = json!({ "range": "today" });
    let (status, settings, stats, series, subscriptions) = tokio::try_join!(
        client.call(commands::PROXY_STATUS, json!({})),
        client.call(commands::GET_SETTINGS, json!({})),
        client.call(commands::GET_OVERALL_STATS, today.clone()),
        client.call(commands::GET_DAILY_SERIES, today.clone()),
        client.call(commands::LIST_SUBSCRIPTIONS, json!({})),
    )?;
    Ok(OverviewData { status, settings, stats, series, subscriptions })
}

fn spawn_fetch(client: Arc<Client>, tx: UnboundedSender<Action>, cmd: Cmd) {
    tokio::spawn(async move {
        let result = match cmd {
            Cmd::FetchOverview => fetch_overview(&client).await.map(|d| Action::OverviewLoaded(Box::new(d))),
            Cmd::FetchSubscriptions => client
                .call::<Vec<Subscription>>(commands::LIST_SUBSCRIPTIONS, json!({}))
                .await
                .map(Action::SubscriptionsLoaded),
            Cmd::Quit => return,
        };
        let _ = tx.send(result.unwrap_or_else(|e| Action::LoadFailed { cmd, message: e.to_string() }));
    });
}

/// 常驻任务: 连事件流 → 转发事件 → 断了就退避重连。`Client` 在失败时会自己重读 runtime.json,
/// 所以 app 重启换了端口 / 密钥也能接上。
async fn sse_loop(client: Arc<Client>, tx: UnboundedSender<Action>) {
    let mut attempt = 0;
    loop {
        if let Ok(mut stream) = client.events().await {
            attempt = 0;
            let app_version = client.runtime().await.app_version;
            if tx.send(Action::Connected { app_version }).is_err() {
                return;
            }
            while let Ok(Some(ev)) = stream.next().await {
                if tx.send(Action::Sse { name: ev.name, data: ev.data }).is_err() {
                    return;
                }
            }
        }
        if tx.send(Action::ConnectionLost).is_err() {
            return;
        }
        tokio::time::sleep(backoff(attempt)).await;
        attempt += 1;
    }
}

/// 哪个 [`Cmd`] 的结果回来了 (用来清「进行中」标记)。
fn finished(action: &Action) -> Option<Cmd> {
    match action {
        Action::OverviewLoaded(_) => Some(Cmd::FetchOverview),
        Action::SubscriptionsLoaded(_) => Some(Cmd::FetchSubscriptions),
        Action::LoadFailed { cmd, .. } => Some(*cmd),
        _ => None,
    }
}

pub async fn run(client: Arc<Client>, mut app: App) -> std::io::Result<()> {
    // init() 进入备用屏幕 + raw mode, 并装好 panic 钩子 (panic 时先恢复终端再打印)。
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, client, &mut app).await;
    ratatui::restore();
    result
}

async fn event_loop(terminal: &mut ratatui::DefaultTerminal, client: Arc<Client>, app: &mut App) -> std::io::Result<()> {
    let (tx, mut rx) = unbounded_channel::<Action>();
    let sse = tokio::spawn(sse_loop(client.clone(), tx.clone()));

    let mut keys = EventStream::new();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // 同一种加载同时只跑一个: 后端慢的时候轮询不会越积越多。
    let mut in_flight: HashSet<Cmd> = HashSet::new();
    let mut last_frame = Instant::now();

    'outer: loop {
        let now = Instant::now();
        let elapsed = now - last_frame;
        last_frame = now;
        terminal.draw(|frame| app.draw(frame, elapsed))?;

        let fast = app.wants_fast_frames();
        let action = tokio::select! {
            ev = keys.next() => match ev {
                Some(Ok(Event::Key(key))) => app.handle_key(key),
                Some(Ok(_)) => None, // Resize 等: 回到循环顶部重画即可
                Some(Err(_)) | None => Some(Action::Quit),
            },
            _ = tick.tick() => Some(Action::Tick { now_ms: unix_ms() }),
            Some(action) = rx.recv() => Some(action),
            _ = tokio::time::sleep(FRAME), if fast => None,
        };

        let Some(action) = action else { continue };
        if let Some(done) = finished(&action) {
            in_flight.remove(&done);
        }
        for cmd in app.update(action) {
            match cmd {
                Cmd::Quit => break 'outer,
                fetch => {
                    if in_flight.insert(fetch) {
                        spawn_fetch(client.clone(), tx.clone(), fetch);
                    }
                }
            }
        }
    }
    sse.abort();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_1_2_5_then_flat() {
        let secs: Vec<u64> = (0..6).map(|a| backoff(a).as_secs()).collect();
        assert_eq!(secs, [1, 2, 5, 5, 5, 5]);
    }

    #[test]
    fn finished_maps_results_back_to_their_cmd() {
        assert_eq!(finished(&Action::SubscriptionsLoaded(vec![])), Some(Cmd::FetchSubscriptions));
        assert_eq!(
            finished(&Action::LoadFailed { cmd: Cmd::FetchOverview, message: String::new() }),
            Some(Cmd::FetchOverview)
        );
        assert_eq!(finished(&Action::Refresh), None);
    }
}
