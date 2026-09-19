//! 界面测试: `App` 的更新逻辑 + `TestBackend` 渲染结果。不碰网络、不碰真实终端。
//!
//! 快照在 `tests/snapshots/`。改了布局后先看 diff 再接受:
//!   INSTA_UPDATE=always cargo test -p cc-router-tui --test ui
//! 动效在快照里一律关闭 (`fx_enabled: false`), 否则第一帧是启动动效的中间态。

use std::time::Duration;

use cc_router_tui::action::{Action, Cmd, Fetch, FetchData, OverviewData, Tab};
use cc_router_tui::app::{App, AppOptions};
use cc_router_tui::client::dto::{
    BalanceCache, BalanceEntry, BalanceSeverity, BalanceSnapshot, ModelCache, ModelInfo, ModelSlots, OverallStats, ProxyStatus,
    QuotaPeriod, QuotaUsage, SeriesPoint, Settings, SlotEfforts, Subscription, SubscriptionState,
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
    quota_period(QuotaPeriod::Daily, limit, used)
}

fn quota_period(period: QuotaPeriod, limit: u64, used: u64) -> QuotaUsage {
    QuotaUsage { period, limit: Some(limit), input: used, output: 0, cache_creation: 0, cache_read: 0, exceeded: used >= limit }
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
        provider_id: "p".into(),
        base_url: "https://example.invalid".into(),
        auth_type: "api_key".into(),
        model_slots: ModelSlots { fable: "d".into(), opus: "a".into(), sonnet: "b".into(), haiku: "c".into(), fallback: String::new() },
        slot_efforts: Default::default(),
        referenced_by: vec![],
        balance_supported: false,
        balance_cache: None,
        model_cache: None,
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

/// 构造一次 `Fetch::Overview` 加载完成的 [`Action`]。
fn overview_done(issued: u64, data: OverviewData) -> Action {
    Action::FetchDone { fetch: Fetch::Overview, issued, result: Ok(FetchData::Overview(Box::new(data))) }
}

/// 构造一次 `Fetch::Subscriptions` 加载完成的 [`Action`]。
fn subs_done(issued: u64, subs: Vec<Subscription>) -> Action {
    Action::FetchDone { fetch: Fetch::Subscriptions, issued, result: Ok(FetchData::Subscriptions(subs)) }
}

/// 构造一次加载失败的 [`Action`]; `fetch`/`issued` 对失败路径不影响 (`App::update` 不看它们)。
fn fetch_failed(message: &str) -> Action {
    Action::FetchDone { fetch: Fetch::Overview, issued: 0, result: Err(message.into()) }
}

fn loaded(fx_enabled: bool) -> App {
    let mut a = app(fx_enabled);
    assert_eq!(a.update(Action::Connected { app_version: VERSION.into() }), vec![Cmd::Fetch(Fetch::Overview)]);
    assert!(a.update(overview_done(1, data())).is_empty());
    a
}

/// 订阅页测试用的 3 条订阅 (后端顺序: 智谱主号 / Kimi 备用 / 示例中转), 字段覆盖 task-3-brief.md
/// 详情面板的每一条规则:
///   - 智谱主号: 单个限额周期、槽位 effort 覆盖 (opus=high)、兜底槽未配置、模型已缓存、
///     被引用、余额有 entries (含 Low 严重度 + hint, 外加 is_available=false 的不可用行)。
///   - Kimi 备用: 限流 + 冷却倒计时、两个限额周期同时设了上限、sonnet 槽是 (pending)、
///     余额支持但还没查过、有最近错误 (用来看 Wrap)。
///   - 示例中转: 凭证失效、余额不支持、没有限额、没有被引用、没有最近错误、模型没缓存过 ——
///     占位符那组规则全靠它覆盖。
fn detail_subs() -> Vec<Subscription> {
    let mut zhipu = sub("1", "智谱主号", SubscriptionState::Healthy);
    zhipu.provider_display_name = "智谱".into();
    zhipu.provider_id = "zhipu".into();
    zhipu.base_url = "https://open.bigmodel.cn/api/anthropic".into();
    zhipu.auth_type = "api_key".into();
    zhipu.model_slots =
        ModelSlots { fable: "glm-4.6".into(), opus: "glm-4.6".into(), sonnet: "glm-4.6".into(), haiku: "glm-4.5-air".into(), fallback: String::new() };
    zhipu.slot_efforts = SlotEfforts { opus: Some("high".into()), ..Default::default() };
    zhipu.quota_usage = vec![quota(1000, 620)];
    zhipu.balance_supported = true;
    zhipu.balance_cache = Some(BalanceCache {
        fetched_at: NOW,
        snapshot: BalanceSnapshot {
            is_available: Some(false),
            entries: vec![
                BalanceEntry { label: "余额".into(), value_text: "39.28".into(), unit: "CNY".into(), hint: None, severity: BalanceSeverity::Normal },
                BalanceEntry {
                    label: "赠送余额".into(),
                    value_text: "1.00".into(),
                    unit: "CNY".into(),
                    hint: Some("即将过期".into()),
                    severity: BalanceSeverity::Low,
                },
            ],
        },
    });
    zhipu.model_cache = Some(ModelCache { fetched_at: NOW, models: (0..12).map(|i| ModelInfo { id: format!("m{i}"), display_name: None }).collect() });
    zhipu.referenced_by = vec!["model-sonnet".into(), "model-opus".into()];

    let mut kimi = sub("2", "Kimi 备用", SubscriptionState::RateLimited);
    kimi.provider_display_name = "Moonshot".into();
    kimi.cooldown_until = Some(NOW + 42_000);
    kimi.model_slots.sonnet = "(pending)".into();
    kimi.quota_usage = vec![quota_period(QuotaPeriod::Daily, 1000, 950), quota_period(QuotaPeriod::Monthly, 5000, 4200)];
    kimi.balance_supported = true;
    kimi.balance_cache = None;
    kimi.referenced_by = vec!["model-haiku".into()];
    kimi.last_error_message = Some("上游返回 429 Too Many Requests, 已达到本分钟请求数上限, 请稍后重试".into());

    let mut relay = sub("3", "示例中转", SubscriptionState::AuthFailed);
    relay.provider_display_name = "自定义中转".into();
    relay.base_url = "https://relay.example.invalid/v1".into();
    relay.model_slots = ModelSlots {
        fable: "claude-haiku-4-5".into(),
        opus: "claude-opus-4-1".into(),
        sonnet: "claude-sonnet-4-5".into(),
        haiku: "claude-haiku-4-5".into(),
        fallback: String::new(),
    };

    vec![zhipu, kimi, relay]
}

/// 连上 + 切到订阅页 + 喂一份 `detail_subs()`。
fn subs_app(fx_enabled: bool) -> App {
    let mut a = app(fx_enabled);
    a.update(Action::Connected { app_version: VERSION.into() });
    a.update(Action::SwitchTab(Tab::Subscriptions));
    a.update(subs_done(1, detail_subs()));
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
    // 订阅页 (Tab::Subscriptions) 从 Task 3 起是真页面了, 占位快照换一个仍然占位的标签。
    a.update(Action::SwitchTab(Tab::VirtualModels));
    insta::assert_snapshot!(render(&mut a, 80, 24));
}

// ---------- 订阅页 ----------

#[test]
fn subscriptions_120x40() {
    insta::assert_snapshot!(render(&mut subs_app(false), 120, 40));
}

#[test]
fn subscriptions_list_80x24() {
    insta::assert_snapshot!(render(&mut subs_app(false), 80, 24));
}

#[test]
fn subscriptions_detail_80x24() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24); // 先画一帧, 让页面记住这是窄屏。
    a.handle_key(key(KeyCode::Enter));
    insta::assert_snapshot!(render(&mut a, 80, 24));
}

