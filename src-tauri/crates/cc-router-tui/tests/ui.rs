//! 界面测试: `App` 的更新逻辑 + `TestBackend` 渲染结果。不碰网络、不碰真实终端。
//!
//! 快照在 `tests/snapshots/`。改了布局后先看 diff 再接受:
//!   INSTA_UPDATE=always cargo test -p cc-router-tui --test ui
//! 动效在快照里一律关闭 (`fx_enabled: false`), 否则第一帧是启动动效的中间态。

use std::time::Duration;

use cc_router_tui::action::{Action, Cmd, OverviewData};
use cc_router_tui::app::{App, AppOptions};
use cc_router_tui::client::dto::{
    OverallStats, ProxyStatus, QuotaPeriod, QuotaUsage, SeriesPoint, Settings, Subscription, SubscriptionState,
};
use cc_router_tui::i18n::ZH;
use cc_router_tui::theme::{ColorMode, Theme};
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use ratatui::Terminal;

const NOW: i64 = 1_700_000_000_000;
const VERSION: &str = "9.9.9";

fn app(fx_enabled: bool) -> App {
    App::new(AppOptions {
        strings: &ZH,
        theme: Theme::new(ColorMode::TrueColor),
        fx_enabled,
        now_ms: NOW,
        tui_version: VERSION,
    })
}

fn quota(limit: u64, used: u64) -> QuotaUsage {
    QuotaUsage {
        period: QuotaPeriod::Daily,
        limit: Some(limit),
        input: used,
        output: 0,
        cache_creation: 0,
        cache_read: 0,
        exceeded: used >= limit,
    }
}

fn sub(id: &str, name: &str, state: SubscriptionState) -> Subscription {
    Subscription {
        id: id.into(),
        display_name: name.into(),
        provider_display_name: "p".into(),
        enabled: true,
        state,
        cooldown_until: None,
        last_error_message: None,
        is_dispatchable: state == SubscriptionState::Healthy,
        quota_usage: vec![],
    }
}

fn data() -> OverviewData {
    let mut zhipu = sub("1", "智谱主号", SubscriptionState::Healthy);
    zhipu.quota_usage = vec![quota(100, 62)];
    let mut kimi = sub("2", "Kimi 备用", SubscriptionState::RateLimited);
    kimi.cooldown_until = Some(NOW + 42_000);
    kimi.quota_usage = vec![quota(100, 91)];
    let relay = sub("3", "示例中转", SubscriptionState::AuthFailed);
    let mut off = sub("4", "停用的订阅", SubscriptionState::Healthy);
    off.enabled = false;
    off.is_dispatchable = false;
    OverviewData {
        status: ProxyStatus {
            running: true,
            mode: "http".into(),
            http_port: Some(23456),
            https_port: None,
            listen_all: false,
            base_url: "http://127.0.0.1:23456".into(),
        },
        settings: Settings { preferred_language: "zh".into(), tui_enabled: true, auth_enabled: true },
        stats: OverallStats {
            total_requests: 1284,
            success_rate_pct: 98.6,
            total_input_tokens: 3_000_000,
            total_output_tokens: 200_000,
            total_cache_creation_tokens: 0,
            total_cache_read_tokens: 0,
        },
        series: (0..24).map(|h| SeriesPoint { hour: Some(h), request_count: (h - 12).abs() * 3 + 1 }).collect(),
        // 故意把停用的放最前、出问题的放后面: 页面要自己按严重程度排。
        subscriptions: vec![off, zhipu, kimi, relay],
    }
}

fn loaded(fx_enabled: bool) -> App {
    let mut a = app(fx_enabled);
    assert_eq!(a.update(Action::Connected { app_version: VERSION.into() }), vec![Cmd::FetchOverview]);
    assert!(a.update(Action::OverviewLoaded(Box::new(data()))).is_empty());
    a
}

