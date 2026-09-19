//! 界面测试: `App` 的更新逻辑 + `TestBackend` 渲染结果。不碰网络、不碰真实终端。
//!
//! 快照在 `tests/snapshots/`。改了布局后先看 diff 再接受:
//!   INSTA_UPDATE=always cargo test -p cc-router-tui --test ui
//! 动效在快照里一律关闭 (`fx_enabled: false`), 否则第一帧是启动动效的中间态。

use std::time::Duration;

use cc_router_tui::action::{Action, Cmd, Fetch, FetchData, Mutation, MutationOutcome, OverviewData, Tab};
use cc_router_tui::app::{App, AppOptions};
use cc_router_tui::client::dto::{
    BalanceCache, BalanceEntry, BalanceSeverity, BalanceSnapshot, ModelCache, ModelInfo, ModelSlots, OverallStats, ProxyStatus,
    QuotaPeriod, QuotaUsage, RefreshBalanceResult, RefreshModelsResult, SeriesPoint, Settings, SlotEfforts, Subscription,
    SubscriptionState, TestConnectionResult,
};
use cc_router_tui::i18n::ZH;
use cc_router_tui::theme::{ColorMode, Theme};
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use ratatui::Terminal;
use unicode_width::UnicodeWidthStr;

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

/// M6 (`disabled_rows_are_muted`) 需要检查单元格的**样式**而不是文字内容, 文字断言够不着这个——
/// 所以留一份返回 `Buffer` 本身的变体, 供需要读 `.style()` 的测试直接检查颜色。
fn render_buffer(a: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| a.draw(f, Duration::ZERO)).unwrap();
    terminal.backend().buffer().clone()
}

/// 把 `buf` 第 `y` 行重建成一段纯文本 (宽字符的第二个占位单元格 `symbol()` 是空串, 天然跳过),
/// 用来定位某个字段所在的行号——不依赖页面内部的坐标常量。
fn buffer_row_text(buf: &ratatui::buffer::Buffer, y: u16) -> String {
    (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect()
}

/// 在第 `y` 行从左到右找纯 ASCII 子串 `needle` 出现的起始格, 返回那一格的样式——用来断言
/// 「这段文字是不是被整体涂成了 muted 颜色」(M6: `disabled_rows_are_muted`)。
fn find_cell_style(buf: &ratatui::buffer::Buffer, y: u16, needle: &str) -> ratatui::style::Style {
    let bytes: Vec<char> = needle.chars().collect();
    let mut matched = 0usize;
    let mut start_x = 0u16;
    for x in 0..buf.area.width {
        let sym = buf[(x, y)].symbol();
        let matches_next = sym.starts_with(bytes[matched]);
        if matches_next {
            if matched == 0 {
                start_x = x;
            }
            matched += 1;
            if matched == bytes.len() {
                return buf[(start_x, y)].style();
            }
        } else {
            matched = 0;
        }
    }
    panic!("第 {y} 行找不到 {needle:?}\n{}", buffer_row_text(buf, y));
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

/// Fix round 1, #7: 120×40 双栏下列表的 sonnet 列本来就会显示 "(pending)"（Kimi 那一行的槽位
/// 名字本身就是 "(pending)"), 断言 `out.contains("(pending)")` 不管选没选中 Kimi 都成立, 咬不住
/// "详情面板真的把它当 pending 槽位处理"这件事。改成窄屏 + 进详情, 让输出只有详情面板的内容。
#[test]
fn detail_shows_every_limited_period_and_pending_slots() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24); // 记住这是窄屏。
    a.handle_key(key(KeyCode::Char('j'))); // Kimi: 两个限额周期 + pending 槽
    a.handle_key(key(KeyCode::Enter));
    let out = render(&mut a, 80, 24);
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

/// 在一行里找到 `label` 之后, 数出它后面显示宽度上的列号 (border/pad/label 本身的宽度 + 紧跟着
/// 的空格数), 不依赖页面内部的 `FIELD_LABEL_COL` 常量——用渲染出来的文字反推, 这样如果实现改了
/// 对齐方式但没有真的对齐, 测试也不会跟着"配合"通过。
fn value_column(line: &str, label: &str) -> usize {
    let idx = line.find(label).unwrap_or_else(|| panic!("缺 {label:?} 这一行\n{line}"));
    let prefix_width = line[..idx].width();
    let after = &line[idx + label.len()..];
    let spaces = after.chars().take_while(|c| *c == ' ').count();
    prefix_width + label.width() + spaces
}

/// `render()` 返回的是 `TestBackend` 自己的 `Display` 格式, 每行形如
/// `"<可见内容>" Hidden by multi-width symbols: [...]` (宽字符的占位单元被跳过时才带这段调试
/// 信息)——不是原始终端行。逐字符对齐相关的断言得先剥掉这层包装, 拿到真正的可见内容再量列号,
/// 不然每行开头那个字面双引号会把 `trim_start_matches('│')` 之类的结构性判断全部带偏。
fn plain(line: &str) -> &str {
    let body = line.split(" Hidden by multi-width symbols:").next().unwrap_or(line);
    body.trim_matches('"')
}

/// 找到"去掉边框和内边距之后以 `label` 开头"的那一行——比 `l.contains(label)` 更严格, 避免
/// 「限额」这个标签被同一行里「日限额」这种周期名的子串抢先命中。
fn find_field_row<'a>(out: &'a str, label: &str) -> &'a str {
    out.lines()
        .map(plain)
        .find(|l| l.trim_start_matches('│').trim_start().starts_with(label))
        .unwrap_or_else(|| panic!("缺 {label:?} 这一行\n{out}"))
}