#[test]
fn subscriptions_page_lists_in_backend_order() {
    let out = render(&mut subs_app(false), 120, 40);
    let pos = |needle: &str| out.find(needle).unwrap_or_else(|| panic!("缺 {needle}\n{out}"));
    assert!(pos("智谱主号") < pos("Kimi 备用"), "应该按后端给的顺序, 不按严重度排\n{out}");
    assert!(pos("Kimi 备用") < pos("示例中转"), "{out}");
}

#[test]
fn selection_moves_and_clamps() {
    let mut a = subs_app(false);
    for _ in 0..4 {
        a.handle_key(key(KeyCode::Char('j')));
    }
    let out = render(&mut a, 120, 40);
    assert!(out.contains(" 3/3 "), "j j j j 不该越过最后一条\n{out}");

    a.handle_key(key(KeyCode::Char('k')));
    let out = render(&mut a, 120, 40);
    assert!(out.contains(" 2/3 "), "{out}");

    a.handle_key(key(KeyCode::Char('G')));
    assert!(render(&mut a, 120, 40).contains(" 3/3 "));
    a.handle_key(key(KeyCode::Char('g')));
    assert!(render(&mut a, 120, 40).contains(" 1/3 "));

    // 到顶之后再往上不该绕回最后一条。
    a.handle_key(key(KeyCode::Char('k')));
    assert!(render(&mut a, 120, 40).contains(" 1/3 "), "k 到顶不绕回");
}