fn render_with(a: &mut App, width: u16, height: u16, elapsed: Duration) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| a.draw(f, elapsed)).unwrap();
    terminal.backend().to_string()
}

fn render(a: &mut App, width: u16, height: u16) -> String {
    render_with(a, width, height, Duration::ZERO)
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

// ---------- 快照 ----------

#[test]
fn overview_80x24() {
    insta::assert_snapshot!(render(&mut loaded(false), 80, 24));
}

#[test]
fn overview_120x40() {
    insta::assert_snapshot!(render(&mut loaded(false), 120, 40));
}

#[test]
fn help_popup_80x24() {
    let mut a = loaded(false);
    a.update(Action::ToggleHelp);
    insta::assert_snapshot!(render(&mut a, 80, 24));
}

#[test]
fn placeholder_page_80x24() {
    let mut a = loaded(false);
    a.update(Action::SwitchTab(1));
    insta::assert_snapshot!(render(&mut a, 80, 24));
}

// ---------- 渲染内容 ----------

#[test]
fn overview_shows_the_numbers_and_sorts_broken_first() {
    let out = render(&mut loaded(false), 80, 24);
    for needle in ["1,284", "98.6%", "3.2M", "http://127.0.0.1:23456", "鉴权 开启", "已连接", "v9.9.9"] {
        assert!(out.contains(needle), "缺 {needle:?}\n{out}");
    }
    // 4 个订阅里只有「智谱主号」可调度
    assert!(out.contains("4 个订阅 · 1 个可调度"), "{out}");
    assert!(out.contains("限流 · 00:42"), "{out}");
    let pos = |needle: &str| out.find(needle).unwrap_or_else(|| panic!("缺 {needle}\n{out}"));
    assert!(pos("Kimi 备用") < pos("智谱主号"), "出问题的排在可调度的前面");
    assert!(pos("智谱主号") < pos("停用的订阅"), "手动停用的排最后");
    assert!(out.contains("62%") && out.contains("91%"), "{out}");
}

#[test]
fn before_the_first_load_it_says_connecting_and_loading() {
    let out = render(&mut app(false), 80, 24);
    assert!(out.contains(ZH.conn_connecting), "{out}");
    assert!(out.contains(ZH.loading), "{out}");
}

#[test]
fn small_terminal_only_shows_the_hint() {
    for (w, h) in [(79, 24), (80, 23)] {
        let out = render(&mut loaded(false), w, h);
        assert!(out.contains("请放大终端窗口"), "{w}x{h}\n{out}");
        assert!(!out.contains("已连接"), "{w}x{h} 不应该再画外壳");
    }
}

#[test]
fn version_mismatch_shows_a_banner() {
    let mut a = app(false);
    a.update(Action::Connected { app_version: "1.2.3".into() });
    let out = render(&mut a, 120, 30);
    assert!(out.contains("9.9.9") && out.contains("1.2.3") && out.contains('⚠'), "{out}");
    assert!(!render(&mut loaded(false), 120, 30).contains('⚠'));
}

#[test]
fn long_subscription_list_is_truncated_with_a_count() {
    let mut a = app(false);
    a.update(Action::Connected { app_version: VERSION.into() });
    let mut d = data();
    d.subscriptions = (0..30).map(|i| sub(&i.to_string(), &format!("sub-{i:02}"), SubscriptionState::Healthy)).collect();
    a.update(Action::OverviewLoaded(Box::new(d)));
    let out = render(&mut a, 80, 24);
    // 80×24: 健康度面板内高 9 行 → 8 条 + 1 行「还有 22 个」
    assert!(out.contains("… 还有 22 个"), "{out}");
    assert!(out.contains("sub-07") && !out.contains("sub-08"), "{out}");
}

#[test]
fn reconnect_toast_appears_then_expires() {
    let mut a = loaded(false);
    a.update(Action::ConnectionLost);
    assert!(render(&mut a, 80, 24).contains(ZH.conn_reconnecting));
    a.update(Action::Connected { app_version: VERSION.into() });
    assert!(render(&mut a, 80, 24).contains(ZH.toast_reconnected));
    a.update(Action::Tick { now_ms: NOW + 2_999 });
    assert!(render(&mut a, 80, 24).contains(ZH.toast_reconnected));
    a.update(Action::Tick { now_ms: NOW + 3_000 });
    assert!(!render(&mut a, 80, 24).contains(ZH.toast_reconnected));
}

#[test]
fn load_failure_toasts_only_while_connected() {
    let fail = || Action::LoadFailed { cmd: Cmd::FetchOverview, message: "boom".into() };
    let mut a = loaded(false);
    a.update(fail());
    assert!(render(&mut a, 80, 24).contains("加载失败：boom"));

    let mut b = loaded(false);
    b.update(Action::ConnectionLost);
    b.update(fail());
    assert!(!render(&mut b, 80, 24).contains("加载失败"));
}

// ---------- 更新逻辑 ----------

#[test]
fn keys_map_to_actions() {
    let mut a = loaded(false);
    assert_eq!(a.handle_key(key(KeyCode::Char('q'))), Some(Action::Quit));
    assert_eq!(a.handle_key(key(KeyCode::Char('3'))), Some(Action::SwitchTab(2)));
    assert_eq!(a.handle_key(key(KeyCode::Char('6'))), None);
    assert_eq!(a.handle_key(key(KeyCode::Tab)), Some(Action::NextTab));
    assert_eq!(a.handle_key(key(KeyCode::BackTab)), Some(Action::PrevTab));
    assert_eq!(a.handle_key(key(KeyCode::Char('r'))), Some(Action::Refresh));
    assert_eq!(a.handle_key(key(KeyCode::Char('?'))), Some(Action::ToggleHelp));
    assert_eq!(a.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)), Some(Action::Quit));
}