/// Fix round 1, #1: 限额行的 `Layout::horizontal(...).spacing(1)` 把标签也算进了间距里, 值比其它
/// 字段行的值多缩进一列。这里挑 7 个覆盖普通行 (状态/端点/模型/被引用) 与特殊渲染路径 (限额用
/// `LineGauge`、余额是多行里的第一行、最近错误是 `Wrap` 过的段落) 的字段, 断言它们的值列完全相同。
#[test]
fn detail_field_values_share_one_column() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter));
    let out = render(&mut a, 80, 24);

    let labels = [ZH.sub_f_state, ZH.sub_f_endpoint, ZH.sub_f_quota, ZH.sub_f_balance, ZH.sub_f_models, ZH.sub_f_referenced, ZH.sub_f_last_error];
    let columns: Vec<(&str, usize)> = labels.iter().map(|label| (*label, value_column(find_field_row(&out, label), label))).collect();
    let first = columns[0].1;
    for (label, col) in &columns {
        assert_eq!(*col, first, "「{label}」的值列错位: {columns:?}\n{out}");
    }
}

/// Fix round 1, #3: 详情面板装不下所有字段时, 之前是直接把靠后的字段 (比如"被引用") 悄悄丢掉;
/// 现在最后一行必须换成总览页同款的"还有 N 个"提示, 而且不能把底边框顶飞。
#[test]
fn detail_overflow_is_announced() {
    let mut a = app(false);
    a.update(Action::Connected { app_version: VERSION.into() });
    a.update(Action::SwitchTab(Tab::Subscriptions));

    let mut overflowing = detail_subs().into_iter().next().unwrap(); // 智谱主号, 已经带槽位/effort
    overflowing.quota_usage = vec![
        quota_period(QuotaPeriod::Daily, 100, 90),
        quota_period(QuotaPeriod::Weekly, 100, 90),
        quota_period(QuotaPeriod::Monthly, 100, 90),
        quota_period(QuotaPeriod::Total, 100, 90),
    ];
    overflowing.balance_cache = Some(BalanceCache {
        fetched_at: NOW,
        snapshot: BalanceSnapshot {
            is_available: None,
            entries: (0..3)
                .map(|i| BalanceEntry {
                    label: format!("余额{i}"),
                    value_text: "1.00".into(),
                    unit: "CNY".into(),
                    hint: None,
                    severity: BalanceSeverity::Normal,
                })
                .collect(),
        },
    });
    overflowing.last_error_message = Some("x".repeat(90)); // 够长, 折成好几行
    a.update(subs_done(1, vec![overflowing]));

    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter));
    let out = render(&mut a, 80, 24);
    assert!(out.contains("… 还有"), "装不下时最后一行应该提示还有多少字段没显示\n{out}");

    let lines: Vec<&str> = out.lines().collect();
    let block_bottom = lines[lines.len() - 2]; // 最后一行是底部键位栏, 倒数第二行才是详情面板的下边框。
    assert!(block_bottom.contains('╰') && block_bottom.contains('╯'), "溢出提示不该把面板的下边框顶飞\n{out}");
}

/// Fix round 1, #5: `base_url` 之前直接整串塞进 Line, 超宽时被无声硬裁 (没有省略号), 也可能顶到
/// 边框外面。给一段正好 70 列宽的端点 (比窄屏详情面板留给值的宽度还长), 断言渲染结果以省略号收尾。
#[test]
fn detail_long_base_url_is_truncated_with_ellipsis() {
    let mut a = subs_app(false);
    let mut subs = detail_subs();
    let prefix = "https://example.invalid/";
    subs[0].base_url = format!("{prefix}{}", "x".repeat(70 - prefix.width()));
    assert_eq!(subs[0].base_url.width(), 70, "测试串本身要保证正好 70 列宽");
    a.update(subs_done(2, subs));

    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter));
    let out = render(&mut a, 80, 24);
    let row = find_field_row(&out, ZH.sub_f_endpoint);
    let trimmed = row.trim_end_matches('│').trim_end();
    assert!(trimmed.ends_with('…'), "超长端点应该截断成省略号收尾, 不能硬裁\n{row}");
}

/// 从 " n/total " 标题里读出当前是第几条, 用来在不重复实现分页算法的前提下断言 `PageUp` /
/// `PageDown` 翻的是"一整屏", 不是固定步长 1。
fn parse_current_of(out: &str, total: usize) -> usize {
    let needle = format!("/{total} ");
    let idx = out.find(&needle).unwrap_or_else(|| panic!("缺 .../{total} 这段\n{out}"));
    let mut start = idx;
    while start > 0 && out.as_bytes()[start - 1].is_ascii_digit() {
        start -= 1;
    }
    out[start..idx].parse().unwrap_or_else(|_| panic!("解析不出当前行号: {:?}\n{out}", &out[start..idx]))
}

