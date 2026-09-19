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

use crate::action::{Action, Cmd, Fetch, FetchData, Mutation, MutationOutcome, OverviewData};
use crate::app::App;
use crate::client::dto::{RefreshBalanceResult, RefreshModelsResult, Subscription, TestConnectionResult};
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

fn spawn_fetch(client: Arc<Client>, tx: UnboundedSender<Action>, fetch: Fetch, issued: u64) {
    tokio::spawn(async move {
        let result = match fetch {
            Fetch::Overview => fetch_overview(&client).await.map(|d| FetchData::Overview(Box::new(d))),
            Fetch::Subscriptions => client
                .call::<Vec<Subscription>>(commands::LIST_SUBSCRIPTIONS, json!({}))
                .await
                .map(FetchData::Subscriptions),
        };
        let result = result.map_err(|e: ClientError| e.to_string());
        let _ = tx.send(Action::FetchDone { fetch, issued, result });
    });
}

/// 测试连接要真的打一次上游, 60 秒超时; 刷新模型 / 余额各给 30 秒。
const TEST_CONNECTION_TIMEOUT: Duration = Duration::from_secs(60);
const REFRESH_TIMEOUT: Duration = Duration::from_secs(30);

/// 真正发出一次就地操作对应的 HTTP 调用, 按 [`Mutation`] 的种类分派到对应 command。
async fn call_mutation(client: &Client, mutation: &Mutation) -> Result<MutationOutcome, ClientError> {
    match mutation {
        Mutation::SetEnabled { id, enabled } => {
            client.call::<()>(commands::SET_SUBSCRIPTION_ENABLED, json!({ "id": id, "enabled": enabled })).await?;
            Ok(MutationOutcome::EnabledSet)
        }
        Mutation::TestConnection { id } => {
            let r: TestConnectionResult =
                client.call_with_timeout(commands::TEST_CONNECTION, json!({ "id": id }), TEST_CONNECTION_TIMEOUT).await?;
            Ok(MutationOutcome::Tested(r))
        }
        Mutation::RefreshModels { id } => {
            let r: RefreshModelsResult =
                client.call_with_timeout(commands::REFRESH_MODEL_LIST, json!({ "id": id }), REFRESH_TIMEOUT).await?;
            Ok(MutationOutcome::Models(r))
        }
        Mutation::RefreshBalance { id } => {
            let r: RefreshBalanceResult =
                client.call_with_timeout(commands::REFRESH_SUBSCRIPTION_BALANCE, json!({ "id": id }), REFRESH_TIMEOUT).await?;
            Ok(MutationOutcome::Balance(r))
        }
    }
}

/// **不去重、不补跑**: 每一个 `Cmd::Mutate` 都直接 `spawn` 一次 HTTP 调用, 不经过 [`Fetches`] ——
/// 与就地操作「同一订阅同时只跑一个」的语义完全由 `App` 的忙碌表在更上游把关, 这里只管发送。
/// 退出时仍在进行的变更不等待 (与加载一致, 任务被主循环结束时一并丢弃)。
fn spawn_mutation(client: Arc<Client>, tx: UnboundedSender<Action>, mutation: Mutation) {
    tokio::spawn(async move {
        let result = call_mutation(&client, &mutation).await.map_err(|e: ClientError| e.to_string());
        let _ = tx.send(Action::MutationDone { mutation, result });
    });
}

/// 主循环发起加载时盖的单调递增序号 (从 1 开始)。抽成独立结构体, 方便单测「递增」这一件事本身。
#[derive(Default)]
struct Issued(u64);

impl Issued {
    fn next(&mut self) -> u64 {
        self.0 += 1;
        self.0
    }
}