#[test]
fn key_release_events_are_ignored() {
    let release = KeyEvent {
        code: KeyCode::Char('q'),
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Release,
        state: KeyEventState::NONE,
    };
    assert_eq!(loaded(false).handle_key(release), None);
}

#[test]
fn an_open_popup_swallows_page_keys_but_not_ctrl_c() {
    let mut a = loaded(false);
    a.update(Action::ToggleHelp);
    assert_eq!(a.handle_key(key(KeyCode::Char('2'))), None);
    assert_eq!(a.handle_key(key(KeyCode::Char('r'))), None);
    assert_eq!(a.handle_key(key(KeyCode::Esc)), Some(Action::ClosePopup));
    assert_eq!(a.handle_key(key(KeyCode::Char('q'))), Some(Action::ClosePopup));
    assert_eq!(a.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)), Some(Action::Quit));
}

#[test]
fn tabs_wrap_around_and_returning_to_a_page_refreshes_it() {
    let mut a = loaded(false);
    assert!(a.update(Action::PrevTab).is_empty(), "占位页不拉数据");
    assert!(render(&mut a, 80, 24).contains(ZH.coming_soon));
    assert_eq!(a.update(Action::NextTab), vec![Cmd::FetchOverview], "从第 5 页绕回总览");
    assert!(a.update(Action::SwitchTab(0)).is_empty(), "已经在这一页");
    assert!(a.update(Action::SwitchTab(9)).is_empty());
}

#[test]
fn only_the_visible_page_polls_and_only_while_connected() {
    let mut a = loaded(false);
    let mut fetches = 0;
    for i in 1..=40 {
        fetches += a.update(Action::Tick { now_ms: NOW + i * 250 }).len();
    }
    assert_eq!(fetches, 2, "40 个 tick = 10 秒 = 2 次轮询");

    a.update(Action::ConnectionLost);
    let quiet: usize = (41..=80).map(|i| a.update(Action::Tick { now_ms: NOW + i * 250 }).len()).sum();
    assert_eq!(quiet, 0, "断线期间不轮询");

    let mut b = loaded(false);
    b.update(Action::SwitchTab(3));
    let hidden: usize = (1..=40).map(|i| b.update(Action::Tick { now_ms: NOW + i * 250 }).len()).sum();
    assert_eq!(hidden, 0, "总览不可见时不轮询");
}