/// Fix round 1, #8: 第一帧画出来之前 `last_page_rows` 不该是 0 (那样 `PageDown` 只会移动 0 格,
/// 跟没按一样)；画过一帧之后, `PageDown` / `PageUp` 应该按可视行数整屏翻页, 并且在两端钳制。
#[test]
fn page_keys_move_by_the_visible_row_count() {
    let mut a = app(false);
    a.update(Action::Connected { app_version: VERSION.into() });
    a.update(Action::SwitchTab(Tab::Subscriptions));
    let subs: Vec<Subscription> = (0..30).map(|i| sub(&i.to_string(), &format!("sub-{i:02}"), SubscriptionState::Healthy)).collect();
    a.update(subs_done(1, subs));
    render(&mut a, 80, 24); // 画一帧, 让 `last_page_rows` 从默认值更新成真实可视行数。

    a.handle_key(key(KeyCode::PageDown));
    let after_down = render(&mut a, 80, 24);
    let landed = parse_current_of(&after_down, 30);
    assert!(landed > 1 && landed < 30, "PageDown 应该往下翻一整屏, 不是移动 1 格 (落在第 {landed} 条)\n{after_down}");

    a.handle_key(key(KeyCode::PageUp));
    let after_up = render(&mut a, 80, 24);
    assert_eq!(parse_current_of(&after_up, 30), 1, "PageUp 应该翻回同样的距离\n{after_up}");

    for _ in 0..5 {
        a.handle_key(key(KeyCode::PageDown));
    }
    let bottom = render(&mut a, 80, 24);
    assert!(bottom.contains(" 30/30 "), "多次 PageDown 应该钳制在最后一条, 不越界\n{bottom}");

    for _ in 0..5 {
        a.handle_key(key(KeyCode::PageUp));
    }
    let top = render(&mut a, 80, 24);
    assert!(top.contains(" 1/30 "), "多次 PageUp 应该钳制在第一条, 不越界\n{top}");
}

// ---------- final-fix: I2 模型名列按宽度动态算 ----------

/// I2: 真实模型 id 经常超过旧的固定 24 列 (`claude-sonnet-4-5-20250929` / 30 字符级别的
/// `qwen3-coder-480b-a35b-instruct`)。80 列窄屏详情 (inner=76) 与 120×40 宽屏右侧详情面板
/// (inner=62) 都留有余量 (`slot_model_col` 分别算出 58 / 44), 应该整段显示, 不截断。
#[test]
fn a_long_model_name_renders_in_full_when_there_is_room() {
    let long_model = "qwen3-coder-480b-a35b-instruct";
    assert!(long_model.len() > 24, "测试串本身要比旧的固定列宽更长");

    let mut a = subs_app(false);
    let mut subs = detail_subs();
    subs[0].model_slots.sonnet = long_model.into();
    a.update(subs_done(2, subs));
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter));
    let out = render(&mut a, 80, 24);
    assert!(out.contains(long_model), "80 列详情面板应该完整显示 30 字符的模型名, 不截断\n{out}");

    let mut b = subs_app(false);
    let mut subs2 = detail_subs();
    subs2[0].model_slots.sonnet = long_model.into();
    b.update(subs_done(2, subs2));
    let out2 = render(&mut b, 120, 40);
    assert!(out2.contains(long_model), "120×40 宽屏右侧详情面板同样应该完整显示\n{out2}");
}

// ---------- final-fix: M4 限额上限为 0 视为不限额 ----------

/// M4: `QuotaUsage::ratio()` 把 `limit == Some(0)` 当无限额处理, 限额行的过滤条件必须跟它同一
/// 套规则 (`tightest_quota` 已经是这样), 不能只看 `limit.is_some()`——否则会显示一条假的
/// "0%  n / 0" 限额行。
#[test]
fn a_configured_limit_of_zero_is_treated_as_unlimited() {
    let mut a = subs_app(false);
    let mut subs = detail_subs();
    subs[2].quota_usage = vec![quota_period(QuotaPeriod::Daily, 0, 5)]; // 示例中转: 唯一一条限额, limit=0
    a.update(subs_done(2, subs));
    render(&mut a, 80, 24); // 记住这是窄屏。
    a.handle_key(key(KeyCode::Char('G'))); // 选中示例中转
    a.handle_key(key(KeyCode::Enter));
    let out = render(&mut a, 80, 24);
    let row = out.lines().find(|l| l.contains(ZH.sub_f_quota)).unwrap_or_else(|| panic!("缺「限额」这一行\n{out}"));
    assert!(row.contains('—'), "limit=0 应该视为不限额, 显示占位符而不是假的百分比\n{out}");
    assert!(!out.contains("0%"), "{out}");
}

// ---------- final-fix: M2 折行字段的续行对齐 ----------

/// M2: 「最近错误」折行后, 续行必须从与首行相同的值列开始 (不能顶到列 0)——90 个 'x' 远超窄屏
/// 详情的值列宽度 (66), 保证真的会折成至少两行。
#[test]
fn wrapped_field_continuation_lines_share_the_value_column() {
    let mut a = subs_app(false);
    let mut subs = detail_subs();
    subs[1].last_error_message = Some("x".repeat(90));
    a.update(subs_done(2, subs));
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Char('j'))); // Kimi 备用
    a.handle_key(key(KeyCode::Enter));
    let out = render(&mut a, 80, 24);

    let lines: Vec<&str> = out.lines().map(plain).collect();
    let idx = lines
        .iter()
        .position(|l| l.trim_start_matches('│').trim_start().starts_with(ZH.sub_f_last_error))
        .unwrap_or_else(|| panic!("缺「最近错误」这一行\n{out}"));
    let value_col = value_column(lines[idx], ZH.sub_f_last_error);

    let continuation = lines[idx + 1];
    let content = continuation.trim_start_matches('│');
    assert!(!content.trim().is_empty(), "续行不该是空的 (说明确实折行了)\n{out}");
    // `value_column` 量出来的列号是从整行 (含开头的「│」边框) 算起的绝对列; 这里要用同一套基准,
    // 不能先把「│」切掉再数空格 (那样会系统性少数 1 列, 边框本身占的那一列)。
    let indent = 1 + continuation.chars().skip(1).take_while(|c| *c == ' ').count();
    assert_eq!(indent, value_col, "续行应该从与首行相同的值列开始, 不能顶到列 0\n{out}");
}