/// 跑一轮「连上事件流 → 转发事件, 直到断线」。返回值是这一轮事件流**实际存活了多久**——
/// 连接失败 / 超时是 `Duration::ZERO` (绝不会被当成「稳定过」), 连上了才从 `events()` 成功的
/// 那一刻开始计时到断线为止。`connect_deadline` 独立传参而不是直接读 `CONNECT_DEADLINE`,
/// 方便测试用一个远小于生产值的超时去戳一个卡住不响应的 mock, 不用真等 10 秒 (G4)。
///
/// `ever_connected` 只有真的连上过一次才置 true: 调用方靠它判断「从没连上时不发
/// `ConnectionLost`」(G6), 免得「从未连接」被 `App` 当成「掉线重连」。
async fn run_once(client: &Client, tx: &UnboundedSender<Action>, ever_connected: &mut bool, connect_deadline: Duration) -> Duration {
    // 建立连接本身也要有超时, 否则一个卡住不响应的上游会让这个任务永久挂起 (G4b)。
    let Ok(Ok(mut stream)) = tokio::time::timeout(connect_deadline, client.events()).await else {
        return Duration::ZERO;
    };
    *ever_connected = true;
    // 从连上的这一刻开始计时, 而不是从这一轮循环 (含连接排队 / 后面的退避 sleep) 开始算,
    // 否则「流活了多久」会把连接耗时和断线后的等待都算进去, next_attempt 判断全乱 (G4)。
    let up = tokio::time::Instant::now();
    let app_version = client.runtime().await.app_version;
    if tx.send(Action::Connected { app_version }).is_err() {
        return up.elapsed();
    }
    while let Ok(Some(ev)) = stream.next().await {
        if tx.send(Action::Sse { name: ev.name, data: ev.data }).is_err() {
            break;
        }
    }
    up.elapsed()
}

/// 常驻任务: 连事件流 → 转发事件 → 断了就退避重连。`Client` 在失败时会自己重读 runtime.json,
/// 所以 app 重启换了端口 / 密钥也能接上。
async fn sse_loop(client: Arc<Client>, tx: UnboundedSender<Action>) {
    let mut attempt = 0;
    let mut ever_connected = false;
    loop {
        let lived = run_once(&client, &tx, &mut ever_connected, CONNECT_DEADLINE).await;
        if ever_connected && tx.send(Action::ConnectionLost).is_err() {
            return;
        }
        tokio::time::sleep(backoff(attempt)).await;
        attempt = next_attempt(attempt, lived);
    }
}

/// 同一种加载同时只跑一个; 进行中又来了同种请求, 记一笔, 等这次回来后补跑一次 (G7:
/// 否则会静默丢掉一次刷新请求, 比如断线重连期间某订阅状态变了, 要等下一次 5s 轮询才补上)。
#[derive(Default)]
struct Fetches {
    in_flight: HashSet<Fetch>,
    rerun: HashSet<Fetch>,
}

impl Fetches {
    /// 要不要现在就发起? (false = 已有同种请求在跑, 已记下待补跑)
    fn request(&mut self, fetch: Fetch) -> bool {
        if self.in_flight.insert(fetch) {
            true
        } else {
            self.rerun.insert(fetch);
            false
        }
    }