#[test]
fn subscription_events_refetch_the_list_only_on_the_overview() {
    let ev = |name: &str| Action::Sse { name: name.into(), data: "\"1\"".into() };
    let mut a = loaded(false);
    assert_eq!(a.update(ev("subscription_state_changed")), vec![Cmd::FetchSubscriptions]);
    assert_eq!(a.update(ev("subscription_quota_reached")), vec![Cmd::FetchSubscriptions]);
    assert!(a.update(ev("route_attempt_started")).is_empty());
    a.update(Action::SwitchTab(2));
    assert!(a.update(ev("subscription_state_changed")).is_empty());
}

#[test]
fn a_load_that_finishes_after_leaving_the_page_still_lands() {
    let mut a = app(false);
    a.update(Action::Connected { app_version: VERSION.into() });
    a.update(Action::SwitchTab(1));
    a.update(Action::OverviewLoaded(Box::new(data())));
    a.update(Action::SwitchTab(0));
    assert!(render(&mut a, 80, 24).contains("1,284"));
}

// ---------- 动效 ----------

/// 把正在播的效果播完。
fn settle(a: &mut App) {
    render(a, 80, 24);
    render_with(a, 80, 24, Duration::from_secs(2));
    assert!(!a.wants_fast_frames());
}

#[test]
fn startup_effect_plays_once() {
    let mut a = loaded(true);
    render(&mut a, 80, 24);
    assert!(a.wants_fast_frames(), "第一帧触发启动动效");
    render_with(&mut a, 80, 24, Duration::from_secs(2));
    assert!(!a.wants_fast_frames());
    render(&mut a, 80, 24);
    assert!(!a.wants_fast_frames(), "只播一次");
}

/// 空闲时两帧之间隔了很久, 新触发的效果不能被这段空闲「快进」掉。
#[test]
fn idle_time_before_a_trigger_does_not_fast_forward_the_effect() {
    let mut a = loaded(true);
    settle(&mut a);
    a.update(Action::SwitchTab(1));
    render_with(&mut a, 80, 24, Duration::from_secs(5));
    assert!(a.wants_fast_frames(), "切页效果这一帧才开始");
    render_with(&mut a, 80, 24, Duration::from_millis(200));
    assert!(!a.wants_fast_frames(), "150ms 的效果 200ms 后结束");
}

#[test]
fn a_changed_subscription_row_flashes() {
    let mut a = loaded(true);
    settle(&mut a);

    a.update(Action::SubscriptionsLoaded(data().subscriptions));
    render(&mut a, 80, 24);
    assert!(!a.wants_fast_frames(), "没变化就不闪");

    let mut subs = data().subscriptions;
    subs[1].state = SubscriptionState::RateLimited;
    subs[1].is_dispatchable = false;
    a.update(Action::SubscriptionsLoaded(subs));
    render(&mut a, 80, 24);
    assert!(a.wants_fast_frames());
}

#[test]
fn a_changed_number_pulses_but_the_first_load_does_not() {
    let mut a = loaded(true);
    settle(&mut a);
    let mut d = data();
    d.stats.total_requests += 1;
    a.update(Action::OverviewLoaded(Box::new(d)));
    render(&mut a, 80, 24);
    assert!(a.wants_fast_frames());
}

#[test]
fn with_fx_disabled_nothing_ever_animates() {
    let mut a = loaded(false);
    render(&mut a, 80, 24);
    a.update(Action::SwitchTab(1));
    a.update(Action::ToggleHelp);
    render(&mut a, 80, 24);
    assert!(!a.wants_fast_frames());
}