/// M2: 装不下 (超过 4 行 cap) 的「最近错误」不能被 `Paragraph` 的 `Wrap` 悄悄裁掉——必须由我们自己
/// 截断并在最后一行补省略号 (`clip_to_rows`)。300 个 'x' 远超 4 行 × 66 列的容量。
#[test]
fn a_very_long_last_error_ends_with_an_ellipsis_within_the_row_cap() {
    let mut a = subs_app(false);
    let mut subs = detail_subs();
    subs[1].last_error_message = Some("x".repeat(300));
    a.update(subs_done(2, subs));
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Char('j'))); // Kimi 备用
    a.handle_key(key(KeyCode::Enter));
    let out = render(&mut a, 80, 24);

    let lines: Vec<&str> = out.lines().map(plain).collect();
    let idx = lines
        .iter()
        .position(|l| l.trim_start_matches('│').trim_start().starts_with(ZH.sub_f_last_error))
        .unwrap_or_else(|| panic!("缺「最近错误」这一行\n{out}"));
    let last_row = lines[idx + 3]; // 4 行 cap: 第 0 行是标签所在行, 第 3 行是最后一行。
    let trimmed = last_row.trim_end_matches('│').trim_end();
    assert!(trimmed.ends_with('…'), "装不下的最近错误最后一行应该以省略号收尾\n{out}");
}

// ---------- final-fix: M6 列表状态列 + 停用行整体变灰 ----------

/// M6: 状态列只在留得出空间时才加进来。80×24 (窄屏, 列表独占整行) 有room, 应该看到「状态」表头
/// 与冷却倒计时；120×40 宽屏的左侧列表固定只有 58 列, 加上状态列会把 sonnet 挤到 12 列以下,
/// 按规则应该整列省略——sonnet 列 (`fit` 到定宽) 依然保留原来的宽度, 不因为省略状态列而跟着变。
#[test]
fn list_has_a_status_column_with_cooldown_when_there_is_room() {
    let narrow = render(&mut subs_app(false), 80, 24);
    assert!(narrow.contains(ZH.sub_col_state), "80 列有空间, 应该显示「状态」表头\n{narrow}");
    let kimi_row = narrow.lines().find(|l| l.contains("Kimi 备用")).unwrap_or_else(|| panic!("{narrow}"));
    assert!(kimi_row.contains("限流 · 00:42"), "状态列应该带冷却倒计时\n{kimi_row}");

    // 120×40: 右侧详情面板的「状态」字段行 (`sub_f_state`) 与列表的状态列表头是**同一个中文词**,
    // 直接在整页输出里找会被详情面板那份撞上——只看表头所在行、且只看双栏分界符 `││` 左边
    // (列表那一半) 的内容。
    let wide = render(&mut subs_app(false), 120, 40);
    let header_line = wide.lines().find(|l| l.contains(ZH.sub_col_name)).unwrap_or_else(|| panic!("缺表头行\n{wide}"));
    let list_half = header_line.split("││").next().unwrap_or(header_line);
    assert!(
        !list_half.contains(ZH.sub_col_state),
        "120×40 宽屏的左侧列表固定 58 列, 放不下状态列 (还要给 sonnet 留 ≥12 列), 应该整列省略\n{header_line}"
    );
}