/// 用窄屏 + 进详情来断言, 好让渲染出来的文字只包含被选中的那一条 —— 宽屏双栏下列表本来就会把
/// 三条订阅的名字/厂商全部列出来, `out.contains("Kimi 备用")` 那种断言不管选没选中 Kimi 都成立,
/// 咬不住「选中项到底是谁」这件事。
#[test]
fn selection_follows_the_id_across_reloads() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24); // 让页面记住这是窄屏。
    a.handle_key(key(KeyCode::Char('j'))); // 选中第 2 条: Kimi 备用 (id "2")
    a.handle_key(key(KeyCode::Enter));
    let out = render(&mut a, 80, 24);
    assert!(out.contains("Kimi 备用") && out.contains("Moonshot"), "{out}");

    // 新列表把 Kimi 挪到第 3 位: [智谱主号, 示例中转, Kimi 备用]。
    let mut subs = detail_subs();
    subs.swap(1, 2);
    a.update(subs_done(2, subs));
    let out2 = render(&mut a, 80, 24);
    assert!(out2.contains("Kimi 备用") && out2.contains("Moonshot"), "选中项应该跟着 id 走, 不是跟着下标\n{out2}");

    // 再来一份不含 Kimi 的列表 (长度 2): 应该落到 last_index=2 钳制到长度 2 后的下标 1, 也就是
    // 「示例中转」(此时它在新列表的下标 1), 而不是 panic。
    let full = detail_subs();
    let without_kimi = vec![full[0].clone(), full[2].clone()];
    a.update(subs_done(3, without_kimi));
    let out3 = render(&mut a, 80, 24);
    assert!(out3.contains("示例中转"), "id 消失后应该落到钳制后的下标, 不 panic\n{out3}");
}

#[test]
fn enter_opens_detail_only_on_narrow_terminals() {
    let mut a = subs_app(false);
    let before = render(&mut a, 80, 24);
    assert!(before.contains(ZH.sub_col_name) && !before.contains(ZH.sub_f_endpoint), "{before}");

    a.handle_key(key(KeyCode::Enter));
    let after = render(&mut a, 80, 24);
    assert!(after.contains(ZH.sub_f_endpoint) && !after.contains(ZH.sub_col_name), "80 列: ⏎ 应该进详情\n{after}");

    a.handle_key(key(KeyCode::Esc));
    let back = render(&mut a, 80, 24);
    assert!(back.contains(ZH.sub_col_name), "{back}");

    let wide_before = render(&mut a, 120, 40);
    a.handle_key(key(KeyCode::Enter));
    let wide_after = render(&mut a, 120, 40);
    assert_eq!(wide_before, wide_after, "120 列: ⏎ 前后渲染应该完全相同");
}

#[test]
fn detail_survives_widening() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter));
    let narrow = render(&mut a, 80, 24);
    assert!(narrow.contains(ZH.sub_f_endpoint) && !narrow.contains(ZH.sub_col_name), "{narrow}");

    let wide = render(&mut a, 120, 40);
    assert!(wide.contains(ZH.sub_col_name) && wide.contains(ZH.sub_f_endpoint), "拉宽后左表右详情都该在\n{wide}");
}

#[test]
fn detail_shows_every_limited_period_and_pending_slots() {
    let mut a = subs_app(false);
    a.handle_key(key(KeyCode::Char('j'))); // Kimi: 两个限额周期 + pending 槽
    let out = render(&mut a, 120, 40);
    assert!(out.contains(ZH.q_daily) && out.contains(ZH.q_monthly), "两个设了上限的周期都该有一条 gauge\n{out}");
    assert!(out.contains("(pending)"), "{out}");
}

