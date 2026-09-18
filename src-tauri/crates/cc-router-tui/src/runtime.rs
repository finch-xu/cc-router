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
/// 事件流连上住满这么久, 断线重连计数器才清零; 刚连上就断不算「恢复」, 不能让退避失效 (G4a)。
const STABLE_AFTER: Duration = Duration::from_secs(10);
/// 等 `events()` 建立连接 (拿到响应头) 的上限; 网络卡住不能让重连无限期挂起 (G4b)。
const CONNECT_DEADLINE: Duration = Duration::from_secs(10);

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

/// 断线重连计数器的下一个值: 这一轮连接住满 `STABLE_AFTER` 才清零 (真的恢复了才重置退避),
/// 否则接着累加 —— 刚连上就断的抖动连接不会让退避一直停在 1s (G4a)。
fn next_attempt(attempt: u32, stream_lived: Duration) -> u32 {
    if stream_lived >= STABLE_AFTER {
        0
    } else {
        attempt + 1
    }
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
///
/// `ever_connected` 只有真的连上过一次才置 true: 从没连上时不发 `ConnectionLost` (G6), 免得
/// 「从未连接」被 `App` 当成「掉线重连」, 首次连上时弹一条不存在的「已重新连接」 toast。
async fn sse_loop(client: Arc<Client>, tx: UnboundedSender<Action>) {
    let mut attempt = 0;
    let mut ever_connected = false;
    loop {
        let started = Instant::now();
        // 建立连接本身也要有超时, 否则一个卡住不响应的上游会让这个任务永久挂起 (G4b)。
        if let Ok(Ok(mut stream)) = tokio::time::timeout(CONNECT_DEADLINE, client.events()).await {
            ever_connected = true;
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
        if ever_connected && tx.send(Action::ConnectionLost).is_err() {
            return;
        }
        tokio::time::sleep(backoff(attempt)).await;
        attempt = next_attempt(attempt, started.elapsed());
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

/// 同一种加载同时只跑一个; 进行中又来了同种请求, 记一笔, 等这次回来后补跑一次 (G7:
/// 否则会静默丢掉一次刷新请求, 比如断线重连期间某订阅状态变了, 要等下一次 5s 轮询才补上)。
#[derive(Default)]
struct Fetches {
    in_flight: HashSet<Cmd>,
    rerun: HashSet<Cmd>,
}

impl Fetches {
    /// 要不要现在就发起? (false = 已有同种请求在跑, 已记下待补跑)
    fn request(&mut self, cmd: Cmd) -> bool {
        if self.in_flight.insert(cmd) {
            true
        } else {
            self.rerun.insert(cmd);
            false
        }
    }

    /// 一次加载回来了; 返回 true 表示要立刻补跑一次 (调用方负责 spawn)。
    fn finished(&mut self, cmd: Cmd) -> bool {
        self.in_flight.remove(&cmd);
        if self.rerun.remove(&cmd) {
            self.in_flight.insert(cmd);
            true
        } else {
            false
        }
    }
}

pub async fn run(client: Arc<Client>, mut app: App) -> std::io::Result<()> {
    // try_init() 进入备用屏幕 + raw mode, 并装好 panic 钩子 (panic 时先恢复终端再打印)。
    // 用 try_init 而不是 init(): 没有可用终端 (无 tty, 比如 stdin/stdout 都不是终端) 时返回
    // Err 而不是 panic, 这样 main.rs 能走统一的「一行提示 + exit 1」路径, 而不是 panic 的
    // exit 101 + backtrace。
    let mut terminal = ratatui::try_init()?;
    let result = event_loop(&mut terminal, client, &mut app).await;
    ratatui::restore();
    result
}

async fn event_loop(terminal: &mut ratatui::DefaultTerminal, client: Arc<Client>, app: &mut App) -> std::io::Result<()> {
    let (tx, mut rx) = unbounded_channel::<Action>();
    let mut sse = tokio::spawn(sse_loop(client.clone(), tx.clone()));
    // sse_loop 正常情况下永远不返回; 它结束了 (panic 或者提前 return) 说明事件流彻底死了,
    // 得让界面知道, 不然「已连接」会永远挂在那 (G5)。一个已经 ready 过的 JoinHandle 不能
    // 再被 poll, 所以用这个 bool 守卫 select! 分支, 命中一次之后就不再选它。
    let mut sse_alive = true;

    let mut keys = EventStream::new();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // 同一种加载同时只跑一个: 后端慢的时候轮询不会越积越多。
    let mut fetches = Fetches::default();
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
            res = &mut sse, if sse_alive => {
                sse_alive = false;
                let _ = res; // Ok(()) 正常退出 (理论上不会发生) 或 Err(JoinError) panic 了, 两种都当断线处理
                Some(Action::ConnectionLost)
            }
            _ = tokio::time::sleep(FRAME), if fast => None,
        };

        let Some(action) = action else { continue };
        if let Some(done) = finished(&action) {
            if fetches.finished(done) {
                spawn_fetch(client.clone(), tx.clone(), done);
            }
        }
        for cmd in app.update(action) {
            match cmd {
                Cmd::Quit => break 'outer,
                fetch => {
                    if fetches.request(fetch) {
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
    use crate::client::discovery::RUNTIME_FILE;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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

    #[test]
    fn next_attempt_resets_only_after_a_stable_connection() {
        // 刚连上就断 (远小于 STABLE_AFTER): 接着累加, 不清零。
        assert_eq!(next_attempt(0, Duration::from_millis(1)), 1);
        assert_eq!(next_attempt(3, Duration::from_secs(1)), 4);
        assert_eq!(next_attempt(3, STABLE_AFTER - Duration::from_millis(1)), 4);
        // 住满 STABLE_AFTER (含边界) 才算真的恢复了。
        assert_eq!(next_attempt(3, STABLE_AFTER), 0);
        assert_eq!(next_attempt(5, Duration::from_secs(20)), 0);
    }

    #[test]
    fn fetches_coalesces_concurrent_requests_of_the_same_kind() {
        let mut f = Fetches::default();
        assert!(f.request(Cmd::FetchOverview)); // 第一次: 发起
        assert!(!f.request(Cmd::FetchOverview)); // 还在跑: 只记一笔待补跑
        assert!(f.finished(Cmd::FetchOverview)); // 回来了: 之前记的那笔要补跑
        assert!(!f.finished(Cmd::FetchOverview)); // 这次是补跑的结果, 没有再记新的待补跑
    }

    #[test]
    fn fetches_unrelated_kinds_do_not_interfere() {
        let mut f = Fetches::default();
        assert!(f.request(Cmd::FetchOverview));
        assert!(f.request(Cmd::FetchSubscriptions)); // 不同种类互不影响
        assert!(!f.finished(Cmd::FetchSubscriptions)); // 没有同种的待补跑
        assert!(!f.finished(Cmd::FetchOverview));
    }

    fn write_runtime(dir: &std::path::Path, port: u16, secret: &str, app_version: &str) {
        let body = serde_json::json!({
            "pid": 1,
            "app_version": app_version,
            "http_port": port,
            "https_port": null,
            "ca_pem_path": null,
            "local_secret": secret,
        });
        std::fs::write(dir.join(RUNTIME_FILE), body.to_string()).unwrap();
    }

    #[tokio::test]
    async fn sse_loop_reports_connected_then_events_then_lost() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/ui/api/events"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                b"event: subscription_state_changed\ndata: \"1\"\n\n".to_vec(),
                "text/event-stream",
            ))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        write_runtime(dir.path(), server.address().port(), "s", "9.9.9-test");
        let client = Arc::new(Client::connect(dir.path()).unwrap());

        let (tx, mut rx) = unbounded_channel::<Action>();
        let task = tokio::spawn(sse_loop(client, tx));

        assert_eq!(rx.recv().await, Some(Action::Connected { app_version: "9.9.9-test".into() }));
        assert_eq!(
            rx.recv().await,
            Some(Action::Sse { name: "subscription_state_changed".into(), data: "\"1\"".into() })
        );
        // 上游发完这一条就关闭了连接 (set_body_raw 是有限响应体) → 下一次 next() 读到 EOF。
        assert_eq!(rx.recv().await, Some(Action::ConnectionLost));

        task.abort();
    }

    /// G6: 从没连上过时不该报「掉线」——那会让 `App` 把「从未连接」误判成「重连中」,
    /// 首次连上瞬间弹出不存在的「已重新连接」 toast。
    #[tokio::test]
    async fn never_connected_does_not_report_a_lost_connection() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/ui/api/events")).respond_with(ResponseTemplate::new(500)).mount(&server).await;
        let dir = tempfile::tempdir().unwrap();
        write_runtime(dir.path(), server.address().port(), "s", "9.9.9-test");
        let client = Arc::new(Client::connect(dir.path()).unwrap());

        let (tx, mut rx) = unbounded_channel::<Action>();
        let task = tokio::spawn(sse_loop(client, tx));

        // 等过至少一轮 backoff (第一档 1s) 之后仍然没有任何 action —— 既没有 Connected,
        // 也没有 ConnectionLost。
        let seen = tokio::time::timeout(Duration::from_millis(1500), rx.recv()).await;
        assert!(seen.is_err(), "从未连上不该发出任何 action, 实际收到 {seen:?}");

        task.abort();
    }
}