/// M6: 手动停用的订阅整行都该是 muted 颜色, 不再各自套 badge 的语义色。
#[test]
fn disabled_rows_are_muted() {
    let mut a = subs_app(false);
    let mut subs = detail_subs();
    subs[1].enabled = false; // Kimi 备用 (provider_display_name = "Moonshot", 纯 ASCII, 方便按字符定位)
    a.update(subs_done(2, subs));
    render(&mut a, 80, 24);

    let buf = render_buffer(&mut a, 80, 24);
    let theme = Theme::new(ColorMode::TrueColor);
    let kimi_y = (0..buf.area.height)
        .find(|&y| buffer_row_text(&buf, y).contains("Kimi"))
        .unwrap_or_else(|| panic!("找不到 Kimi 备用所在的行"));

    let name_style = find_cell_style(&buf, kimi_y, "Kimi");
    assert_eq!(name_style.fg, Some(theme.muted), "停用订阅的备注名应该是 muted 颜色");
    let provider_style = find_cell_style(&buf, kimi_y, "Moonshot");
    assert_eq!(provider_style.fg, Some(theme.muted), "停用订阅的厂商也应该是 muted 颜色");

    // 对照组: 智谱主号 (下标 0, 紧挨在 Kimi 上面那一行) 没被停用, 该保留自己 badge 的颜色, 不受
    // 影响——用 sonnet 列 ("glm-4.6", 纯 ASCII) 断言, 避开 CJK 宽字符的续格在这个 ratatui 版本里
    // 重建成带空格文本 (`buffer_row_text` 逐格拼 `symbol()`, 宽字符续格是字面空格而不是空串,
    // "智谱主号" 会被拼成 "智 谱 主 号") 的干扰。
    let zhipu_y = kimi_y - 1;
    let zhipu_sonnet_style = find_cell_style(&buf, zhipu_y, "glm-4.6");
    assert_ne!(zhipu_sonnet_style.fg, Some(theme.muted), "没被停用的订阅不该被整行变灰\n{}", buffer_row_text(&buf, zhipu_y));
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

    // 订阅页忙碌行 (Task 4): spinner 是唯一读 tick 计数器的渲染路径, 同一帧画两遍必须得到同一个
    // 符号, 不能因为 throbber 步长算法而漂移。
    let mut d = subs_app(false);
    d.update(Action::Mutate(Mutation::TestConnection { id: "1".into() }));
    let first = render(&mut d, 80, 24);
    let second = render(&mut d, 80, 24);
    assert_eq!(first, second, "忙碌行 (spinner) 也应该幂等");

    // final-fix I1: 详情面板显示「上次操作」结果时也应该幂等。
    let mut e = subs_app(false);
    let mutation = Mutation::TestConnection { id: "1".into() };
    e.update(Action::Mutate(mutation.clone()));
    e.update(Action::MutationDone {
        mutation,
        result: Ok(MutationOutcome::Tested(TestConnectionResult {
            ok: true,
            message: "ok".into(),
            http_status: Some(200),
            model_used: None,
            state_reset: true,
        })),
    });
    render(&mut e, 80, 24);
    e.handle_key(key(KeyCode::Enter));
    let first = render(&mut e, 80, 24);
    let second = render(&mut e, 80, 24);
    assert_eq!(first, second, "详情面板显示上次操作结果时也应该幂等");
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

// ---------- 订阅页就地操作 (Task 4) ----------

/// `e` 在已启用的订阅上发「停用」, 在已停用的上发「启用」(目标值 = 当前 `enabled` 取反);
/// `t` / `m` / `b` 各自映射到对应的 `Mutation`。`detail_subs()` 的第一条 (id "1", 智谱主号) 默认启用。
#[test]
fn keys_on_the_subscriptions_page_map_to_mutations() {
    let mut a = subs_app(false);
    assert_eq!(
        a.handle_key(key(KeyCode::Char('e'))),
        Some(Action::Mutate(Mutation::SetEnabled { id: "1".into(), enabled: false })),
        "已启用的订阅上 e 应该发「停用」"
    );
    assert_eq!(a.handle_key(key(KeyCode::Char('t'))), Some(Action::Mutate(Mutation::TestConnection { id: "1".into() })));
    assert_eq!(a.handle_key(key(KeyCode::Char('m'))), Some(Action::Mutate(Mutation::RefreshModels { id: "1".into() })));
    assert_eq!(a.handle_key(key(KeyCode::Char('b'))), Some(Action::Mutate(Mutation::RefreshBalance { id: "1".into() })));

    // 停用后再按 e 应该发「启用」。
    let mut subs = detail_subs();
    subs[0].enabled = false;
    a.update(subs_done(2, subs));
    assert_eq!(
        a.handle_key(key(KeyCode::Char('e'))),
        Some(Action::Mutate(Mutation::SetEnabled { id: "1".into(), enabled: true })),
        "已停用的订阅上 e 应该发「启用」"
    );
}

/// 忙碌表在 `MutationDone` 落地时就清空了, 但 `Store` 里的 `enabled` 要等下一轮
/// `Cmd::Fetch(Subscriptions)` 真正跑完才会更新。这中间有个窗口: 第一次 `e` 的结果已经回来
/// (toast 弹过、忙碌表已清), 但刷新还没跑完, 这时马上再按一次 `e` 必须读到**乐观更新过**的
/// `enabled`, 朝反方向走——而不是从 `Store` 里的旧值重新算出同一个目标 (变成 no-op 请求,
/// 且因为 toast 文案与上一条相同被去重规则吞掉, 用户毫无反馈)。
#[test]
fn a_second_toggle_right_after_the_first_goes_the_other_way() {
    let mut a = subs_app(false); // id "1" (智谱主号) 默认 enabled=true
    assert_eq!(
        a.handle_key(key(KeyCode::Char('e'))),
        Some(Action::Mutate(Mutation::SetEnabled { id: "1".into(), enabled: false }))
    );
    let mutation = Mutation::SetEnabled { id: "1".into(), enabled: false };
    a.update(Action::Mutate(mutation.clone()));
    // 结果回来了 (忙碌表已清), 但 subs_done 刷新还没跑完——Store 里原始的 enabled=true 没变。
    a.update(Action::MutationDone { mutation, result: Ok(MutationOutcome::EnabledSet) });

    assert_eq!(
        a.handle_key(key(KeyCode::Char('e'))),
        Some(Action::Mutate(Mutation::SetEnabled { id: "1".into(), enabled: true })),
        "第一次 e 已经把它关掉了 (乐观更新), 这次 e 应该朝相反方向 (重新打开) 走"
    );
    assert_eq!(
        a.update(Action::Mutate(Mutation::SetEnabled { id: "1".into(), enabled: true })),
        vec![Cmd::Mutate(Mutation::SetEnabled { id: "1".into(), enabled: true })],
        "这次应该真的发出去, 不是 no-op"
    );
}

/// 同一订阅的操作同时只跑一个 (按订阅 id 判重, 不按 `Mutation` 整体): 连按两次 `t` 第二次被丢弃;
/// 另一条订阅同时 `t` 照发; `MutationDone` 之后可以再发。
#[test]
fn a_mutation_is_issued_once_per_subscription_until_it_finishes() {
    let mut a = subs_app(false);
    let m1 = Mutation::TestConnection { id: "1".into() };
    assert_eq!(a.update(Action::Mutate(m1.clone())), vec![Cmd::Mutate(m1.clone())]);
    assert!(a.update(Action::Mutate(m1.clone())).is_empty(), "同一订阅的第二次 t 应该被丢弃");

    // 忙碌表按订阅 id 判重, 不是按 `Mutation` 整体判重: 同一条订阅上换一种操作 (t 还在跑时按 e)
    // 也该被拒绝, 不能因为 Mutation 值不同就放过去。
    assert!(
        a.update(Action::Mutate(Mutation::SetEnabled { id: "1".into(), enabled: false })).is_empty(),
        "同一条订阅上, 另一种操作也该被忙碌表拒绝"
    );

    let m2 = Mutation::TestConnection { id: "2".into() };
    assert_eq!(a.update(Action::Mutate(m2)), vec![Cmd::Mutate(Mutation::TestConnection { id: "2".into() })], "另一条订阅应该照发");

    let result = Ok(MutationOutcome::Tested(TestConnectionResult {
        ok: true,
        message: "ok".into(),
        http_status: Some(200),
        model_used: None,
        state_reset: false,
    }));
    a.update(Action::MutationDone { mutation: m1.clone(), result });
    assert_eq!(a.update(Action::Mutate(m1.clone())), vec![Cmd::Mutate(m1)], "MutationDone 之后应该可以再发");
}

#[test]
fn mutations_are_refused_while_offline() {
    let mut a = app(false); // 还没连上 (Conn::Connecting), 不是 Connected
    assert!(a.update(Action::Mutate(Mutation::TestConnection { id: "1".into() })).is_empty());
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.toast_offline), "{out}");
}