#[test]
fn balance_rows_cover_unsupported_never_and_entries() {
    let mut a = subs_app(false);
    // 智谱主号: 有 entries (含 Low 严重度 + hint), 外加 is_available=false 的不可用提示行。
    let out_zhipu = render(&mut a, 120, 40);
    assert!(out_zhipu.contains("39.28") && out_zhipu.contains("CNY"), "{out_zhipu}");
    assert!(out_zhipu.contains("即将过期"), "hint 应该跟在 entry 后面\n{out_zhipu}");
    assert!(out_zhipu.contains(ZH.sub_balance_unavailable), "is_available=false 应该加一行不可用提示\n{out_zhipu}");

    // Kimi 备用: 支持但还没查过。
    a.handle_key(key(KeyCode::Char('j')));
    let out_kimi = render(&mut a, 120, 40);
    assert!(out_kimi.contains(ZH.sub_balance_never), "{out_kimi}");

    // 示例中转: 该厂商不支持余额查询。
    a.handle_key(key(KeyCode::Char('j')));
    let out_relay = render(&mut a, 120, 40);
    assert!(out_relay.contains(ZH.sub_balance_unsupported), "{out_relay}");
}

#[test]
fn unreferenced_and_no_error_use_placeholders() {
    let mut a = subs_app(false);
    a.handle_key(key(KeyCode::Char('G'))); // 示例中转: 没有被引用, 也没有最近错误
    let out = render(&mut a, 120, 40);
    assert!(out.contains(ZH.sub_unreferenced), "{out}");
    let error_row = out.lines().find(|l| l.contains(ZH.sub_f_last_error)).unwrap_or_else(|| panic!("缺「最近错误」这一行\n{out}"));
    assert!(error_row.contains('—'), "没有最近错误时应该显示占位符\n{out}");
}

#[test]
fn esc_without_a_popup_reaches_the_page() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24); // 让页面记住这是窄屏。
    a.handle_key(key(KeyCode::Enter));
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.sub_f_endpoint), "应该已经进入详情\n{out}");

    assert_eq!(a.handle_key(key(KeyCode::Esc)), None, "没有弹窗时 Esc 应该交给页面, 不产生 Action");
    let out2 = render(&mut a, 80, 24);
    assert!(out2.contains(ZH.sub_col_name), "Esc 应该退出详情回到列表\n{out2}");

    // 弹窗打开时 Esc 仍然只关弹窗。
    a.update(Action::ToggleHelp);
    assert_eq!(a.handle_key(key(KeyCode::Esc)), Some(Action::ClosePopup));
}

#[test]
fn a_changed_subscription_row_flashes_on_the_subscriptions_page() {
    let mut a = subs_app(true);
    settle(&mut a);

    let mut subs = detail_subs();
    subs[1].state = SubscriptionState::TransientError;
    subs[1].is_dispatchable = false;
    a.update(subs_done(2, subs));
    render(&mut a, 80, 24);
    assert!(a.wants_fast_frames(), "订阅页也该因为状态变化而闪一下");
}

#[test]
fn help_popup_shows_page_keys_on_the_subscriptions_page() {
    let mut a = subs_app(false);
    a.update(Action::ToggleHelp);
    let out = render(&mut a, 80, 24);
    for (_, desc) in ZH.sub_help_rows {
        assert!(out.contains(desc), "缺 {desc:?}\n{out}");
    }
    assert!(out.contains("PgUp / PgDn"), "{out}");
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
    a.update(overview_done(1, d));
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
    let mut a = loaded(false);
    a.update(fetch_failed("boom"));
    assert!(render(&mut a, 80, 24).contains("加载失败：boom"));

    let mut b = loaded(false);
    b.update(Action::ConnectionLost);
    b.update(fetch_failed("boom"));
    assert!(!render(&mut b, 80, 24).contains("加载失败"));
}

// ---------- 更新逻辑 ----------

#[test]
fn keys_map_to_actions() {
    let mut a = loaded(false);
    assert_eq!(a.handle_key(key(KeyCode::Char('q'))), Some(Action::Quit));
    assert_eq!(a.handle_key(key(KeyCode::Char('3'))), Some(Action::SwitchTab(Tab::VirtualModels)));
    assert_eq!(a.handle_key(key(KeyCode::Char('6'))), None);
    assert_eq!(a.handle_key(key(KeyCode::Tab)), Some(Action::NextTab));
    assert_eq!(a.handle_key(key(KeyCode::BackTab)), Some(Action::PrevTab));
    assert_eq!(a.handle_key(key(KeyCode::Char('r'))), Some(Action::Refresh));
    assert_eq!(a.handle_key(key(KeyCode::Char('?'))), Some(Action::ToggleHelp));
    assert_eq!(a.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)), Some(Action::Quit));
}