/// F1: 终端太小画不出内容, 在播的效果必须被清掉, 否则 `wants_fast_frames()` 会一直为真
/// 把主循环锁死在 60fps。
#[test]
fn shrinking_below_the_minimum_stops_the_animation_loop() {
    let mut a = loaded(true);
    render(&mut a, 80, 24);
    assert!(a.wants_fast_frames(), "启动动效这一帧才开始");
    render(&mut a, 79, 24);
    assert!(!a.wants_fast_frames(), "太小画不出内容, 动效应该被清掉");
}

/// F2: `calc_step(0)` 在 throbber-widgets-tui 里是「随机取一格」, spinner 步长不能传 0 ——
/// 否则同一状态画两次会得到不同的帧, 违反 `draw` 是状态纯函数的约束。用 tick=0 (未连接第一帧)
/// 和帮助弹窗打开 (走 draw_health 的 loading 分支) 两种场景各画两遍比较。
#[test]
fn drawing_the_same_state_twice_gives_the_same_frame() {
    let mut a = app(false);
    let first = render(&mut a, 80, 24);
    let second = render(&mut a, 80, 24);
    assert_eq!(first, second);

    let mut b = loaded(false);
    b.update(Action::ToggleHelp);
    let first = render(&mut b, 80, 24);
    let second = render(&mut b, 80, 24);
    assert_eq!(first, second);
}

/// F3: 空列表加载时没有「旧」行可以比较, 不该把更早排队、还没画出来的闪烁带到后面某一帧。
/// (对照第一次加载改变了一条订阅状态 → 排队一个闪烁; 紧接着加载空列表, 在这个闪烁还没画之前
/// 就把它取走扔掉; 再加载回原始数据时, 因为对比的「旧」列表是空的, 不会重新排队这个闪烁。)
#[test]
fn a_flash_queued_while_the_list_was_empty_does_not_fire_later() {
    let mut a = loaded(true);
    settle(&mut a);

    let mut subs = data().subscriptions;
    subs[1].state = SubscriptionState::RateLimited;
    subs[1].is_dispatchable = false;
    a.update(Action::SubscriptionsLoaded(subs));
    a.update(Action::SubscriptionsLoaded(vec![]));
    render(&mut a, 80, 24);
    settle(&mut a);

    a.update(Action::SubscriptionsLoaded(data().subscriptions));
    render(&mut a, 80, 24);
    assert!(!a.wants_fast_frames(), "早前排队又落空的闪烁不该在这里冒出来");
}

/// F4: 断线期间每 5 秒轮询失败一次, 文案完全相同; 不该无限堆积 toast 队列。
#[test]
fn repeated_failures_do_not_pile_up_toasts() {
    let mut a = loaded(false);
    let fail = || Action::LoadFailed { cmd: Cmd::FetchOverview, message: "boom".into() };
    for _ in 0..10 {
        a.update(fail());
    }
    assert!(render(&mut a, 80, 24).contains("加载失败：boom"));
    a.update(Action::Tick { now_ms: NOW + 3_000 });
    assert!(!render(&mut a, 80, 24).contains("加载失败"), "去重后只剩一条, 过期就该消失");
}

/// F6: `toast::area` 之前硬编码 `screen.y + 3`——版本不一致横幅也画在这一行, 会被 toast 盖住。
#[test]
fn a_toast_does_not_cover_the_version_banner() {
    let mut a = app(false);
    a.update(Action::Connected { app_version: "1.2.3".into() });
    a.update(Action::LoadFailed { cmd: Cmd::FetchOverview, message: "boom".into() });
    // 80 列下版本不一致的横幅文案比屏幕还宽, 会填满整行; toast 如果画到这一行,
    // 一定会用自己的边框字符覆盖掉横幅的一部分 —— 这样测试才咬得住 toast 顶行硬编码回归。
    let out = render(&mut a, 80, 30);
    let lines: Vec<&str> = out.lines().collect();
    let banner_line = lines.iter().position(|l| l.contains('⚠')).unwrap_or_else(|| panic!("没有横幅行\n{out}"));
    let toast_line = lines.iter().position(|l| l.contains("加载失败")).unwrap_or_else(|| panic!("没有 toast 文字行\n{out}"));
    assert!(
        toast_line >= banner_line + 2,
        "toast 顶部至少要在横幅下方 2 行 (顶边框 1 行 + 文字行) (banner={banner_line}, toast={toast_line})\n{out}"
    );
    let banner_text = lines[banner_line];
    assert!(
        !banner_text.contains('╭') && !banner_text.contains('╮') && !banner_text.contains('│'),
        "横幅行不该出现 toast 的边框字符\n{out}"
    );
}