/// 断线判定必须排在忙碌表前面: 一条订阅正忙着 (上一次操作还没等到 `MutationDone` 就掉线了) 时再按
/// 键, 也该看到「未连接」提示, 而不是被忙碌表的「已经在跑, 丢弃」悄悄吞掉、界面毫无反馈。
#[test]
fn a_key_press_on_a_busy_row_while_offline_still_shows_the_offline_toast() {
    let mut a = subs_app(false); // 已连接
    let m = Mutation::TestConnection { id: "1".into() };
    assert!(!a.update(Action::Mutate(m.clone())).is_empty(), "先让这条订阅忙起来");

    a.update(Action::ConnectionLost); // 忙碌表里的记录还在, 但现在断线了。
    assert!(a.update(Action::Mutate(m)).is_empty(), "断线时不该真的发请求");
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.toast_offline), "忙碌订阅断线时按键也该弹「未连接」\n{out}");
}

/// 「示例中转」(id "3") 的 `balance_supported = false`: `b` 应该就地回答, 不产生 `Cmd`。
#[test]
fn balance_refresh_on_an_unsupported_provider_is_answered_locally() {
    let mut a = subs_app(false);
    let cmds = a.update(Action::Mutate(Mutation::RefreshBalance { id: "3".into() }));
    assert!(cmds.is_empty(), "不支持余额查询的 provider 上 b 不该发请求");
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.sub_balance_unsupported), "{out}");
}