/// `'3'` 直达第三个标签 (`Tab::VirtualModels`, 0 起算下标 2); `'6'` 超出 5 个标签, 不产生 action。
#[test]
fn tab_keys_use_the_tab_enum() {
    let mut a = loaded(false);
    assert_eq!(a.handle_key(key(KeyCode::Char('3'))), Some(Action::SwitchTab(Tab::VirtualModels)));
    assert_eq!(a.handle_key(key(KeyCode::Char('6'))), None);
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
    assert_eq!(a.update(Action::NextTab), vec![Cmd::Fetch(Fetch::Overview)], "从第 5 页绕回总览");
    assert!(a.update(Action::SwitchTab(Tab::Overview)).is_empty(), "已经在这一页");
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
    b.update(Action::SwitchTab(Tab::Live));
    let hidden: usize = (1..=40).map(|i| b.update(Action::Tick { now_ms: NOW + i * 250 }).len()).sum();
    assert_eq!(hidden, 0, "总览不可见时不轮询");

    // 停在订阅页时轮询发的是 Fetch::Subscriptions, 不是 Fetch::Overview。
    let mut c = loaded(false);
    c.update(Action::SwitchTab(Tab::Subscriptions));
    let sub_fetches: Vec<Cmd> = (1..=20).flat_map(|i| c.update(Action::Tick { now_ms: NOW + i * 250 })).collect();
    assert_eq!(sub_fetches, vec![Cmd::Fetch(Fetch::Subscriptions)], "订阅页可见时轮询应该发 Fetch::Subscriptions");
}

#[test]
fn subscription_events_refetch_the_list_only_on_the_overview() {
    let ev = |name: &str| Action::Sse { name: name.into(), data: "\"1\"".into() };
    let mut a = loaded(false);
    assert_eq!(a.update(ev("subscription_state_changed")), vec![Cmd::Fetch(Fetch::Subscriptions)]);
    assert_eq!(a.update(ev("subscription_quota_reached")), vec![Cmd::Fetch(Fetch::Subscriptions)]);
    assert!(a.update(ev("route_attempt_started")).is_empty());
    a.update(Action::SwitchTab(Tab::VirtualModels));
    assert!(a.update(ev("subscription_state_changed")).is_empty());
}

#[test]
fn a_load_that_finishes_after_leaving_the_page_still_lands() {
    let mut a = app(false);
    a.update(Action::Connected { app_version: VERSION.into() });
    a.update(Action::SwitchTab(Tab::Subscriptions));
    a.update(overview_done(1, data()));
    a.update(Action::SwitchTab(Tab::Overview));
    assert!(render(&mut a, 80, 24).contains("1,284"));
}

// ---------- Store / 订阅数据流 ----------

/// 晚到的旧总览加载不该把更新的订阅列表覆盖回去; 但总览页自己的统计数字 (与订阅无关) 照常更新。
#[test]
fn a_stale_overview_does_not_overwrite_a_newer_subscription_list() {
    let mut a = loaded(false); // issued=1 (Overview)

    let mut subs = data().subscriptions;
    subs[1].state = SubscriptionState::RateLimited; // "智谱主号"
    subs[1].is_dispatchable = false;
    a.update(subs_done(7, subs));

    let mut old = data();
    old.stats.total_requests = 9999;
    a.update(overview_done(6, old)); // 晚于 issued=7, 订阅部分应该被丢弃

    let out = render(&mut a, 80, 24);
    let row = out.lines().find(|l| l.contains("智谱主号")).unwrap_or_else(|| panic!("缺「智谱主号」这一行\n{out}"));
    assert!(row.contains("限流"), "订阅列表不该被旧的 overview 加载覆盖\n{out}");
    assert!(out.contains("9,999"), "总览页自己的统计数字照常用这次 overview 的\n{out}");
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
    a.update(Action::SwitchTab(Tab::Subscriptions));
    render_with(&mut a, 80, 24, Duration::from_secs(5));
    assert!(a.wants_fast_frames(), "切页效果这一帧才开始");
    render_with(&mut a, 80, 24, Duration::from_millis(200));
    assert!(!a.wants_fast_frames(), "150ms 的效果 200ms 后结束");
}