/// F7: `a_changed_number_pulses_but_the_first_load_does_not` 的「首次加载不脉冲」这一半被
/// `settle()` 悄悄吞掉了, 单独补一个覆盖它。
#[test]
fn the_first_load_does_not_pulse() {
    let mut a = app(true);
    a.update(Action::Connected { app_version: VERSION.into() });
    settle(&mut a);
    a.update(Action::OverviewLoaded(Box::new(data())));
    render(&mut a, 80, 24);
    assert!(!a.wants_fast_frames(), "首次加载没有旧值可比, 不该脉冲");
}

/// H2: toast 的消散效果 (`fx::ms::TOAST_OUT` = 300ms) 播完之后, `Action::Tick` 要等到下一个
/// 250ms 档位才会真正把它弹出队列 —— 这段空隙里任何重画 (按键 / SSE 事件触发, 都不带
/// `elapsed` 时间) 不该让它以全亮度闪回。
#[test]
fn a_dissolved_toast_does_not_flash_back() {
    let mut a = loaded(true);
    settle(&mut a); // 播完启动动效

    a.update(Action::ConnectionLost);
    a.update(Action::Connected { app_version: VERSION.into() });
    settle(&mut a); // 播完 toast_in

    a.update(Action::Tick { now_ms: NOW + 2_750 });
    render(&mut a, 80, 24); // 触发消散 (fading = true), 这一帧还没播

    render_with(&mut a, 80, 24, Duration::from_millis(400)); // 播完消散 (300ms 足够)

    // 模拟 Tick 之前的一次按键重画: 没有新的 Tick, 消散早已播完。
    let out = render_with(&mut a, 80, 24, Duration::ZERO);
    assert!(!out.contains(ZH.toast_reconnected), "消散播完后不该再闪回\n{out}");
}

#[test]
fn quit_action_yields_the_quit_cmd() {
    assert_eq!(loaded(false).update(Action::Quit), vec![Cmd::Quit]);
}

#[test]
fn empty_subscription_list_and_listen_all_are_shown() {
    let mut a = app(false);
    a.update(Action::Connected { app_version: VERSION.into() });
    let mut d = data();
    d.subscriptions = vec![];
    d.status.listen_all = true;
    a.update(Action::OverviewLoaded(Box::new(d)));
    // 80×24 是支持的最小终端: 「基址 · 监听 0.0.0.0」这一整行必须放得下, 不能被裁掉 (fix round 2)。
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.ov_no_subs), "{out}");
    assert!(out.contains(ZH.ov_listen_all), "{out}");
    assert!(out.contains("监听 0.0.0.0"), "{out}");

    // https 的 base_url 比 http 多一个字符, 是更紧的那个场景, 同样要放得下。
    let mut b = app(false);
    b.update(Action::Connected { app_version: VERSION.into() });
    let mut d2 = data();
    d2.subscriptions = vec![];
    d2.status.listen_all = true;
    d2.status.base_url = "https://127.0.0.1:23457".into();
    b.update(Action::OverviewLoaded(Box::new(d2)));
    let out2 = render(&mut b, 80, 24);
    assert!(out2.contains("https://127.0.0.1:23457"), "{out2}");
    assert!(out2.contains("监听 0.0.0.0"), "{out2}");
}