/// toast 文案表里的每一行各一个用例: 断言渲染里出现对应文案, 且都追加了 `Cmd::Fetch(Subscriptions)`。
#[test]
fn every_mutation_outcome_toasts_and_refetches() {
    let cases: Vec<(Mutation, Result<MutationOutcome, String>, &str)> = vec![
        (
            Mutation::SetEnabled { id: "1".into(), enabled: true },
            Ok(MutationOutcome::EnabledSet),
            "已启用 智谱主号",
        ),
        (
            Mutation::SetEnabled { id: "1".into(), enabled: false },
            Ok(MutationOutcome::EnabledSet),
            "已停用 智谱主号",
        ),
        (
            Mutation::TestConnection { id: "1".into() },
            Ok(MutationOutcome::Tested(TestConnectionResult {
                ok: true,
                message: "ok".into(),
                http_status: Some(200),
                model_used: Some("glm-4.6".into()),
                state_reset: true,
            })),
            "智谱主号：连接正常 (glm-4.6)",
        ),
        (
            Mutation::TestConnection { id: "1".into() },
            Ok(MutationOutcome::Tested(TestConnectionResult {
                ok: true,
                message: "ok".into(),
                http_status: Some(200),
                model_used: None,
                state_reset: true,
            })),
            "智谱主号：连接正常",
        ),
        (
            Mutation::TestConnection { id: "1".into() },
            Ok(MutationOutcome::Tested(TestConnectionResult {
                ok: false,
                message: "上游拒绝".into(),
                http_status: Some(401),
                model_used: Some("glm-4.6".into()),
                state_reset: false,
            })),
            "智谱主号：上游拒绝",
        ),
        (
            Mutation::RefreshModels { id: "1".into() },
            Ok(MutationOutcome::Models(RefreshModelsResult::Auto {
                models: vec![ModelInfo { id: "a".into(), display_name: None }, ModelInfo { id: "b".into(), display_name: None }],
                fetched_at: NOW,
            })),
            "智谱主号：获取到 2 个模型",
        ),
        (
            Mutation::RefreshModels { id: "1".into() },
            Ok(MutationOutcome::Models(RefreshModelsResult::ManualFallback { reason: "无 model_discovery".into() })),
            "智谱主号：无法自动获取模型 (无 model_discovery)",
        ),
        (
            Mutation::RefreshBalance { id: "1".into() },
            Ok(MutationOutcome::Balance(RefreshBalanceResult::Success {
                snapshot: BalanceSnapshot { is_available: None, entries: vec![] },
                fetched_at: NOW,
            })),
            "智谱主号：余额已刷新",
        ),
        (
            Mutation::RefreshBalance { id: "1".into() },
            Ok(MutationOutcome::Balance(RefreshBalanceResult::Failed { reason: "超时".into() })),
            "智谱主号：余额查询失败 (超时)",
        ),
        (
            Mutation::RefreshBalance { id: "1".into() },
            Ok(MutationOutcome::Balance(RefreshBalanceResult::Unsupported)),
            ZH.sub_balance_unsupported,
        ),
        (
            Mutation::TestConnection { id: "1".into() },
            Err("网络错误".into()),
            "智谱主号：操作失败 (网络错误)",
        ),
    ];
    for (mutation, result, expect) in cases {
        let mut a = subs_app(false);
        a.update(Action::Mutate(mutation.clone()));
        let cmds = a.update(Action::MutationDone { mutation, result });
        assert_eq!(cmds, vec![Cmd::Fetch(Fetch::Subscriptions)], "无论成败都该追加一次订阅列表刷新");
        let out = render(&mut a, 80, 24);
        assert!(out.contains(expect), "缺 {expect:?}\n{out}");
    }
}

#[test]
fn a_busy_row_shows_a_spinner_and_the_detail_says_what_is_running() {
    let mut a = subs_app(false);
    let before = render(&mut a, 80, 24); // 列表态, 三条都在。
    let zhipu_before = before.lines().find(|l| l.contains("智谱主号")).unwrap_or_else(|| panic!("{before}"));
    assert!(zhipu_before.contains('●'), "忙碌前应该显示健康符号\n{zhipu_before}");

    a.update(Action::Mutate(Mutation::TestConnection { id: "1".into() }));
    let busy_list = render(&mut a, 80, 24);
    let zhipu_busy = busy_list.lines().find(|l| l.contains("智谱主号")).unwrap_or_else(|| panic!("{busy_list}"));
    assert!(!zhipu_busy.contains('●'), "忙碌行不该再显示健康 badge 符号, 应该换成 spinner\n{zhipu_busy}");
    // 没在忙的行仍然照常显示自己的 badge 符号——忙碌只影响那一行, 不是整页。
    let kimi = busy_list.lines().find(|l| l.contains("Kimi 备用")).unwrap_or_else(|| panic!("{busy_list}"));
    assert!(kimi.contains('◐'), "没在忙的行应该保留自己的 badge 符号\n{kimi}");
    let relay = busy_list.lines().find(|l| l.contains("示例中转")).unwrap_or_else(|| panic!("{busy_list}"));
    assert!(relay.contains('✕'), "没在忙的行应该保留自己的 badge 符号\n{relay}");

    a.handle_key(key(KeyCode::Enter));
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.sub_busy_testing), "详情面板的状态行应该追加进行中文案\n{out}");
}

/// 简报规定四个就地操作在列表态与详情态都生效; 这里专门覆盖窄屏详情态打开时按 e/t/m/b 仍然
/// 能映射出正确的 `Action::Mutate` (不是只有列表态测过)。
#[test]
fn mutation_keys_work_inside_the_narrow_detail_view() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24); // 让页面记住这是窄屏。
    a.handle_key(key(KeyCode::Enter)); // 进入详情, 选中的是第一条 "智谱主号" (id "1", enabled=true)。
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.sub_f_endpoint), "应该已经在详情态\n{out}");

    assert_eq!(
        a.handle_key(key(KeyCode::Char('e'))),
        Some(Action::Mutate(Mutation::SetEnabled { id: "1".into(), enabled: false }))
    );
    assert_eq!(a.handle_key(key(KeyCode::Char('t'))), Some(Action::Mutate(Mutation::TestConnection { id: "1".into() })));
    assert_eq!(a.handle_key(key(KeyCode::Char('m'))), Some(Action::Mutate(Mutation::RefreshModels { id: "1".into() })));
    assert_eq!(a.handle_key(key(KeyCode::Char('b'))), Some(Action::Mutate(Mutation::RefreshBalance { id: "1".into() })));
}

/// 结果回来时订阅已经不在 `Store` 里了 (比如用户在别处删掉了这条订阅) —— toast 应该退回用 id。
#[test]
fn subscription_name_falls_back_to_the_id_in_toasts() {
    let mut a = subs_app(false);
    let mutation = Mutation::TestConnection { id: "1".into() };
    a.update(Action::Mutate(mutation.clone()));

    let mut subs = detail_subs();
    subs.remove(0); // id "1" 没了
    a.update(subs_done(2, subs));

    let result = Ok(MutationOutcome::Tested(TestConnectionResult {
        ok: true,
        message: "ok".into(),
        http_status: Some(200),
        model_used: None,
        state_reset: true,
    }));
    a.update(Action::MutationDone { mutation, result });
    let out = render(&mut a, 80, 24);
    assert!(out.contains("1：连接正常"), "订阅不在 Store 里时应该退回用 id\n{out}");
}