#[test]
fn a_changed_subscription_row_flashes() {
    let mut a = loaded(true);
    settle(&mut a);

    a.update(subs_done(2, data().subscriptions));
    render(&mut a, 80, 24);
    assert!(!a.wants_fast_frames(), "没变化就不闪");

    let mut subs = data().subscriptions;
    subs[1].state = SubscriptionState::RateLimited;
    subs[1].is_dispatchable = false;
    a.update(subs_done(3, subs));
    render(&mut a, 80, 24);
    assert!(a.wants_fast_frames());
}

/// I1 fix round 1: 订阅变更通知 (`on_subscriptions_changed` 的 fan-out) 必须从 `Fetch::Overview`
/// 与 `Fetch::Subscriptions` 两条 `FetchDone` 分支都能送达页面。重构前两处各自维护一份页面列表,
/// 漏改其中一处不会挂任何编译错误或既有测试, 只会表现成"轮询触发的整页刷新不闪, 但 SSE 触发的
/// 单独订阅刷新会闪"这种只在真机上才会注意到的不对称。
#[test]
fn subscription_changes_reach_pages_from_both_fetch_kinds() {
    // 经 Fetch::Overview (整页加载) 送达的变化。
    let mut a = loaded(true);
    settle(&mut a);
    let mut d = data();
    d.subscriptions[1].state = SubscriptionState::RateLimited;
    d.subscriptions[1].is_dispatchable = false;
    a.update(overview_done(2, d));
    render(&mut a, 80, 24);
    assert!(a.wants_fast_frames(), "经 Fetch::Overview 送达的订阅变化也该闪");

    // 经 Fetch::Subscriptions (单独刷新) 送达的变化; 另开一个 App 避免和上面的动效互相干扰。
    let mut b = loaded(true);
    settle(&mut b);
    let mut subs = data().subscriptions;
    subs[1].state = SubscriptionState::RateLimited;
    subs[1].is_dispatchable = false;
    b.update(subs_done(2, subs));
    render(&mut b, 80, 24);
    assert!(b.wants_fast_frames(), "经 Fetch::Subscriptions 送达的订阅变化也该闪");
}

#[test]
fn a_changed_number_pulses_but_the_first_load_does_not() {
    let mut a = loaded(true);
    settle(&mut a);
    let mut d = data();
    d.stats.total_requests += 1;
    a.update(overview_done(2, d));
    render(&mut a, 80, 24);
    assert!(a.wants_fast_frames());
}

#[test]
fn with_fx_disabled_nothing_ever_animates() {
    let mut a = loaded(false);
    render(&mut a, 80, 24);
    a.update(Action::SwitchTab(Tab::Subscriptions));
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

    // 订阅页列表态: `resolve_selection` 在 `draw` 里重新解析下标, 必须是幂等的。
    let mut c = subs_app(false);
    let first = render(&mut c, 80, 24);
    let second = render(&mut c, 80, 24);
    assert_eq!(first, second, "订阅页列表态应该幂等");

    // 订阅页详情态 (窄屏)。
    c.handle_key(key(KeyCode::Enter));
    let first = render(&mut c, 80, 24);
    let second = render(&mut c, 80, 24);
    assert_eq!(first, second, "订阅页详情态应该幂等");
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
    a.update(subs_done(2, subs));
    a.update(subs_done(3, vec![]));
    render(&mut a, 80, 24);
    settle(&mut a);

    a.update(subs_done(4, data().subscriptions));
    render(&mut a, 80, 24);
    assert!(!a.wants_fast_frames(), "早前排队又落空的闪烁不该在这里冒出来");
}

/// F4: 断线期间每 5 秒轮询失败一次, 文案完全相同; 不该无限堆积 toast 队列。
#[test]
fn repeated_failures_do_not_pile_up_toasts() {
    let mut a = loaded(false);
    for _ in 0..10 {
        a.update(fetch_failed("boom"));
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
    a.update(fetch_failed("boom"));
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
    a.update(overview_done(1, data()));
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
    a.update(overview_done(1, d));
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
    b.update(overview_done(1, d2));
    let out2 = render(&mut b, 80, 24);
    assert!(out2.contains("https://127.0.0.1:23457"), "{out2}");
    assert!(out2.contains("监听 0.0.0.0"), "{out2}");
}