    /// 一次加载回来了; 返回 true 表示要立刻补跑一次 (调用方负责 spawn)。
    fn finished(&mut self, fetch: Fetch) -> bool {
        self.in_flight.remove(&fetch);
        if self.rerun.remove(&fetch) {
            self.in_flight.insert(fetch);
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

/// 处理单个 [`Action`]: 先用它做 `Fetches` / `Issued` 的记账 (清「进行中」标记, 要补跑就以新的
/// 序号重新 spawn), 再喂给 `App::update` 把结果 `Cmd` 变成真正的副作用 (spawn 新加载 / 退出)。
/// 单个 action 的处理逻辑抽成这个函数, 供 `select!` 拿到的那一个和抽干队列时的每一个共用。
/// 返回 `true` 表示主循环该退出 (`Cmd::Quit`)。
fn process_action(
    action: Action,
    client: &Arc<Client>,
    tx: &UnboundedSender<Action>,
    fetches: &mut Fetches,
    issued: &mut Issued,
    app: &mut App,
) -> bool {
    if let Action::FetchDone { fetch, .. } = &action {
        if fetches.finished(*fetch) {
            spawn_fetch(client.clone(), tx.clone(), *fetch, issued.next());
        }
    }
    for cmd in app.update(action) {
        match cmd {
            Cmd::Quit => return true,
            Cmd::Fetch(fetch) => {
                if fetches.request(fetch) {
                    spawn_fetch(client.clone(), tx.clone(), fetch, issued.next());
                }
            }
            Cmd::Mutate(mutation) => spawn_mutation(client.clone(), tx.clone(), mutation),
        }
    }
    false
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
    // 每次真正 spawn 一个加载 (含补跑) 时盖的单调递增序号, 带进 `Action::FetchDone` 给 `Store`
    // 判断新旧。
    let mut issued = Issued::default();
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
        if process_action(action, &client, &tx, &mut fetches, &mut issued, app) {
            break 'outer;
        }
        // 抽干再画: select! 那一个处理完之后, 把这时已经排在队列里的 action 一并处理掉,
        // 再回到循环顶部画一帧 (为事件洪峰准备; 键盘与 tick 走 select! 的常规分支, 不受影响)。
        // **有界**: 只抽干「进入这段代码那一刻已经排队的那些」(`rx.len()` 那一刻的快照),
        // 不是无条件 `while let` —— 否则一个持续produce的生产者 (比如密集 SSE) 会让抽干永远
        // 抽不完, 一直不回到循环顶部, 键盘响应和下一帧重绘都被无限期推迟 (M5 fix round 1)。
        // 抽干过程中 `process_action` 触发的新 fetch 结果晚一点由下一轮循环处理, 不会丢。
        for _ in 0..rx.len() {
            match rx.try_recv() {
                Ok(a) => {
                    if process_action(a, &client, &tx, &mut fetches, &mut issued, app) {
                        break 'outer;
                    }
                }
                Err(_) => break,
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
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn backoff_is_1_2_5_then_flat() {
        let secs: Vec<u64> = (0..6).map(|a| backoff(a).as_secs()).collect();
        assert_eq!(secs, [1, 2, 5, 5, 5, 5]);
    }

    #[test]
    fn issued_numbers_are_strictly_increasing() {
        let mut issued = Issued::default();
        assert_eq!(issued.next(), 1);
        assert_eq!(issued.next(), 2);
        assert_eq!(issued.next(), 3);
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

    /// 咬住 G4 的 bug 场景: 连续几轮「连上即断」(`lived` 都是 `Duration::ZERO`, 对应连接失败 /
    /// 超时) 必须让退避一档一档往上走 (1s → 2s → 5s), 而不是每轮都被误判成「稳定过」而清零 ——
    /// 这正是 fix round 1 里量错区间导致的回归: 把 `Duration::ZERO` 之外的「连接耗时 + 退避
    /// sleep 时长」算进 `stream_lived`, 会让 `next_attempt` 提前判定为已恢复。
    #[test]
    fn consecutive_failures_back_off_1_2_5_via_next_attempt_wiring() {
        let mut attempt = 0;
        let mut backoffs = Vec::new();
        for lived in [Duration::ZERO, Duration::ZERO, Duration::ZERO] {
            backoffs.push(backoff(attempt).as_secs());
            attempt = next_attempt(attempt, lived);
        }
        assert_eq!(backoffs, [1, 2, 5]);
        assert_eq!(attempt, 3);
        // 这次流真住满了 STABLE_AFTER: 清零, 下一次断线又是从 1s 开始。
        attempt = next_attempt(attempt, STABLE_AFTER);
        assert_eq!(attempt, 0);
    }

    #[test]
    fn fetches_coalesces_concurrent_requests_of_the_same_kind() {
        let mut f = Fetches::default();
        assert!(f.request(Fetch::Overview)); // 第一次: 发起
        assert!(!f.request(Fetch::Overview)); // 还在跑: 只记一笔待补跑
        assert!(f.finished(Fetch::Overview)); // 回来了: 之前记的那笔要补跑
        assert!(!f.finished(Fetch::Overview)); // 这次是补跑的结果, 没有再记新的待补跑
    }

    #[test]
    fn fetches_unrelated_kinds_do_not_interfere() {
        let mut f = Fetches::default();
        assert!(f.request(Fetch::Overview));
        assert!(f.request(Fetch::Subscriptions)); // 不同种类互不影响
        assert!(!f.finished(Fetch::Subscriptions)); // 没有同种的待补跑
        assert!(!f.finished(Fetch::Overview));
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

    /// G4 回归: 直接测 `sse_loop` 内部真正喂给 `next_attempt` 的那个值, 而不只是 `next_attempt`
    /// 这个纯函数本身 —— fix round 1 的 bug 恰恰是「纯函数本身是对的, 喂给它的区间量错了」。
    #[tokio::test]
    async fn run_once_returns_zero_lived_duration_when_connect_fails() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/ui/api/events")).respond_with(ResponseTemplate::new(500)).mount(&server).await;
        let dir = tempfile::tempdir().unwrap();
        write_runtime(dir.path(), server.address().port(), "s", "9.9.9-test");
        let client = Client::connect(dir.path()).unwrap();
        let (tx, _rx) = unbounded_channel::<Action>();
        let mut ever_connected = false;

        let lived = run_once(&client, &tx, &mut ever_connected, Duration::from_secs(1)).await;

        assert_eq!(lived, Duration::ZERO);
        assert!(!ever_connected);
    }

    #[tokio::test]
    async fn run_once_returns_a_small_nonzero_lived_duration_for_a_finite_stream() {
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
        let client = Client::connect(dir.path()).unwrap();
        let (tx, mut rx) = unbounded_channel::<Action>();
        let mut ever_connected = false;

        let lived = run_once(&client, &tx, &mut ever_connected, Duration::from_secs(1)).await;

        assert!(ever_connected);
        assert!(lived < STABLE_AFTER, "一次快速的连上又断不该被算成「稳定过」, 实际 {lived:?}");
        assert_eq!(rx.recv().await, Some(Action::Connected { app_version: "9.9.9-test".into() }));
        assert_eq!(
            rx.recv().await,
            Some(Action::Sse { name: "subscription_state_changed".into(), data: "\"1\"".into() })
        );
    }

    /// 上游卡住不响应 (mock 延迟 2s) 时, `run_once` 必须按传入的 `connect_deadline` (这里给
    /// 200ms, 远小于生产的 10s) 及时放弃, 而不是真的等满 mock 的延迟——否则 G4b 的超时保护就是
    /// 摆设。外层再包一层 800ms 的 timeout 当安全网: 如果 `run_once` 真的没有遵守
    /// `connect_deadline`, 测试会在 800ms 处失败, 而不是真的挂等 2 秒。
    #[tokio::test]
    async fn run_once_treats_a_hanging_connect_as_zero_lived_once_the_deadline_hits() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/ui/api/events"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(2)))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        write_runtime(dir.path(), server.address().port(), "s", "9.9.9-test");
        let client = Client::connect(dir.path()).unwrap();
        let (tx, _rx) = unbounded_channel::<Action>();
        let mut ever_connected = false;

        let lived = tokio::time::timeout(Duration::from_millis(800), run_once(&client, &tx, &mut ever_connected, Duration::from_millis(200)))
            .await
            .expect("run_once 应该在 connect_deadline (200ms) 附近就返回, 不该等满 mock 的 2s 延迟");

        assert_eq!(lived, Duration::ZERO);
        assert!(!ever_connected);
    }

    /// 四种就地操作各自打对了 command、带对了 JSON 键名 (`id` / `enabled`, 与后端 `#[tauri::command]`
    /// 的参数名同名, camelCase 下与蛇形写法一致), 并且把响应体正确包进对应的 `MutationOutcome`。
    #[tokio::test]
    async fn a_mutation_calls_the_right_command_and_reports_back() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/ui/api/cmd/set_subscription_enabled"))
            .and(body_json(json!({"id": "1", "enabled": false})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!(null)))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/ui/api/cmd/test_connection"))
            .and(body_json(json!({"id": "1"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"ok": true, "message": "连接正常", "http_status": 200, "model_used": "glm-4.6", "state_reset": true}),
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/ui/api/cmd/refresh_model_list"))
            .and(body_json(json!({"id": "1"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"kind": "auto", "models": [], "fetched_at": 1})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/ui/api/cmd/refresh_subscription_balance"))
            .and(body_json(json!({"id": "1"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"kind": "unsupported"})))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        write_runtime(dir.path(), server.address().port(), "s", "9.9.9-test");
        let client = Arc::new(Client::connect(dir.path()).unwrap());
        let (tx, mut rx) = unbounded_channel::<Action>();

        spawn_mutation(client.clone(), tx.clone(), Mutation::SetEnabled { id: "1".into(), enabled: false });
        assert_eq!(
            rx.recv().await,
            Some(Action::MutationDone {
                mutation: Mutation::SetEnabled { id: "1".into(), enabled: false },
                result: Ok(MutationOutcome::EnabledSet),
            })
        );

        spawn_mutation(client.clone(), tx.clone(), Mutation::TestConnection { id: "1".into() });
        assert_eq!(
            rx.recv().await,
            Some(Action::MutationDone {
                mutation: Mutation::TestConnection { id: "1".into() },
                result: Ok(MutationOutcome::Tested(TestConnectionResult {
                    ok: true,
                    message: "连接正常".into(),
                    http_status: Some(200),
                    model_used: Some("glm-4.6".into()),
                    state_reset: true,
                })),
            })
        );

        spawn_mutation(client.clone(), tx.clone(), Mutation::RefreshModels { id: "1".into() });
        assert_eq!(
            rx.recv().await,
            Some(Action::MutationDone {
                mutation: Mutation::RefreshModels { id: "1".into() },
                result: Ok(MutationOutcome::Models(RefreshModelsResult::Auto { models: vec![], fetched_at: 1 })),
            })
        );

        spawn_mutation(client, tx, Mutation::RefreshBalance { id: "1".into() });
        assert_eq!(
            rx.recv().await,
            Some(Action::MutationDone {
                mutation: Mutation::RefreshBalance { id: "1".into() },
                result: Ok(MutationOutcome::Balance(RefreshBalanceResult::Unsupported)),
            })
        );
    }

    /// M7: `process_action` 是主循环真正的路由——`mutations_are_never_coalesced` 只调了
    /// `spawn_mutation` 本身, 哪怕以后有人手滑把 `Cmd::Mutate` 也接进 `Fetches` 的去重表, 那个测试
    /// 照样会通过, 咬不住这个回归。这里直接驱动 `process_action`:
    /// 1. 两次会各自让 `App::update` 产出 `Cmd::Fetch(Subscriptions)` 的 action → 只有一次真正的
    ///    HTTP 请求 (第二次被 `Fetches` 记成待补跑, 不再发)。
    /// 2. 那次请求的结果 (`Action::FetchDone`) 送进来 → `process_action` 内部的补跑逻辑再发一次,
    ///    带一个更大的 `issued`。
    /// 3. 三次 `Cmd::Mutate` (两条不同订阅 + 第三条对其中一条订阅再来一次同类操作但换成第三条
    ///    订阅) → 三次都应该真的发出 POST, 不经过 `Fetches` 的去重。
    /// 全程不 `sleep` 固定时长, 用「等 mock 收到的请求数到达预期」的轮询 + 3 秒超时兜底, 避免抖动。
    #[tokio::test]
    async fn process_action_dedupes_fetches_and_reruns_after_completion() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/ui/api/cmd/list_subscriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/ui/api/cmd/set_subscription_enabled"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!(null)))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        write_runtime(dir.path(), server.address().port(), "s", "9.9.9-test");
        let client = Arc::new(Client::connect(dir.path()).unwrap());
        let (tx, mut rx) = unbounded_channel::<Action>();
        let mut fetches = Fetches::default();
        let mut issued = Issued::default();
        let mut app = App::new(crate::app::AppOptions {
            strings: &crate::i18n::ZH,
            theme: crate::theme::Theme::new(crate::theme::ColorMode::TrueColor),
            fx_enabled: false,
            now_ms: 0,
            tui_version: "9.9.9-test",
        });
        app.update(crate::action::Action::SwitchTab(crate::action::Tab::Subscriptions));

        // 1) 两次都会产出 Cmd::Fetch(Subscriptions): `Connected` 与 `Refresh` 各自在订阅页的
        //    `update()` 里映射成一次 Fetch。第一次真的发; 这次请求还没回来时第二次应该被去重。
        assert!(!process_action(Action::Connected { app_version: "9.9.9-test".into() }, &client, &tx, &mut fetches, &mut issued, &mut app));
        assert!(!process_action(Action::Refresh, &client, &tx, &mut fetches, &mut issued, &mut app));

        tokio::time::timeout(Duration::from_secs(3), async {
            while count_requests(&server, "list_subscriptions").await < 1 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("第一次请求应该很快发出去");
        // 给「第二次会不会也发一次」留一点时间观察, 而不是立刻断言——去重发生在 `Fetches::request`
        // 同步返回的那一刻 (上面第二个 `process_action` 调用里), 这里只是确认没有多余的请求跟上来。
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(count_requests(&server, "list_subscriptions").await, 1, "第二次 Cmd::Fetch 应该被去重, 不是真的再发一次请求");

        // 2) 第一次的结果回来了: `process_action` 应该按上面记的那笔待补跑, 立刻用更大的 issued
        //    再发一次。
        let first_done = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await.expect("等第一次 FetchDone 超时").expect("channel 关闭了");
        let Action::FetchDone { issued: first_issued, .. } = &first_done else { panic!("{first_done:?}") };
        let first_issued = *first_issued;
        assert!(!process_action(first_done, &client, &tx, &mut fetches, &mut issued, &mut app));

        let rerun_done = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await.expect("等补跑的 FetchDone 超时").expect("channel 关闭了");
        let Action::FetchDone { issued: rerun_issued, .. } = &rerun_done else { panic!("{rerun_done:?}") };
        assert!(*rerun_issued > first_issued, "补跑应该带一个比第一次更大的 issued");
        assert!(!process_action(rerun_done, &client, &tx, &mut fetches, &mut issued, &mut app));

        assert_eq!(count_requests(&server, "list_subscriptions").await, 2, "总共应该只发生过两次 list_subscriptions 请求: 第一次 + 补跑");

        // 3) 三次 Cmd::Mutate (两条不同订阅 + 第三条订阅上同一种操作) 都应该真的发出去, 不经过
        //    `Fetches` 的去重——就地操作永不去重、永不补跑 (与 `Fetch` 相反)。
        assert!(!process_action(
            Action::Mutate(Mutation::SetEnabled { id: "a".into(), enabled: false }),
            &client,
            &tx,
            &mut fetches,
            &mut issued,
            &mut app
        ));
        assert!(!process_action(
            Action::Mutate(Mutation::SetEnabled { id: "b".into(), enabled: false }),
            &client,
            &tx,
            &mut fetches,
            &mut issued,
            &mut app
        ));
        assert!(!process_action(
            Action::Mutate(Mutation::SetEnabled { id: "c".into(), enabled: false }),
            &client,
            &tx,
            &mut fetches,
            &mut issued,
            &mut app
        ));

        tokio::time::timeout(Duration::from_secs(3), async {
            while count_requests(&server, "set_subscription_enabled").await < 3 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("三次 Cmd::Mutate 都应该真的各自发出一次 POST");
    }

    /// 数 mock server 收到的、路径以 `suffix` 结尾的请求条数 (供上面这条测试轮询用, 不固定
    /// `sleep` 时长)。
    async fn count_requests(server: &MockServer, suffix: &str) -> usize {
        server.received_requests().await.unwrap().iter().filter(|r| r.url.path().ends_with(suffix)).count()
    }

    /// 就地操作绝不经过 `Fetches` 的「同类只跑一个」去重: 同一个 `Cmd::Mutate` 连发两次必须真的
    /// 打两次上游 (`.expect(2)` 在 `MockServer` drop 时校验)。忙碌表「同一订阅同时只跑一个」的
    /// 语义是 `App` 更上游的把关, 与这里「runtime 层不去重」是两回事。
    #[tokio::test]
    async fn mutations_are_never_coalesced() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/ui/api/cmd/test_connection"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"ok": true, "message": "ok", "http_status": 200, "model_used": null, "state_reset": false}),
            ))
            .expect(2)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        write_runtime(dir.path(), server.address().port(), "s", "9.9.9-test");
        let client = Arc::new(Client::connect(dir.path()).unwrap());
        let (tx, mut rx) = unbounded_channel::<Action>();

        let mutation = Mutation::TestConnection { id: "1".into() };
        spawn_mutation(client.clone(), tx.clone(), mutation.clone());
        spawn_mutation(client, tx, mutation);

        assert!(rx.recv().await.is_some());
        assert!(rx.recv().await.is_some());
    }
}