/// 80 列下页面键位 (e/t/m/b 等) 放不下时应该被裁掉, 但全局的 `?` 帮助 / `q` 退出必须一直在。
#[test]
fn global_keys_survive_at_80_columns_on_the_subscriptions_page() {
    let out = render(&mut subs_app(false), 80, 24);
    assert!(out.contains(ZH.key_help) && out.contains(ZH.key_quit), "{out}");

    // M5 fix: 详情态不再重复显示一份「Esc 返回」(面板右下角的标题已经有了), 腾出的空间应该够放
    // 下 `b 余额`。
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter));
    let detail_out = render(&mut a, 80, 24);
    let footer = detail_out.lines().last().unwrap_or_else(|| panic!("{detail_out}"));
    assert!(footer.contains(ZH.key_balance), "详情态键位栏应该放得下 b 余额\n{footer}");
    assert!(footer.contains(ZH.key_help) && footer.contains(ZH.key_quit), "{footer}");
}

// ---------- final-fix: I1 就地操作的完整结果落在详情面板 ----------

/// I1: 失败结果的完整文案只能在详情面板的「上次操作」行看到——toast 单行被截断 (≤72 列, 3 秒就
/// 消失)。120 个 'x' (安全落在窄屏详情 3 行 × 66 列 = 198 列的容量以内, 不会被 `clip_to_rows`
/// 二次截断) 用来确认「完整」这件事, 而不是像最近错误那样也可能需要省略号收尾。
#[test]
fn a_failed_test_connection_stays_readable_in_the_detail() {
    let long_message = "x".repeat(120);
    let mut a = subs_app(false);
    let mutation = Mutation::TestConnection { id: "1".into() };
    a.update(Action::Mutate(mutation.clone()));
    a.update(Action::MutationDone {
        mutation,
        result: Ok(MutationOutcome::Tested(TestConnectionResult {
            ok: false,
            message: long_message.clone(),
            http_status: Some(401),
            model_used: None,
            state_reset: false,
        })),
    });

    // toast 还在屏幕上 (刚发生, 没过期): 应该是被截断的一行, 不包含完整的 120 个 'x'。
    let toast_out = render(&mut a, 80, 24);
    assert!(!toast_out.contains(&long_message), "toast 应该被截断成一行, 不该塞下完整的 120 个 x\n{toast_out}");
    assert!(toast_out.contains('…'), "toast 截断后应该以省略号收尾\n{toast_out}");

    // 进详情前先让 toast 过期 (3 秒生命周期): 否则它的浮层边框会叠在刚打开的详情面板上面,
    // 挡住「上次操作」这一行, 跟本用例要验证的东西 (详情面板本身能不能显示完整文案) 无关。
    a.update(Action::Tick { now_ms: NOW + 3_000 });
    // 进详情: 「上次操作」行应该显示完整文案 (不截断), 不受 toast 单行限制。
    a.handle_key(key(KeyCode::Enter));
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.sub_f_last_action), "详情面板应该有「上次操作」这一行\n{out}");
    // 折行后的 120 个 'x' 会被拆到好几个渲染行里, 中间隔着每行末尾的引号 + 换行, 不能直接
    // `.contains(&long_message)`——改成数总的 'x' 个数 (面板里其它文字都不含 'x')。
    let x_count = out.chars().filter(|&c| c == 'x').count();
    assert_eq!(x_count, 120, "详情面板应该显示完整的 120 个 x (折行但不截断), 不像 toast 那样截断\n{out}");
}

/// I1: 发起新一次操作那一刻, 上一条 `last_outcome` 就该被清掉——不能让用户以为「上次操作」显示
/// 的是这次刚发出去的操作的结果 (正忙时应该显示的是「正在测试连接…」busy 文案)。
#[test]
fn last_outcome_is_cleared_when_a_new_mutation_starts() {
    let mut a = subs_app(false);
    let mutation = Mutation::TestConnection { id: "1".into() };
    a.update(Action::Mutate(mutation.clone()));
    a.update(Action::MutationDone {
        mutation: mutation.clone(),
        result: Ok(MutationOutcome::Tested(TestConnectionResult {
            ok: false,
            message: "上游拒绝".into(),
            http_status: Some(401),
            model_used: None,
            state_reset: false,
        })),
    });
    render(&mut a, 80, 24);
    // 让 toast 过期, 不然它的浮层边框会叠在刚打开的详情面板上面, 挡住这里要断言的文字。
    a.update(Action::Tick { now_ms: NOW + 3_000 });
    a.handle_key(key(KeyCode::Enter));
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.sub_f_last_action) && out.contains("上游拒绝"), "上一次操作结果应该出现在详情里\n{out}");

    a.handle_key(key(KeyCode::Esc));
    a.update(Action::Mutate(mutation));
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter));
    let out2 = render(&mut a, 80, 24);
    assert!(!out2.contains("上游拒绝"), "新一次操作发起后, 上一条结果应该被清掉\n{out2}");
    assert!(out2.contains(ZH.sub_busy_testing), "正忙时详情面板应该显示进行中文案\n{out2}");
}
