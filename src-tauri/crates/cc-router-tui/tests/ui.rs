//! 界面测试: `App` 的更新逻辑 + `TestBackend` 渲染结果。不碰网络、不碰真实终端。
//!
//! 快照在 `tests/snapshots/`。改了布局后先看 diff 再接受:
//!   INSTA_UPDATE=always cargo test -p cc-router-tui --test ui
//! 动效在快照里一律关闭 (`fx_enabled: false`), 否则第一帧是启动动效的中间态。

use std::cell::Cell;
use std::time::Duration;

use cc_router_tui::action::{Action, Cmd, Fetch, FetchData, Mutation, MutationOutcome, OnYes, OverviewData, Tab, WizardCmd, WizardResult};
use cc_router_tui::app::{App, AppOptions, MIN_HEIGHT};
use cc_router_tui::client::dto::{
    AuthHeaderFormat, BalanceCache, BalanceEntry, BalanceSeverity, BalanceSnapshot, CreateInput, CreateSource, CreatedSubscription,
    CustomProtocol, CustomSource, ModelCache, ModelDiscovery, ModelInfo, ModelSlots, OverallStats, ProbeInput, ProbeModelsResult, Provider,
    ProviderAuth, ProviderEndpoint, ProviderText, ProviderTranslations, ProxyStatus, UrlParam, QuotaPeriod, QuotaUsage, RefreshBalanceResult, RefreshModelsResult, RequestFilters,
    RequestLog, RequestPage, RequestQuery, RequestStatus, RoutingMode, SeriesPoint, Settings, SlotEfforts, Subscription, SubscriptionState,
    TestConnectionResult, VirtualModel, EFFORT_CHOICES,
};
use cc_router_tui::client::events::{ROUTE_ATTEMPT_FINISHED, ROUTE_ATTEMPT_STARTED};
use cc_router_tui::format::{fit, Tz};
use cc_router_tui::i18n::{strings, Lang, Strings, EN, JA, ZH};
use cc_router_tui::pages::Pages;
use cc_router_tui::secret::Secret;
use cc_router_tui::theme::{ColorMode, Theme};
use cc_router_tui::widgets::detail::{DetailRow, DetailSpec, Tone};
use cc_router_tui::widgets::picker::{self, PickerChoice, PickerItem, PickerSpec, PickerTag, Slot};
use cc_router_tui::widgets::toast::ToastKind;
use cc_router_tui::widgets::{confirm, help, toast};
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use ratatui::style::Modifier;
use ratatui::Terminal;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const NOW: i64 = 1_700_000_000_000;
const VERSION: &str = "9.9.9";

thread_local! {
    /// 这个测试用哪种语言构造 `App`; 默认中文。每个测试跑在自己的线程里, 设了也不会影响别的测试。
    static STRINGS: Cell<&'static Strings> = const { Cell::new(&ZH) };
}

/// 当前测试的界面文案 (见 [`use_lang`])。按文案找行 / 断言文字的辅助函数都经它取, 同一份驱动
/// 路径在任何语言下都能用。
fn s() -> &'static Strings {
    STRINGS.with(Cell::get)
}

/// 之后 [`app`] 构造的 `App` 都用 `lang` 的文案。
fn use_lang(lang: Lang) {
    STRINGS.with(|c| c.set(strings(lang)));
}

fn app(fx_enabled: bool) -> App {
    App::new(AppOptions {
        strings: s(),
        theme: Theme::new(ColorMode::TrueColor),
        fx_enabled,
        now_ms: NOW,
        tui_version: VERSION,
        tz: Tz::Fixed(8 * 3600),
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
        provider_names: None,
        enabled: true,
        state,
        cooldown_until: None,
        last_error_message: None,
        is_dispatchable: state == SubscriptionState::Healthy,
        quota_usage: vec![],
        provider_id: "p".into(),
        base_url: "https://example.invalid".into(),
        auth_type: "api_key".into(),
        endpoint_protocol: "messages".into(),
        model_slots: ModelSlots { fable: "d".into(), opus: "a".into(), sonnet: "b".into(), haiku: "c".into(), fallback: String::new(), jev: String::new() },
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
        ModelSlots { fable: "glm-4.6".into(), opus: "glm-4.6".into(), sonnet: "glm-4.6".into(), haiku: "glm-4.5-air".into(), fallback: String::new(), jev: String::new() };
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
        jev: String::new(),
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

/// 12 个模型条目, 给 picker 快照 (`picker_popup_80x24`) 与其它 picker 相关测试共用——够长, 能看出
/// 输入 "gl" 过滤后只剩 glm 系列的效果。
fn picker_items() -> Vec<PickerItem> {
    [
        ("glm-4.6", "GLM 4.6 主力"),
        ("glm-4.5-air", "GLM 4.5 Air"),
        ("glm-4.5", "GLM 4.5"),
        ("glm-4.5-flash", "GLM 4.5 Flash"),
        ("claude-3-opus", "Claude 3 Opus"),
        ("claude-3-sonnet", "Claude 3 Sonnet"),
        ("gpt-4o", "GPT-4o"),
        ("gpt-4o-mini", "GPT-4o mini"),
        ("o3", "o3"),
        ("o3-mini", "o3-mini"),
        ("deepseek-chat", "DeepSeek Chat"),
        ("deepseek-reasoner", "DeepSeek Reasoner"),
    ]
    .into_iter()
    .map(|(id, label)| PickerItem { id: id.into(), label: label.into(), hint: None })
    .collect()
}

fn picker_spec() -> PickerSpec {
    PickerSpec {
        tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Sonnet },
        title: "选择模型".into(),
        items: picker_items(),
        allow_custom: true,
        initial: String::new(),
    }
}

/// Task 1: `Popup::Detail` 的快照/滚动测试共用的一份内容——「基本信息」8 个字段 (其中「端点」值
/// 长到会折行), 「错误信息」两行 `Text`, 「上游响应」一段足够长的 `Text` 让内容超出一屏、逼出
/// 滚动条。「上游响应」用递增的四位数字拼接 (而不是重复字符), 好让 `detail_popup_scrolls_and_closes`
/// 断言「滚到底能看到末尾内容」时有真正区分度——重复字符滚到哪里看起来都一样。
fn detail_fixture() -> DetailSpec {
    DetailSpec {
        title: "请求详情".into(),
        rows: vec![
            DetailRow::Section("基本信息".into()),
            DetailRow::Field { label: "ID".into(), value: "req-12345".into(), tone: Tone::Normal },
            DetailRow::Field { label: "时间".into(), value: "2026-09-20 10:00:00".into(), tone: Tone::Normal },
            DetailRow::Field { label: "方法".into(), value: "POST".into(), tone: Tone::Normal },
            DetailRow::Field { label: "路径".into(), value: "/v1/messages".into(), tone: Tone::Normal },
            DetailRow::Field { label: "状态".into(), value: "200".into(), tone: Tone::Ok },
            DetailRow::Field { label: "模型".into(), value: "model-sonnet".into(), tone: Tone::Normal },
            DetailRow::Field { label: "订阅".into(), value: "智谱主号".into(), tone: Tone::Normal },
            DetailRow::Field {
                label: "端点".into(),
                value: "https://open.bigmodel.cn/api/anthropic/v1/messages?stream=true&client=cc-router-tui".into(),
                tone: Tone::Normal,
            },
            DetailRow::Section("错误信息".into()),
            DetailRow::Text { text: "上游返回 429 Too Many Requests\n已达到本分钟请求数上限, 请稍后重试".into(), tone: Tone::Err },
            DetailRow::Section("上游响应".into()),
            DetailRow::Text { text: (0..500).map(|i| format!("{i:04}")).collect(), tone: Tone::Normal },
        ],
    }
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

// ---------- 新建订阅向导: 加载 ----------

/// 向导刚打开、厂商列表还没拉回来的画面: 带边框的空容器 + 居中的「正在获取厂商列表…」+ throbber。
#[test]
fn wizard_loading_80x24() {
    let mut a = app(false);
    a.update(Action::OpenWizard);
    insta::assert_snapshot!(render(&mut a, 80, 24));
}

/// 厂商列表拉取失败: 容器内容换成居中的错误文案, 只能 `Esc` 退出——不开新快照 (与加载中的容器
/// 骨架一样, 只是正文文字不同), 用 `contains` 断言内容, 用 `handle_key` 断言 Esc 行为。
#[test]
fn wizard_load_failure_shows_the_reason_and_esc_still_closes() {
    let mut a = app(false);
    a.update(Action::OpenWizard);
    a.update(wizard_done(&a, WizardResult::Providers(Err("网络错误".into()))));
    let out = render(&mut a, 80, 24);
    assert!(out.contains(&(ZH.wiz_load_failed)("网络错误")), "{out}");
    assert!(!out.contains(ZH.wiz_loading_providers), "失败之后不该还显示加载中的文案\n{out}");

    assert_eq!(a.handle_key(key(KeyCode::Esc)), Some(Action::CloseWizard));
    assert_eq!(a.update(Action::CloseWizard), vec![Cmd::Fetch(Fetch::Subscriptions)]);
}

// ---------- 新建订阅向导: 第一步 (内置厂商) ----------

/// 内置厂商 (智谱 AI) 的假 `Provider`: 两个接入点, 默认选中国内版。`example_models` 非空:
/// `wizard_at_slots_with_manual_fallback` 这类「自动发现失败」场景要用它验证槽位 picker 退回
/// 手填候选这条路径。
fn zhipu_provider() -> Provider {
    Provider {
        id: "zhipu".into(),
        display_name: "智谱 AI".into(),
        description: Some("智谱 AI 大模型".into()),
        endpoints: vec![
            ProviderEndpoint { id: "cn".into(), label: "国内版".into(), base_url: "https://open.bigmodel.cn/api/anthropic".into(), protocol: "messages".into(), example_models: vec![], url_params_used: vec![] },
            ProviderEndpoint { id: "intl".into(), label: "国际版".into(), base_url: "https://api.z.ai/api/anthropic".into(), protocol: "messages".into(), example_models: vec![], url_params_used: vec![] },
        ],
        default_endpoint: Some("cn".into()),
        auth: ProviderAuth { auth_type: "api_key".into() },
        model_discovery: ModelDiscovery { enabled: true, example_models: vec!["glm-4-plus".into(), "glm-4.5-flash".into()] },
        // 向导收到的是 runtime 已经按界面语言换好的列表 (`Provider::localized`), 这里的译文不会上屏。
        translations: ProviderTranslations { en: provider_text("Zhipu AI"), ja: provider_text("Zhipu AI") },
        url_params: vec![],
    }
}

fn provider_text(name: &str) -> ProviderText {
    ProviderText { display_name: name.into(), description: None, endpoints: Default::default(), url_params: Default::default() }
}

/// OAuth 类厂商 (ChatGPT), TUI 不做设备码流程, 选中只给提示、不设值。
fn chatgpt_provider() -> Provider {
    Provider {
        id: "openai_codex".into(),
        display_name: "ChatGPT".into(),
        description: None,
        endpoints: vec![],
        default_endpoint: None,
        auth: ProviderAuth { auth_type: "chatgpt_oauth".into() },
        model_discovery: ModelDiscovery { enabled: false, example_models: vec![] },
        translations: ProviderTranslations { en: provider_text("ChatGPT"), ja: provider_text("ChatGPT") },
        url_params: vec![],
    }
}

/// 向导请求的结果, 带着当前向导的代次——「这个向导自己发的请求回来了」。
fn wizard_done(a: &App, result: WizardResult) -> Action {
    Action::WizardDone { epoch: a.wizard_epoch().expect("准备: 向导应该开着"), result: Box::new(result) }
}

/// 当前向导发出的一个请求 (带着它的代次)。
fn wizard_cmd(a: &App, cmd: Box<WizardCmd>) -> Cmd {
    Cmd::Wizard { epoch: a.wizard_epoch().expect("准备: 向导应该开着"), cmd }
}

/// 打开向导并喂一份厂商列表, 停在第一步、焦点在 `Provider` 行。
fn wizard_with_providers(providers: Vec<Provider>) -> App {
    let mut a = app(false);
    a.update(Action::OpenWizard);
    a.update(wizard_done(&a, WizardResult::Providers(Ok(providers))));
    a
}

/// 打开向导、拉到厂商列表、从厂商 picker 里选中 `custom:<protocol>` 条目, 停在自定义单页、
/// 焦点在 `ProviderName`——自定义路径用例的公共起点, 与 `wizard_with_providers`/`select_zhipu`
/// 对内置路径的角色相同。厂商列表本身与自定义路径无关, 只是复用同一份 `zhipu_provider()` 夹具
/// (不需要为这里单独造一份空列表)。
fn wizard_custom(protocol: CustomProtocol) -> App {
    let mut a = app(false);
    open_custom_wizard(&mut a, protocol);
    a
}

/// 在已有的 `App` 上打开一个新向导并走到自定义单页 (`wizard_custom` 的本体)——跨向导实例的用例
/// 要在同一个 `App` 上先后开两个向导。
fn open_custom_wizard(a: &mut App, protocol: CustomProtocol) {
    a.update(Action::OpenWizard);
    let providers = wizard_done(a, WizardResult::Providers(Ok(vec![zhipu_provider()])));
    a.update(providers);
    let open_action = a.handle_key(key(KeyCode::Enter)).expect("Provider 行 ⏎ 应该产出 Action::OpenPicker");
    a.update(open_action);
    a.update(Action::PickerDone { tag: PickerTag::WizardProvider, choice: PickerChoice::Item(format!("custom:{}", protocol.as_wire())) });
}

/// 有输入的向导按 `Esc` → 确认弹窗选「是」, 向导关掉。
fn escape_and_confirm(a: &mut App) {
    let esc = a.handle_key(key(KeyCode::Esc)).expect("Esc 应该可用");
    a.update(esc);
    let yes = a.handle_key(key(KeyCode::Char('y'))).expect("确认弹窗里 y 应该产出 Action");
    a.update(yes);
    assert!(a.wizard_epoch().is_none(), "准备: 向导应该已经关掉");
}

/// `Provider` 行 `⏎` → 打开厂商 picker → 选中智谱 AI。选中后焦点应该已经移到 `ApiKey`
/// (`apply_provider_choice` 的行为), 后续测试从这里接着打字。
fn select_zhipu(a: &mut App) {
    let open_action = a.handle_key(key(KeyCode::Enter)).expect("Provider 行 ⏎ 应该产出 Action::OpenPicker");
    a.update(open_action);
    a.update(Action::PickerDone { tag: PickerTag::WizardProvider, choice: PickerChoice::Item("zhipu".into()) });
}

fn type_str(a: &mut App, text: &str) {
    for c in text.chars() {
        a.handle_key(key(KeyCode::Char(c)));
    }
}

/// 表单最多有多少行可聚焦 (自定义单页 14 行), 留足余量——`focus_row` 按这么多次 `↑` 保证到顶,
/// 再最多按这么多次 `↓` 找目标行。
const FORM_ROW_LIMIT: usize = 30;

/// 当前焦点是不是停在 `label` 那一行: 字段行画成 `"▌ <标签> "` (标签列定宽, 后面至少一格空白,
/// 所以「厂商」不会误中「厂商名」); 按钮行没有 `▌`, 聚焦时只有 `[ 标签 ]` 那一段反色——用
/// buffer 里 `[` 那一格的 `REVERSED` 判断。按真实渲染结果判, 不依赖向导内部的字段枚举。
fn focused_row_is(a: &mut App, label: &str) -> bool {
    let buf = render_buffer(a, 80, 24);
    let field = format!("▌ {label} ");
    let button = format!("[ {label} ]");
    (0..buf.area.height).any(|y| {
        // 不用 `buffer_row_text`: 宽字符后面的延续格在 `TestBackend` 里是 `" "`, 拼出来是
        // 「厂 商」, 这里按符号宽度跳过延续格, 拼回屏幕上看到的样子。
        let mut text = String::new();
        let mut x = 0;
        while x < buf.area.width {
            let sym = buf[(x, y)].symbol();
            text.push_str(sym);
            x += sym.width().max(1) as u16;
        }
        text.contains(&field)
            || (text.contains(&button)
                && (0..buf.area.width).any(|x| buf[(x, y)].symbol() == "[" && buf[(x, y)].style().add_modifier.contains(Modifier::REVERSED)))
    })
}

fn assert_focus(a: &mut App, label: &str) {
    assert!(focused_row_is(a, label), "焦点应该停在「{label}」行\n{}", render(a, 80, 24));
}

/// 把焦点挪到 `label` 那一行: 先按 `FORM_ROW_LIMIT` 次 `↑` 到顶 (焦点移动在两端夹住, 多按无害),
/// 再逐次 `↓` 直到渲染结果里的焦点行就是它——不再靠注释数步数。找不到就 panic 并打印屏幕。
fn focus_row(a: &mut App, label: &str) {
    for _ in 0..FORM_ROW_LIMIT {
        a.handle_key(key(KeyCode::Up));
    }
    for _ in 0..FORM_ROW_LIMIT {
        if focused_row_is(a, label) {
            return;
        }
        a.handle_key(key(KeyCode::Down));
    }
    panic!("按了 {FORM_ROW_LIMIT} 次 ↓ 也没找到「{label}」行\n{}", render(a, 80, 24));
}

/// 槽位行的标签 (与 `format::slot_label` 同值; 那个函数不公开)。
fn slot_row(slot: Slot) -> &'static str {
    match slot {
        Slot::Fable => "fable",
        Slot::Opus => "opus",
        Slot::Sonnet => "sonnet",
        Slot::Haiku => "haiku",
        Slot::Fallback => s().sub_slot_fallback,
        Slot::Jev => "jev",
    }
}

/// 自定义表单的文本字段, `None` = 不碰这个字段 (保留预设 / 自动跟随出来的值)。
#[derive(Default)]
struct CustomFill<'a> {
    provider_name: Option<&'a str>,
    base_url: Option<&'a str>,
    messages_path: Option<&'a str>,
    api_key: Option<&'a str>,
    display_name: Option<&'a str>,
}

/// 最常用的一份: 厂商名「中转站」/ `https://relay.example.com` / `sk-test`, 其余保留。
fn relay_fill() -> CustomFill<'static> {
    CustomFill { provider_name: Some("中转站"), base_url: Some("https://relay.example.com"), api_key: Some("sk-test"), ..Default::default() }
}

/// 按表单顺序把 `f` 里给了值的文本字段**设成**那个值 (`Ctrl+U` 清空整行再逐字输入——与用户手打
/// 同一条按键路径, 不是直接改草稿)。厂商名先于备注名填, 所以给了 `display_name` 时它覆盖自动
/// 跟随出来的值。
fn fill_custom(a: &mut App, f: CustomFill) {
    let fields = [
        (s().wiz_f_provider_name, f.provider_name),
        (s().wiz_f_base_url, f.base_url),
        (s().wiz_f_messages_path, f.messages_path),
        (s().wiz_f_api_key, f.api_key),
        (s().wiz_f_display_name, f.display_name),
    ];
    for (label, value) in fields {
        if let Some(value) = value {
            focus_row(a, label);
            a.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
            type_str(a, value);
        }
    }
}

/// 自定义表单: 依次在四个核心槽上 `⏎` 开 picker 并选 `choice`。
fn pick_core_slots(a: &mut App, choice: impl Fn(Slot) -> PickerChoice) {
    for slot in [Slot::Fable, Slot::Opus, Slot::Sonnet, Slot::Haiku] {
        focus_row(a, slot_row(slot));
        let open = a.handle_key(key(KeyCode::Enter)).expect("槽位行 ⏎ 应该开 picker");
        a.update(open);
        a.update(Action::PickerDone { tag: PickerTag::WizardSlot { slot }, choice: choice(slot) });
    }
}

/// 填完第一步 (智谱 AI / `sk-test`) 并提交, 返回那次提交产出的 `Action` (调用方按需再
/// `a.update(...)` 一次, 创建请求即在飞)。
fn submit_basics(a: &mut App) -> Action {
    select_zhipu(a);
    type_str(a, "sk-test");
    focus_row(a, s().wiz_btn_next);
    a.handle_key(key(KeyCode::Enter)).expect("填完表单提交应该产出 Action")
}

/// 打开向导, 走完第一步 → 提交 → `Created(Ok)` (id 固定 `"sub-1"`), 停在等模型列表 (已经拿到
/// id, 文案是 `wiz_loading_models`)。第二步的状态流转测试从这里接着喂不同的 `Models` 结果。
fn wizard_after_create() -> App {
    let mut a = wizard_with_providers(vec![zhipu_provider()]);
    let submit_action = submit_basics(&mut a);
    a.update(submit_action); // 创建在飞
    a.update(wizard_done(&a, WizardResult::Created(Ok(CreatedSubscription { id: "sub-1".into() })))); // 等模型列表
    a
}

/// 同上, 再喂一份 `Models(Ok(Auto))`, 停在第二步、聚焦 `Fable` 行——多个用例 (幂等测试、快照、
/// 保存) 共用这条驱动路径, 只是候选模型列表不同。
fn wizard_at_slots(models: Vec<ModelInfo>) -> App {
    let mut a = wizard_after_create();
    a.update(wizard_done(&a, WizardResult::Models {
        id: "sub-1".into(),
        result: Ok(RefreshModelsResult::Auto { models, fetched_at: 0 }),
    }));
    a
}

/// 同上, 但模拟自动发现失败 (`ManualFallback`): 候选为空, 说明行是 `reason`。
fn wizard_at_slots_with_manual_fallback(reason: &str) -> App {
    let mut a = wizard_after_create();
    a.update(wizard_done(&a, WizardResult::Models {
        id: "sub-1".into(),
        result: Ok(RefreshModelsResult::ManualFallback { reason: reason.to_string() }),
    }));
    a
}

/// 80×24: 厂商已选 (智谱 AI / 国内版, 都由选厂商时自动填好)、API Key 已填两个字符 ("sk")、
/// 备注名是自动生成的默认值。**焦点落在 `ApiKey`** (选厂商后的自然结果)。
#[test]
fn wizard_basics_80x24() {
    let mut a = wizard_with_providers(vec![zhipu_provider()]);
    select_zhipu(&mut a);
    type_str(&mut a, "sk");
    insta::assert_snapshot!(render(&mut a, 80, 24));
}

/// Cloudflare 形状的假厂商: `direct` 端点要 account_id, `gateway` 端点再要 gateway_id。
fn cloudflare_provider() -> Provider {
    let ep = |id: &str, used: &[&str]| ProviderEndpoint {
        id: id.into(),
        label: id.into(),
        base_url: "https://api.cloudflare.com".into(),
        protocol: "messages".into(),
        example_models: vec![],
        url_params_used: used.iter().map(|s| s.to_string()).collect(),
    };
    Provider {
        id: "cloudflare".into(),
        display_name: "Cloudflare".into(),
        description: None,
        endpoints: vec![ep("direct", &["account_id"]), ep("gateway", &["account_id", "gateway_id"])],
        default_endpoint: Some("direct".into()),
        auth: ProviderAuth { auth_type: "openai_chat_completions_api_key".into() },
        model_discovery: ModelDiscovery { enabled: true, example_models: vec![] },
        translations: ProviderTranslations { en: provider_text("Cloudflare"), ja: provider_text("Cloudflare") },
        url_params: vec![
            UrlParam { id: "account_id".into(), label: "账户 ID".into(), placeholder: None, pattern: "^[0-9a-f]{32}$".into() },
            UrlParam { id: "gateway_id".into(), label: "网关 ID".into(), placeholder: None, pattern: "^[A-Za-z0-9_-]{1,64}$".into() },
        ],
    }
}

fn select_cloudflare(a: &mut App) {
    let open_action = a.handle_key(key(KeyCode::Enter)).expect("Provider 行 ⏎ 应该产出 Action::OpenPicker");
    a.update(open_action);
    a.update(Action::PickerDone { tag: PickerTag::WizardProvider, choice: PickerChoice::Item("cloudflare".into()) });
}

/// 选了带 URL 参数的厂商: 接入点行下面多出一行「账户 ID」, 切到 gateway 端点再多一行「网关 ID」,
/// 已经填的账户 ID 保留。
#[test]
fn wizard_basics_with_url_params_80x24() {
    let mut a = wizard_with_providers(vec![cloudflare_provider()]);
    select_cloudflare(&mut a);
    focus_row(&mut a, "账户 ID");
    type_str(&mut a, "0123456789abcdef0123456789abcdef");
    insta::assert_snapshot!("wizard_basics_url_param_direct_80x24", render(&mut a, 80, 24));

    a.update(Action::PickerDone { tag: PickerTag::WizardEndpoint, choice: PickerChoice::Item("gateway".into()) });
    insta::assert_snapshot!("wizard_basics_url_param_gateway_80x24", render(&mut a, 80, 24));
}

/// 参数没填或格式不对时提交被拦在参数行上, 通过后 `Create` 带着 `url_params` (trim 后)。
#[test]
fn url_params_are_validated_and_sent_with_the_create_command() {
    let mut a = wizard_with_providers(vec![cloudflare_provider()]);
    select_cloudflare(&mut a);
    focus_row(&mut a, ZH.wiz_f_api_key);
    type_str(&mut a, "sk-test");

    focus_row(&mut a, ZH.wiz_btn_next);
    assert_eq!(a.handle_key(key(KeyCode::Enter)), None, "账户 ID 为空不该发请求");
    assert!(render(&mut a, 80, 24).contains(ZH.wiz_err_url_param_empty), "应该提示请填写此项");
    assert_focus(&mut a, "账户 ID");

    type_str(&mut a, "xyz");
    focus_row(&mut a, ZH.wiz_btn_next);
    assert_eq!(a.handle_key(key(KeyCode::Enter)), None, "格式不对不该发请求");
    assert!(render(&mut a, 80, 24).contains(ZH.wiz_err_url_param_format), "应该提示格式不正确");

    focus_row(&mut a, "账户 ID");
    a.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    type_str(&mut a, "  0123456789abcdef0123456789abcdef  ");
    focus_row(&mut a, ZH.wiz_btn_next);
    let submit = a.handle_key(key(KeyCode::Enter)).expect("提交应该产出 Action");
    let Action::WizardRequest(cmd) = submit else { panic!("应该是 WizardRequest, 实际 {submit:?}") };
    let WizardCmd::Create(input) = *cmd else { panic!("应该是 Create") };
    let CreateSource::Builtin { url_params, .. } = input.source else { panic!("应该是 Builtin") };
    assert_eq!(url_params.get("account_id").map(String::as_str), Some("0123456789abcdef0123456789abcdef"));
    assert_eq!(url_params.len(), 1);
}

/// 选 `chatgpt_oauth` 那一项之后 `provider_id` 仍然是空 (`Endpoint` 行 `⏎` 应该被拒绝, 而不是
/// 打开一个空的接入点列表), 且屏幕上出现 `ZH.wiz_desktop_only`。
#[test]
fn picking_an_oauth_provider_only_explains_where_to_add_it() {
    let mut a = wizard_with_providers(vec![chatgpt_provider()]);
    let open_action = a.handle_key(key(KeyCode::Enter)).expect("Provider 行 ⏎ 应该产出 Action::OpenPicker");
    a.update(open_action);
    a.update(Action::PickerDone { tag: PickerTag::WizardProvider, choice: PickerChoice::Item("openai_codex".into()) });
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.wiz_desktop_only), "选中 OAuth 厂商应该显示「请在桌面端添加」\n{out}");

    assert_focus(&mut a, ZH.wiz_f_provider); // 焦点没有因为选 OAuth 厂商而移动
    focus_row(&mut a, ZH.wiz_f_endpoint);
    assert_eq!(
        a.handle_key(key(KeyCode::Enter)),
        Some(Action::Notify { kind: ToastKind::Info, text: ZH.wiz_pick_provider_first.to_string() }),
        "provider_id 没被设置, 接入点行应该拒绝而不是打开空列表"
    );
}

/// 填完四个字段、焦点到按钮、`⏎` → 返回的 `Cmd` 是 `Wizard(Create(..))`, 且 `model_slots` 四个
/// 核心槽是 "(pending)"、fallback 是空串、source 是 `Builtin`。
#[test]
fn the_wizard_builds_a_create_command_with_pending_slots() {
    let mut a = wizard_with_providers(vec![zhipu_provider()]);
    select_zhipu(&mut a);
    type_str(&mut a, "sk-test");
    focus_row(&mut a, ZH.wiz_btn_next);
    let submit_action = a.handle_key(key(KeyCode::Enter)).expect("填完表单提交应该产出 Action");
    let cmds = a.update(submit_action);
    assert_eq!(
        cmds,
        vec![wizard_cmd(&a, Box::new(WizardCmd::Create(CreateInput {
            display_name: "智谱 AI".into(),
            api_key: Secret::new("sk-test"),
            model_slots: ModelSlots::pending(),
            source: CreateSource::Builtin { provider_id: "zhipu".into(), endpoint_id: "cn".into(), url_params: Default::default() },
        })))]
    );
}

/// 校验按 trim 后判空, 发出去的也必须是 trim 后的值: 粘贴带进来的首尾空白不该落进备注名或
/// API Key。
#[test]
fn the_builtin_create_sends_trimmed_values() {
    let mut a = wizard_with_providers(vec![zhipu_provider()]);
    select_zhipu(&mut a);
    type_str(&mut a, "  sk-test  ");
    focus_row(&mut a, ZH.wiz_f_display_name);
    a.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    type_str(&mut a, "  我的智谱  ");
    focus_row(&mut a, ZH.wiz_btn_next);
    let submit = a.handle_key(key(KeyCode::Enter)).expect("填完表单提交应该产出 Action");
    let Action::WizardRequest(cmd) = submit else { panic!("应该是 WizardRequest, 实际 {submit:?}") };
    let WizardCmd::Create(input) = *cmd else { panic!("应该是 Create") };
    assert_eq!(input.display_name, "我的智谱");
    assert_eq!(input.api_key, Secret::new("sk-test"));
}

/// 同上, 自定义路径: 厂商名 / 备注名 / API Key 都发 trim 后的值, 探测也一样。
#[test]
fn the_custom_probe_and_create_send_trimmed_values() {
    let mut a = wizard_custom(CustomProtocol::Anthropic);
    fill_custom(
        &mut a,
        CustomFill {
            provider_name: Some("  中转站  "),
            base_url: Some("https://relay.example.com"),
            api_key: Some(" sk-test "),
            display_name: Some("  我的中转  "),
            ..Default::default()
        },
    );
    focus_row(&mut a, ZH.wiz_btn_probe);
    let probe = a.handle_key(key(KeyCode::Enter)).expect("Probe 应该产出 Action");
    let Action::WizardRequest(cmd) = probe else { panic!("应该是 WizardRequest, 实际 {probe:?}") };
    let WizardCmd::Probe(input) = *cmd else { panic!("应该是 Probe") };
    assert_eq!(input.api_key, Secret::new("sk-test"), "探测也该发 trim 后的 key");
    // 探测结果回来之前表单只读, 让它失败回到可编辑状态。
    a.update(wizard_done(&a, WizardResult::Probed { base_url: "https://relay.example.com".into(), result: Err("网络错误".into()) }));

    pick_core_slots(&mut a, |_| PickerChoice::Custom("glm-4.6".into()));
    focus_row(&mut a, ZH.wiz_btn_create);
    let submit = a.handle_key(key(KeyCode::Enter)).expect("创建应该产出 Action");
    let Action::WizardRequest(cmd) = submit else { panic!("应该是 WizardRequest, 实际 {submit:?}") };
    let WizardCmd::Create(input) = *cmd else { panic!("应该是 Create") };
    assert_eq!(input.display_name, "我的中转");
    assert_eq!(input.api_key, Secret::new("sk-test"));
    let CreateSource::Custom(custom) = input.source else { panic!("应该是 Custom source") };
    assert_eq!(custom.provider_display_name, "中转站");
}

/// 不填 key 直接提交 → 不产出 `Cmd`, 屏幕上出现 `ZH.wiz_err_api_key`, 焦点跳回 `ApiKey`。
#[test]
fn submitting_an_incomplete_form_moves_the_cursor_to_the_bad_field() {
    let mut a = wizard_with_providers(vec![zhipu_provider()]);
    select_zhipu(&mut a);
    focus_row(&mut a, ZH.wiz_btn_next); // key 还是空的
    assert!(a.handle_key(key(KeyCode::Enter)).is_none(), "校验失败不该产出 Action");
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.wiz_err_api_key), "{out}");
    assert_focus(&mut a, ZH.wiz_f_api_key);
}

/// 打字之后屏幕上是 `••`; `Ctrl+R` 之后是明文; 再 `Ctrl+R` 又变回掩码。
#[test]
fn the_api_key_is_masked_until_ctrl_r() {
    let mut a = wizard_with_providers(vec![zhipu_provider()]);
    select_zhipu(&mut a);
    type_str(&mut a, "sk");

    let masked = render(&mut a, 80, 24);
    assert!(masked.contains("••"), "{masked}");
    assert!(!masked.contains("sk"), "按 Ctrl+R 之前不该看到明文\n{masked}");

    let ctrl_r = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
    a.handle_key(ctrl_r);
    let revealed = render(&mut a, 80, 24);
    assert!(revealed.contains("sk"), "{revealed}");
    assert!(!revealed.contains("••"), "{revealed}");

    a.handle_key(ctrl_r);
    let masked_again = render(&mut a, 80, 24);
    assert!(masked_again.contains("••"), "{masked_again}");
    assert!(!masked_again.contains("sk"), "{masked_again}");
}

/// 掩码不能封顶 —— 108 字符的 key (Anthropic 实测长度) 掩码后
/// 值区里应该还能看到点、光标落在点串末尾一格 (紧跟其后的空位); 左移几下光标应该跟着点串一起
/// 移动, 不会飞到空白区域 (那正是封顶版本会出的问题: 掩码文本比明文短, 光标按明文位置算却找
/// 不到对应的点)。
#[test]
fn a_long_api_key_keeps_the_cursor_aligned_with_the_mask() {
    let mut a = wizard_with_providers(vec![zhipu_provider()]);
    select_zhipu(&mut a);
    let long_key = "x".repeat(108);
    type_str(&mut a, &long_key);

    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|f| a.draw(f, Duration::ZERO)).unwrap();
    let out = terminal.backend().to_string();
    assert!(out.contains('•'), "108 字符的 key 掩码后应该还能看到点, 不该被封顶成空\n{out}");
    let cursor = terminal.backend().cursor_position();
    let buf = terminal.backend().buffer();
    assert_eq!(buf[(cursor.x, cursor.y)].symbol(), " ", "光标应该落在点串之后的空位, 不是盖在最后一个点上面");
    assert_eq!(buf[(cursor.x - 1, cursor.y)].symbol(), "•", "光标前一格应该是点, 不该飞到空白区域");

    for _ in 0..5 {
        a.handle_key(key(KeyCode::Left));
    }
    let mut terminal2 = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal2.draw(|f| a.draw(f, Duration::ZERO)).unwrap();
    let cursor2 = terminal2.backend().cursor_position();
    let buf2 = terminal2.backend().buffer();
    assert_eq!(buf2[(cursor2.x, cursor2.y)].symbol(), "•", "左移之后光标应该仍然落在一个点上, 不是空白\n{}", terminal2.backend());
}

/// 重选同一个厂商不该把用户手动改过的接入点弹回默认值。
#[test]
fn reselecting_the_same_provider_keeps_the_manually_chosen_endpoint() {
    let mut a = wizard_with_providers(vec![zhipu_provider()]);
    select_zhipu(&mut a); // provider_id=zhipu, endpoint_id=cn (默认), 焦点在 ApiKey

    // 手动把接入点改成国际版。
    focus_row(&mut a, ZH.wiz_f_endpoint);
    let open_endpoint = a.handle_key(key(KeyCode::Enter)).expect("Endpoint 行 ⏎ 应该产出 Action::OpenPicker");
    a.update(open_endpoint);
    a.update(Action::PickerDone { tag: PickerTag::WizardEndpoint, choice: PickerChoice::Item("intl".into()) });
    let out = render(&mut a, 80, 24);
    assert!(out.contains("国际版"), "改接入点应该生效\n{out}");

    // 回到厂商行, 重新确认同一个厂商 (智谱)。
    focus_row(&mut a, ZH.wiz_f_provider);
    let open_provider = a.handle_key(key(KeyCode::Enter)).expect("Provider 行 ⏎ 应该产出 Action::OpenPicker");
    a.update(open_provider);
    a.update(Action::PickerDone { tag: PickerTag::WizardProvider, choice: PickerChoice::Item("zhipu".into()) });
    let out2 = render(&mut a, 80, 24);
    assert!(out2.contains("国际版"), "重选同一个厂商不该把手动改过的接入点弹回默认值\n{out2}");
}

/// 校验失败挂上的字段错误, 在那个字段被编辑/重新选定之后应该消失。
#[test]
fn editing_a_field_clears_only_its_own_error() {
    let mut a = wizard_with_providers(vec![zhipu_provider()]);
    // 什么都不填直接提交: 第一个不合法的字段 (厂商) 报错, 焦点也被 `submit()` 拨回那个字段。
    focus_row(&mut a, ZH.wiz_btn_next);
    assert!(a.handle_key(key(KeyCode::Enter)).is_none(), "校验失败不该产出 Action");
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.wiz_err_provider), "应该先报厂商未选\n{out}");

    // 选厂商修好这一项 (`submit()` 已经把焦点拨回了 Provider 行, `select_zhipu` 的前提成立)。
    assert_focus(&mut a, ZH.wiz_f_provider);
    select_zhipu(&mut a);
    let out2 = render(&mut a, 80, 24);
    assert!(!out2.contains(ZH.wiz_err_provider), "选了厂商之后, 厂商自己的错误应该消失\n{out2}");
}

/// 确认弹窗叠在表单上时, 终端光标不该还留在被压暗的输入框里——`Terminal::draw` 按
/// `Frame::set_cursor_position` 有没有在这一帧被调过来决定显示/隐藏光标, 向导在有弹窗时应该
/// 完全不调它。
#[test]
fn wizard_cursor_is_hidden_while_a_popup_is_on_top() {
    let mut a = wizard_with_providers(vec![zhipu_provider()]);
    select_zhipu(&mut a);
    type_str(&mut a, "sk");
    let confirm_action = a.handle_key(key(KeyCode::Esc)).expect("有输入时 Esc 应该弹确认");
    a.update(confirm_action);

    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|f| a.draw(f, Duration::ZERO)).unwrap();
    assert!(!terminal.backend().cursor_visible(), "确认弹窗叠在表单上时不该显示终端光标");
}

/// 光标现在由字段自己算 (`FormFields::cursor_for`), 不再靠调用方在每个文本行的 `Cell` 字面量里
/// 手传——漏传编译期就会失败 (穷尽 `match`), 但光标到底有没有画对了只能靠真实终端断言 (快照文本
/// 不含终端光标)。三张表单各验一次: Basics/Custom 聚焦文本行时光标该跟着打字前进 (同一行内
/// 移动, x 右移打字个数对应的显示宽度); Slots 全是选择/按钮行, `cursor_for` 恒 `None`, 聚焦时
/// 不该出现任何终端光标。
#[test]
fn the_terminal_cursor_tracks_the_focused_text_row_on_every_form() {
    fn cursor_after(a: &mut App, typed: &str) -> ratatui::layout::Position {
        type_str(a, typed);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| a.draw(f, Duration::ZERO)).unwrap();
        assert!(terminal.backend().cursor_visible(), "聚焦文本行时终端光标应该可见");
        terminal.backend().cursor_position()
    }

    // Basics: 选完厂商焦点自动落在 ApiKey 行。
    let mut basics = wizard_with_providers(vec![zhipu_provider()]);
    select_zhipu(&mut basics);
    let p1 = cursor_after(&mut basics, "s");
    let p2 = cursor_after(&mut basics, "k");
    assert_eq!(p2.y, p1.y, "同一行继续打字, y 不该变");
    assert_eq!(p2.x, p1.x + 1, "ASCII 字符应该让光标右移一格");

    // Custom: 进来就聚焦在 ProviderName 行, 不用 focus_row。
    let mut custom = wizard_custom(CustomProtocol::Anthropic);
    let p1 = cursor_after(&mut custom, "中");
    let p2 = cursor_after(&mut custom, "转");
    assert_eq!(p2.y, p1.y, "同一行继续打字, y 不该变");
    assert_eq!(p2.x, p1.x + 2, "CJK 字符应该让光标右移两格 (显示宽度)");

    // Slots: 全是选择行 + 按钮行, 没有任何文本行, 聚焦时不该有终端光标。
    let mut slots = wizard_at_slots(vec![ModelInfo { id: "glm-4.6".into(), display_name: None }]);
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|f| slots.draw(f, Duration::ZERO)).unwrap();
    assert!(!terminal.backend().cursor_visible(), "Slots 表单没有文本行, 不该出现终端光标");
}

/// `Tab`/`BackTab` 在所有行类型上都分别等同 `↓`/`↑`, 不只在文本行才认。
#[test]
fn tab_and_backtab_move_focus_on_every_row_type() {
    let mut a = wizard_with_providers(vec![zhipu_provider()]);
    // 初始焦点在 Provider (选择行)。Tab 应该移到 Endpoint——用「厂商还没选, Endpoint 行 ⏎ 该被
    // 拒绝」这个只在焦点真的到了 Endpoint 才会触发的行为间接验证 (选择行本身不接受直接打字,
    // Tab 在选择行上也应该像 ↓ 一样移动焦点)。
    a.handle_key(key(KeyCode::Tab));
    assert_eq!(
        a.handle_key(key(KeyCode::Enter)),
        Some(Action::Notify { kind: ToastKind::Info, text: ZH.wiz_pick_provider_first.to_string() }),
        "Tab 应该已经把焦点移到 Endpoint 行"
    );

    // BackTab 应该把焦点移回 Provider。
    let backtab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE);
    a.handle_key(backtab);
    let open_action = a.handle_key(key(KeyCode::Enter)).expect("回到 Provider 行, ⏎ 应该开厂商 picker");
    match open_action {
        Action::OpenPicker(spec) => assert_eq!(spec.tag, PickerTag::WizardProvider, "应该是厂商 picker"),
        other => panic!("BackTab 应该已经把焦点移回 Provider 行, 实际 {other:?}"),
    }
}

/// 创建请求在飞时 `Esc` 应该被吞掉 (不弹确认、不关闭), 底栏也不该显示
/// `Esc 取消`——继续显示是纯误导, 这时候按了也没反应。
#[test]
fn creating_swallows_escape_and_hides_the_cancel_hint() {
    let mut a = wizard_with_providers(vec![zhipu_provider()]);
    select_zhipu(&mut a);
    type_str(&mut a, "sk-test");
    focus_row(&mut a, ZH.wiz_btn_next);
    let submit_action = a.handle_key(key(KeyCode::Enter)).expect("提交应该产出 Action");
    a.update(submit_action); // 创建在飞

    assert_eq!(a.handle_key(key(KeyCode::Esc)), None, "在飞时 Esc 应该被吞掉");
    let out = render(&mut a, 80, 24);
    assert!(!out.contains("Esc 取消"), "在飞时底栏不该显示 Esc 提示\n{out}");
}

// ---------- 新建订阅向导: 第二步 (绑定模型) ----------

/// `Created(Ok(id))` 落地应该紧接着发一次 `LoadModels { id }`, 并进入等模型列表的阶段 (见
/// `wizard/mod.rs::BasicsPhase::LoadingModels`)。
#[test]
fn a_successful_create_asks_for_the_model_list() {
    let mut a = wizard_with_providers(vec![zhipu_provider()]);
    let submit_action = submit_basics(&mut a);
    a.update(submit_action);

    let cmds = a.update(wizard_done(&a, WizardResult::Created(Ok(CreatedSubscription { id: "sub-1".into() }))));
    assert_eq!(cmds, vec![wizard_cmd(&a, Box::new(WizardCmd::LoadModels { id: "sub-1".into() }))]);

    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.wiz_loading_models), "拿到 id 之后文案应该换成「正在获取模型列表…」\n{out}");
}

/// `Created(Err)` 回第一步后表单必须**真的能再操作**——不是仅仅「看起来」回去了 (说明行 + 字段
/// 值都在), 而是按钮真的能再按一次、再发一次 `Create`; 否则表单会一直停在创建中, 只能 `Ctrl+C`
/// 强退整个 TUI。
#[test]
fn a_failed_create_returns_to_an_operable_basics_form() {
    let mut a = wizard_with_providers(vec![zhipu_provider()]);
    let submit_action = submit_basics(&mut a);
    a.update(submit_action); // 创建在飞

    a.update(wizard_done(&a, WizardResult::Created(Err("上游炸了".into()))));
    let out = render(&mut a, 80, 24);
    assert!(out.contains(&(ZH.wiz_create_failed)("上游炸了")), "{out}");

    // `submit()` 触发请求前没有移动过焦点, 此刻焦点应该还在 Submit——真的能再按一次「下一步」,
    // 不是被锁死的只读表单。
    let retry = a.handle_key(key(KeyCode::Enter));
    assert!(matches!(retry, Some(Action::WizardRequest(_))), "回到 Basics 之后应该能再次提交, 实际 {retry:?}");
}

/// 等模型列表 (只读请求, 不落库) 时底栏应该显示 `Esc 取消`; 创建 / 保存 (会落库的请求) 在飞时
/// 不显示——三个阶段一次测全。
#[test]
fn loading_models_shows_the_cancel_hint_but_creating_and_saving_do_not() {
    let mut a = wizard_with_providers(vec![zhipu_provider()]);
    let submit_action = submit_basics(&mut a);
    a.update(submit_action); // 创建在飞
    let out_creating = render(&mut a, 80, 24);
    assert!(!out_creating.contains("Esc 取消"), "Creating 不该显示 Esc 取消\n{out_creating}");

    a.update(wizard_done(&a, WizardResult::Created(Ok(CreatedSubscription { id: "sub-1".into() })))); // 等模型列表
    let out_loading_models = render(&mut a, 80, 24);
    assert!(out_loading_models.contains("Esc 取消"), "LoadingModels 应该显示 Esc 取消\n{out_loading_models}");

    a.update(wizard_done(&a, WizardResult::Models {
        id: "sub-1".into(),
        result: Ok(RefreshModelsResult::Auto { models: vec![ModelInfo { id: "glm-4.6".into(), display_name: None }], fetched_at: 0 }),
    })); // 第二步
    focus_row(&mut a, ZH.wiz_btn_save);
    let save_action = a.handle_key(key(KeyCode::Enter)).expect("保存应该产出 Action");
    a.update(save_action); // 保存在飞
    let out_saving = render(&mut a, 80, 24);
    assert!(!out_saving.contains("Esc 取消"), "Saving 不该显示 Esc 取消\n{out_saving}");
}

/// `Models(Ok(Auto { models }))` 进第二步, 候选非空时四个核心槽都
/// 预填 `models[0].id`, 兜底槽留空。
#[test]
fn auto_discovered_models_prefill_every_core_slot() {
    let mut a = wizard_at_slots(vec![
        ModelInfo { id: "glm-4.6".into(), display_name: Some("GLM 4.6 主力".into()) },
        ModelInfo { id: "glm-4.5-air".into(), display_name: None },
    ]);
    let out = render(&mut a, 80, 24);
    assert_eq!(out.matches("glm-4.6").count(), 4, "四个核心槽都应该预填第一项候选\n{out}");
    assert!(out.contains(ZH.sub_slot_unset), "兜底槽应该显示未配置占位\n{out}");

    // 直接跳到 Save 提交应该通过 (四个核心槽都已经填好), 证明预填是真的写进了草稿, 不只是画面
    // 巧合显示了 "glm-4.6" 这几个字。
    focus_row(&mut a, ZH.wiz_btn_save);
    assert!(a.handle_key(key(KeyCode::Enter)).is_some(), "槽位已经填好, 提交应该通过校验");
}

/// `Models(Ok(Auto { models: vec![] }))` (理论上不该发生, 防御性覆盖 `if let Some(first) =
/// models.first()` 那个保护): 没有候选可预填时四个核心槽应该留空、不 panic, 照样进第二步。
#[test]
fn an_empty_auto_list_leaves_the_core_slots_blank() {
    let mut a = wizard_at_slots(vec![]);
    focus_row(&mut a, ZH.wiz_btn_save);
    assert!(a.handle_key(key(KeyCode::Enter)).is_none(), "空候选时槽位应该留空, 校验应该失败");
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.wiz_err_slot), "{out}");
}

/// `Models(Ok(ManualFallback { reason }))` 进第二步, 候选为空、
/// 槽位留空、说明行是 `wiz_models_manual(reason)`。
#[test]
fn a_manual_fallback_leaves_the_slots_empty_and_explains_why() {
    let mut a = wizard_at_slots_with_manual_fallback("上游不支持自动发现");
    let out = render(&mut a, 80, 24);
    assert!(out.contains(&(ZH.wiz_models_manual)("上游不支持自动发现")), "应该显示自动获取失败的原因\n{out}");

    // 槽位留空: 直接跳到 Save 提交应该被 `validate_slots` 拦住, 证明四个核心槽确实是空的
    // (不是巧合没显示出候选文本)。
    focus_row(&mut a, ZH.wiz_btn_save);
    assert!(a.handle_key(key(KeyCode::Enter)).is_none(), "槽位空着, 校验应该失败");
    let out2 = render(&mut a, 80, 24);
    assert!(out2.contains(ZH.wiz_err_slot), "{out2}");
}

/// `Models(Err(e))` 与 `ManualFallback` 走同一条路: 请求本身失败时同样进第二步、候选为空、说明行
/// 复用 `wiz_models_manual`。
#[test]
fn a_failed_model_list_request_behaves_like_manual_fallback() {
    let mut a = wizard_after_create();
    a.update(wizard_done(&a, WizardResult::Models { id: "sub-1".into(), result: Err("网络错误".into()) }));
    let out = render(&mut a, 80, 24);
    assert!(out.contains(&(ZH.wiz_models_manual)("网络错误")), "{out}");
}

/// `Save` 校验通过后发出的 `SaveSlots` 只带 `model_slots`——四个核心槽是真实
/// 选的值, 兜底槽是空串, **没有 `slot_efforts` 这个概念** (`WizardCmd::SaveSlots` 这个变体本身
/// 就没有那个字段, 这条断言顺带用类型形状锁住)。
#[test]
fn saving_sends_only_the_model_slots_patch() {
    let mut a = wizard_at_slots(vec![ModelInfo { id: "glm-4.6".into(), display_name: None }]);
    focus_row(&mut a, ZH.wiz_btn_save);
    let save_action = a.handle_key(key(KeyCode::Enter)).expect("填好后保存应该产出 Action");
    let cmds = a.update(save_action);
    assert_eq!(
        cmds,
        vec![wizard_cmd(&a, Box::new(WizardCmd::SaveSlots {
            id: "sub-1".into(),
            model_slots: ModelSlots {
                fable: "glm-4.6".into(),
                opus: "glm-4.6".into(),
                sonnet: "glm-4.6".into(),
                haiku: "glm-4.6".into(),
                fallback: String::new(),
                jev: String::new(),
            },
        }))]
    );
}

/// 保存发出后表单必须真的只读, 不能在 `SaveSlots` 还在飞的时候再发一个 (后端会收到两个重复的
/// 保存请求)。
#[test]
fn saving_is_read_only_until_the_result_comes_back() {
    let mut a = wizard_at_slots(vec![ModelInfo { id: "glm-4.6".into(), display_name: None }]);
    focus_row(&mut a, ZH.wiz_btn_save);
    let save_action = a.handle_key(key(KeyCode::Enter)).expect("保存应该产出 Action");
    a.update(save_action); // 保存在飞

    assert_eq!(a.handle_key(key(KeyCode::Enter)), None, "保存在飞时再按 ⏎ 不该发第二个请求");
    assert_eq!(a.handle_key(key(KeyCode::Esc)), None, "保存在飞时 Esc 也该被吞掉");
}

/// `SlotsSaved(Ok(()))` 应该关掉向导 (补一次重拉订阅列表) 并弹一条
/// `wiz_created(display_name)` 的 Success toast。
#[test]
fn a_successful_save_closes_the_wizard_with_a_toast() {
    let mut a = wizard_at_slots(vec![ModelInfo { id: "glm-4.6".into(), display_name: None }]);
    focus_row(&mut a, ZH.wiz_btn_save);
    let save_action = a.handle_key(key(KeyCode::Enter)).expect("保存应该产出 Action");
    a.update(save_action); // 保存在飞

    let cmds = a.update(wizard_done(&a, WizardResult::SlotsSaved(Ok(()))));
    assert_eq!(cmds, vec![Cmd::Fetch(Fetch::Subscriptions)], "关掉向导应该补一次重拉订阅列表");

    let out = render(&mut a, 80, 24);
    assert!(out.contains(&(ZH.wiz_created)("智谱 AI")), "应该出现「已创建」的成功 toast\n{out}");
}

/// `SlotsSaved(Err(e))` 应该回到可编辑的第二步并把 `wiz_save_failed(e)` 挂成说明行——不丢用户
/// 已经选好的槽位值 (草稿原样保留, 只是多了一条说明)。光看说明行和槽位文字还不够 (保存中与
/// 可编辑时画出来这两样一样), 必须再断言表单**真的能操作** (按钮能再按一次、真的发出第二个
/// `SaveSlots`)。
#[test]
fn a_failed_save_returns_to_slots_and_explains_why() {
    let mut a = wizard_at_slots(vec![ModelInfo { id: "glm-4.6".into(), display_name: None }]);
    focus_row(&mut a, ZH.wiz_btn_save);
    let save_action = a.handle_key(key(KeyCode::Enter)).expect("保存应该产出 Action");
    a.update(save_action);

    a.update(wizard_done(&a, WizardResult::SlotsSaved(Err("磁盘写满了".into()))));
    let out = render(&mut a, 80, 24);
    assert!(out.contains(&(ZH.wiz_save_failed)("磁盘写满了")), "{out}");
    assert_eq!(out.matches("glm-4.6").count(), 4, "保存失败不该丢掉已经选好的槽位值\n{out}");

    // 焦点还在 Save (提交时没有移动过它), 真的能再按一次 ⏎ 发出第二个 SaveSlots。
    let retry = a.handle_key(key(KeyCode::Enter));
    assert!(matches!(retry, Some(Action::WizardRequest(_))), "回到 Slots 之后应该能再次保存, 实际 {retry:?}");
}

/// 第二步 `Esc` 弹确认, 文案是 `wiz_confirm_exit_pending` (不是第一步用的 `confirm_discard`——
/// 订阅已经建好了, 退出会留下 (pending) 槽位)。
#[test]
fn escaping_after_the_subscription_was_created_warns_about_pending() {
    let mut a = wizard_at_slots_with_manual_fallback("x");
    assert_eq!(
        a.handle_key(key(KeyCode::Esc)),
        Some(Action::OpenConfirm { prompt: ZH.wiz_confirm_exit_pending.to_string(), on_yes: OnYes::discard_then(Action::CloseWizard) }),
        "Slots 阶段的 Esc 确认文案应该是「订阅已经创建…」, 不是通用的放弃编辑提示"
    );
}

/// 槽位 picker 的 `allow_custom` 必须是 `true`——`example_models` 为空的厂商遇上
/// `ManualFallback` 时, 如果不能手输, 四个核心槽永远填不上, 保存永远被校验拦住, 用户只能退出
/// 留下一条 `(pending)`。同时锁住候选来源规则: 有真实候选 (拉到的模型列表) 时用它
/// (label = id, hint = display_name); 候选为空时退回当前厂商的 `example_models` (只有 id,
/// 没有 hint)。
#[test]
fn the_slot_picker_prefers_real_candidates_and_falls_back_to_example_models() {
    // 有真实候选 (Models(Ok(Auto)) 拉到的两个模型)。
    let mut a = wizard_at_slots(vec![
        ModelInfo { id: "glm-4.6".into(), display_name: Some("GLM 4.6 主力".into()) },
        ModelInfo { id: "glm-4.5-air".into(), display_name: None },
    ]);
    let action = a.handle_key(key(KeyCode::Enter)).expect("Fable 行 ⏎ 应该打开 picker");
    let Action::OpenPicker(spec) = action else { panic!("应该是 OpenPicker, 实际 {action:?}") };
    assert!(spec.allow_custom, "槽位 picker 必须允许手输");
    assert_eq!(
        spec.items,
        vec![
            PickerItem { id: "glm-4.6".into(), label: "glm-4.6".into(), hint: Some("GLM 4.6 主力".into()) },
            PickerItem { id: "glm-4.5-air".into(), label: "glm-4.5-air".into(), hint: None },
        ],
        "有真实候选时应该用 models, label 是 id, hint 是 display_name"
    );

    // 没有真实候选 (ManualFallback, models 空) 时退回厂商的 example_models——`zhipu_provider()`
    // 固定给了两个 (`glm-4-plus` / `glm-4.5-flash`)。
    let mut b = wizard_at_slots_with_manual_fallback("上游不支持自动发现");
    let action = b.handle_key(key(KeyCode::Enter)).expect("Fable 行 ⏎ 应该打开 picker");
    let Action::OpenPicker(spec) = action else { panic!("应该是 OpenPicker, 实际 {action:?}") };
    assert!(spec.allow_custom, "候选为空时更要允许手输, 否则永远填不上");
    assert_eq!(
        spec.items,
        vec![
            PickerItem { id: "glm-4-plus".into(), label: "glm-4-plus".into(), hint: None },
            PickerItem { id: "glm-4.5-flash".into(), label: "glm-4.5-flash".into(), hint: None },
        ],
        "候选为空时应该退回厂商的 example_models"
    );
}

/// 去掉兜底槽的「清空」项的话, 兜底槽一旦选过就改不回未配置——锁住它必须是第一项。
#[test]
fn the_fallback_slot_picker_offers_a_clear_item_first() {
    let mut a = wizard_at_slots(vec![ModelInfo { id: "glm-4.6".into(), display_name: None }]);
    focus_row(&mut a, slot_row(Slot::Fallback));
    let action = a.handle_key(key(KeyCode::Enter)).expect("Fallback 行 ⏎ 应该打开 picker");
    let Action::OpenPicker(spec) = action else { panic!("应该是 OpenPicker, 实际 {action:?}") };
    assert_eq!(
        spec.items.first(),
        Some(&PickerItem { id: String::new(), label: ZH.pick_clear_fallback.to_string(), hint: None }),
        "兜底槽 picker 的第一项应该是「清空」\n{:?}",
        spec.items
    );
}

/// `PickerDone { WizardSlot, .. }` 真的经过 `App` 往下走: 自定义输入 (`PickerChoice::Custom`)
/// 应该写进对应槽位的草稿。
#[test]
fn picking_a_slot_model_through_the_app_writes_it_into_the_draft() {
    let mut a = wizard_at_slots(vec![ModelInfo { id: "glm-4.6".into(), display_name: None }]);
    focus_row(&mut a, slot_row(Slot::Opus));
    let open_action = a.handle_key(key(KeyCode::Enter)).expect("Opus 行 ⏎ 应该打开 picker");
    a.update(open_action);
    a.update(Action::PickerDone { tag: PickerTag::WizardSlot { slot: Slot::Opus }, choice: PickerChoice::Custom("custom-model".into()) });

    let out = render(&mut a, 80, 24);
    assert!(out.contains("custom-model"), "自定义输入应该写进 Opus 槽位\n{out}");
}

/// 同上, 兜底槽的「清空」项 (`PickerChoice::Item("")`) 应该写回空串, 让画面重新显示未配置占位。
#[test]
fn picking_clear_on_the_fallback_slot_writes_an_empty_string() {
    let mut a = wizard_at_slots(vec![ModelInfo { id: "glm-4.6".into(), display_name: None }]);
    focus_row(&mut a, slot_row(Slot::Fallback));
    let open_action = a.handle_key(key(KeyCode::Enter)).expect("Fallback 行 ⏎ 应该打开 picker");
    a.update(open_action);
    a.update(Action::PickerDone { tag: PickerTag::WizardSlot { slot: Slot::Fallback }, choice: PickerChoice::Item("glm-4.6".into()) });
    let out = render(&mut a, 80, 24);
    assert!(!out.contains(ZH.sub_slot_unset), "兜底槽选过模型后不该再显示未配置\n{out}");

    let open_action2 = a.handle_key(key(KeyCode::Enter)).expect("Fallback 行 ⏎ 应该再打开一次 picker");
    a.update(open_action2);
    a.update(Action::PickerDone { tag: PickerTag::WizardSlot { slot: Slot::Fallback }, choice: PickerChoice::Item(String::new()) });
    let out2 = render(&mut a, 80, 24);
    assert!(out2.contains(ZH.sub_slot_unset), "选「清空」应该把兜底槽写回空串, 重新显示未配置\n{out2}");
}

/// 80×24: 已创建、自动发现回来一个候选 (`glm-4.6`), 四个核心槽预填、兜底槽显示未配置,
/// 聚焦停在 `Fable` 行 (`Models(Ok)` 落地后的默认聚焦)。
#[test]
fn wizard_slots_80x24() {
    let mut a = wizard_at_slots(vec![ModelInfo { id: "glm-4.6".into(), display_name: None }]);
    insta::assert_snapshot!(render(&mut a, 80, 24));
}

// ---------- 新建订阅向导: 自定义厂商单页 ----------

/// 80×24: Anthropic 兼容协议, 厂商名/Base URL/API Key/备注名都已填好, 探测成功后
/// 手动给四个核心槽选了模型 (探测成功**不**自动预填, 与桌面端一致), 兜底槽留空。
#[test]
fn wizard_custom_80x24() {
    insta::assert_snapshot!(render(&mut custom_wizard_probed_and_filled(), 80, 24));
}

/// `wizard_custom_80x24` 的画面 (各语言的快照共用同一条驱动路径)。
fn custom_wizard_probed_and_filled() -> App {
    let mut a = wizard_custom(CustomProtocol::Anthropic);
    // 备注名跟着厂商名自动生成, 不用再手填一遍——打厂商名的同时备注名就已经是
    // "我的中转" 了。
    // 请求路径 / 鉴权保留 Anthropic 预设, 备注名已经自动跟随厂商名, 都不用碰。
    fill_custom(
        &mut a,
        CustomFill { provider_name: Some("我的中转"), base_url: Some("https://api.example.com"), api_key: Some("abcdef"), ..Default::default() },
    );
    focus_row(&mut a, s().wiz_btn_probe);
    let probe_action = a.handle_key(key(KeyCode::Enter)).expect("Probe 应该产出 Action");
    a.update(probe_action); // 探测在飞

    a.update(wizard_done(&a, WizardResult::Probed {
        base_url: "https://api.example.com".into(),
        result: Ok(ProbeModelsResult::Auto {
            models: vec![
                ModelInfo { id: "claude-sonnet-4".into(), display_name: None },
                ModelInfo { id: "claude-haiku-4".into(), display_name: None },
            ],
            models_url: "https://api.example.com/v1/models".into(),
        }),
    }));

    // 手动给四个核心槽选值 (探测成功不自动预填)。
    pick_core_slots(&mut a, |slot| PickerChoice::Item(if slot == Slot::Haiku { "claude-haiku-4" } else { "claude-sonnet-4" }.into()));
    // 兜底槽留空 (焦点停在它上面, 不操作它)。
    focus_row(&mut a, slot_row(Slot::Fallback));
    a
}

/// 同上, 120×40: 自定义表单是三张表单里最长的一份, 宽终端下确认它不会因为值列
/// (`Constraint::Min(0)` 吃剩余宽度) 变宽而错位。
#[test]
fn wizard_custom_120x40() {
    let mut a = wizard_custom(CustomProtocol::Anthropic);
    fill_custom(
        &mut a,
        CustomFill { provider_name: Some("我的中转"), base_url: Some("https://api.example.com"), api_key: Some("abcdef"), ..Default::default() },
    );
    focus_row(&mut a, ZH.wiz_btn_probe);
    let probe_action = a.handle_key(key(KeyCode::Enter)).expect("Probe 应该产出 Action");
    a.update(probe_action);

    a.update(wizard_done(&a, WizardResult::Probed {
        base_url: "https://api.example.com".into(),
        result: Ok(ProbeModelsResult::Auto {
            models: vec![
                ModelInfo { id: "claude-sonnet-4".into(), display_name: None },
                ModelInfo { id: "claude-haiku-4".into(), display_name: None },
            ],
            models_url: "https://api.example.com/v1/models".into(),
        }),
    }));

    pick_core_slots(&mut a, |slot| PickerChoice::Item(if slot == Slot::Haiku { "claude-haiku-4" } else { "claude-sonnet-4" }.into()));
    focus_row(&mut a, slot_row(Slot::Fallback));

    insta::assert_snapshot!(render(&mut a, 120, 40));
}

/// 80×24 下自定义表单内容区实际可用高度是 **16 行** (24 − 3 标签栏 − 1 底栏 − 2 边框 − 2 内距)。
/// 这里用一条**短**说明 (只占 1 行) 摆到"14 行
/// 固定内容 + 1 行说明 + 1 行 Spacer = 16 行"的边界——恰好等于可用高度、**零余量**, 不该触发
/// 滚动或出现 `form_more`。真正会溢出的长说明场景 (`form.rs` 的
/// `scrolling_keeps_a_focused_button_visible_behind_a_long_wrapped_note`) 单独覆盖。
#[test]
fn the_custom_form_fits_the_minimum_terminal() {
    let mut a = wizard_custom(CustomProtocol::Gemini);
    // Base URL 必须**替换**掉 Gemini 的非空预设: 在预设后面追加的话, 草稿成了两段 URL 拼接,
    // 下面那份 `Probed` 会被身份守卫当成别人的结果丢弃, 说明行根本挂不上, 这条测试就只在量「14 行」。
    // 请求路径保留 Gemini 预设 (已含 {model})。
    fill_custom(&mut a, relay_fill());
    focus_row(&mut a, ZH.wiz_btn_probe);
    let probe_action = a.handle_key(key(KeyCode::Enter)).expect("Probe 应该产出 Action");
    a.update(probe_action); // 探测在飞
    // 探测失败, 挂上一条短说明 (只占 1 行: 14+1+1=16, 恰好等于可用高度)。
    a.update(wizard_done(&a, WizardResult::Probed {
        base_url: "https://relay.example.com".into(),
        result: Ok(ProbeModelsResult::ManualFallback { reason: "上游不支持自动发现".into() }),
    }));

    let out = render(&mut a, 80, 24);
    assert!(out.contains(&(ZH.wiz_models_manual)("上游不支持自动发现")), "说明行应该真的挂上了\n{out}");
    assert!(!out.contains(ZH.form_more), "14 行内容 + 1 行说明 + 1 行空行 = 16 行, 应该恰好放得下 (零余量), 不该出现滚动提示\n{out}");
}

/// 锁定协议 (Gemini) 下从 `MessagesPath` 按 `↓` 应该直接跳到 `ApiKey`, 跳过 `Auth` 行——用
/// "此刻 Ctrl+R 生效" 间接验证焦点真的落在了 `ApiKey`, 而不是路过 `Auth`(那一行不接受
/// `Ctrl+R`/打字, 如果焦点被卡在那里, 后面的输入会凭空消失)。
#[test]
fn a_locked_protocol_skips_the_auth_row() {
    let mut a = wizard_custom(CustomProtocol::Gemini);
    focus_row(&mut a, ZH.wiz_f_messages_path);
    a.handle_key(key(KeyCode::Down)); // 这一步就是被测行为: 跳过锁定的 Auth, 不能换成 focus_row
    assert_focus(&mut a, ZH.wiz_f_api_key);

    let ctrl_r = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
    a.handle_key(ctrl_r);
    type_str(&mut a, "sk");
    let out = render(&mut a, 80, 24);
    assert!(out.contains("sk"), "Ctrl+R 应该已经在 ApiKey 行生效, 说明焦点跳过了锁定的 Auth 行\n{out}");
}

/// 重选同一个协议什么都不重算 (与厂商行重选同一个厂商同理)——用户手动
/// 把 Base URL / 请求路径 / 鉴权都改成中转站真实值之后, 回到协议行确认同一个协议 (picker 默认
/// 高亮当前项, 很容易无意中再按一次 ⏎), 这些手填的值不该被悄悄弹回协议预设。
#[test]
fn reselecting_the_same_protocol_does_not_reset_the_edited_fields() {
    let mut a = wizard_custom(CustomProtocol::Anthropic);
    fill_custom(&mut a, CustomFill { base_url: Some("https://relay.example.com"), messages_path: Some("/api/v1/messages"), ..Default::default() });
    focus_row(&mut a, ZH.wiz_f_auth);
    let open_auth = a.handle_key(key(KeyCode::Enter)).expect("Auth 行 ⏎ 应该产出 Action::OpenPicker");
    a.update(open_auth);
    a.update(Action::PickerDone { tag: PickerTag::WizardAuth, choice: PickerChoice::Item("x-api-key".into()) });

    // 回到协议行, 重新确认同一个协议 (Anthropic)。
    focus_row(&mut a, ZH.wiz_f_protocol);
    let open_protocol = a.handle_key(key(KeyCode::Enter)).expect("Protocol 行 ⏎ 应该产出 Action::OpenPicker");
    a.update(open_protocol);
    a.update(Action::PickerDone { tag: PickerTag::WizardProtocol, choice: PickerChoice::Item(CustomProtocol::Anthropic.as_wire().into()) });

    let out = render(&mut a, 80, 24);
    assert!(out.contains("https://relay.example.com"), "重选同一个协议不该清空手填的 Base URL\n{out}");
    assert!(out.contains("/api/v1/messages"), "重选同一个协议不该清空手填的请求路径\n{out}");
    assert!(out.contains("x-api-key"), "重选同一个协议不该把手选的鉴权头弹回默认值\n{out}");
}

/// 备注名跟着厂商名自动生成, 与内置路径 (`select_zhipu` 选厂商时)
/// 同一条规则——厂商名 trim 后非空、备注名还没被手改过时, 打厂商名的同时备注名就该跟着变。
#[test]
fn display_name_follows_the_provider_name_while_still_auto() {
    let mut a = wizard_custom(CustomProtocol::Anthropic);
    type_str(&mut a, "MyRelay");
    let out = render(&mut a, 80, 24);
    assert_eq!(out.matches("MyRelay").count(), 2, "厂商名与备注名此刻应该是同一个值, 各出现一次\n{out}");
}

/// 用户手改过备注名之后不再跟随厂商名的后续编辑。
#[test]
fn display_name_stops_following_after_a_manual_edit() {
    let mut a = wizard_custom(CustomProtocol::Anthropic);
    type_str(&mut a, "MyRelay"); // ProviderName, 备注名跟着自动变成 "MyRelay"
    focus_row(&mut a, ZH.wiz_f_display_name);
    for _ in 0.."MyRelay".chars().count() {
        a.handle_key(key(KeyCode::Backspace));
    }
    type_str(&mut a, "Other"); // 手动改成别的值

    focus_row(&mut a, ZH.wiz_f_provider_name);
    type_str(&mut a, "X"); // 继续编辑厂商名

    let out = render(&mut a, 80, 24);
    assert!(out.contains("MyRelayX"), "厂商名应该正常继续编辑\n{out}");
    assert!(out.contains("Other"), "手改过的备注名应该保留\n{out}");
    assert_eq!(out.matches("MyRelayX").count(), 1, "手改过的备注名不该被厂商名的后续编辑覆盖\n{out}");
}

/// 厂商名与已有订阅重名时, 自动生成的备注名应该加序号 (`default_display_name` 本来就有
/// 这条规则, 这里验证自定义路径真的接上了它)。
#[test]
fn display_name_auto_fill_adds_a_number_on_collision_with_an_existing_subscription() {
    let mut a = wizard_custom(CustomProtocol::Anthropic);
    a.update(subs_done(1, vec![sub("1", "MyRelay", SubscriptionState::Healthy)]));
    type_str(&mut a, "MyRelay");
    let out = render(&mut a, 80, 24);
    assert!(out.contains("MyRelay 2"), "厂商名与已有订阅重名时, 备注名应该自动加序号\n{out}");
}

/// `Probe` 按钮应该发出 trim 过的 `base_url`, 其余字段原样带上。
#[test]
fn probing_sends_the_protocol_and_the_trimmed_base_url() {
    let mut a = wizard_custom(CustomProtocol::Anthropic);
    fill_custom(&mut a, CustomFill { base_url: Some("  https://relay.example.com  "), api_key: Some("sk-test"), ..Default::default() });
    focus_row(&mut a, ZH.wiz_btn_probe);

    let probe_action = a.handle_key(key(KeyCode::Enter)).expect("Probe 应该产出 Action");
    assert_eq!(
        probe_action,
        Action::WizardRequest(Box::new(WizardCmd::Probe(ProbeInput {
            base_url: "https://relay.example.com".into(),
            auth_header_name: "Authorization".into(),
            auth_header_format: AuthHeaderFormat::Bearer,
            api_key: Secret::new("sk-test"),
            protocol: CustomProtocol::Anthropic,
        })))
    );
}

/// 发起 `Probe` 时应该清掉上一次的失败说明 (与其它提交按钮同一条规则)——否则重试在飞期间, 屏幕上会同时显示"上一次探测失败的原因"和
/// "正在获取模型列表…"两条互相矛盾的文案。
#[test]
fn submitting_probe_clears_the_previous_failure_note() {
    let mut a = wizard_custom(CustomProtocol::Anthropic);
    fill_custom(&mut a, CustomFill { base_url: Some("https://relay.example.com"), api_key: Some("sk-test"), ..Default::default() });
    focus_row(&mut a, ZH.wiz_btn_probe);
    let probe1 = a.handle_key(key(KeyCode::Enter)).expect("Probe 应该产出 Action");
    a.update(probe1); // 探测在飞
    a.update(wizard_done(&a, WizardResult::Probed { base_url: "https://relay.example.com".into(), result: Err("网络错误".into()) }));
    let out = render(&mut a, 80, 24);
    assert!(out.contains(&(ZH.wiz_models_manual)("网络错误")), "先确认失败说明确实挂上了\n{out}");

    // `Probed` 落地后焦点被挪到 Slot(Fable) (`apply_wizard_result` 统一行为), 挪回 Probe 行重试。
    assert_focus(&mut a, slot_row(Slot::Fable));
    focus_row(&mut a, ZH.wiz_btn_probe);
    let probe2 = a.handle_key(key(KeyCode::Enter));
    assert!(matches!(probe2, Some(Action::WizardRequest(_))), "重试应该发出新的 Probe 请求, 实际 {probe2:?}");
    let out2 = render(&mut a, 80, 24);
    assert!(!out2.contains(&(ZH.wiz_models_manual)("网络错误")), "重新发起探测时应该清掉上一次的失败说明\n{out2}");
}

/// 发起 `Create` 时同样应该清掉上一次的失败说明, 理由同 `submitting_probe_...`。
#[test]
fn submitting_create_clears_the_previous_failure_note() {
    let mut a = wizard_custom(CustomProtocol::Anthropic);
    // 备注名跟着厂商名自动填, 不碰。
    fill_custom(&mut a, relay_fill());
    pick_core_slots(&mut a, |_| PickerChoice::Custom("glm-4.6".into()));
    focus_row(&mut a, ZH.wiz_btn_create);
    let submit1 = a.handle_key(key(KeyCode::Enter)).expect("创建应该产出 Action");
    a.update(submit1); // 创建在飞
    a.update(wizard_done(&a, WizardResult::Created(Err("上游炸了".into()))));
    let out = render(&mut a, 80, 24);
    assert!(out.contains(&(ZH.wiz_create_failed)("上游炸了")), "先确认失败说明确实挂上了\n{out}");

    // 焦点还在 Submit (创建失败落地不移动焦点), 重试。
    let submit2 = a.handle_key(key(KeyCode::Enter));
    assert!(matches!(submit2, Some(Action::WizardRequest(_))), "重试应该发出新的 Create 请求, 实际 {submit2:?}");
    let out2 = render(&mut a, 80, 24);
    assert!(!out2.contains(&(ZH.wiz_create_failed)("上游炸了")), "重新发起创建时应该清掉上一次的失败说明\n{out2}");
}

/// `Create` 的 `model_slots` 是真实值 (不是 pending), `source` 是 `Custom`; `Created(Ok)` 之后
/// 向导直接关闭, 不发 `SaveSlots`/`LoadModels`。
#[test]
fn creating_a_custom_subscription_sends_real_slots_and_closes() {
    let mut a = wizard_custom(CustomProtocol::Anthropic);
    // 请求路径 / 鉴权保留默认 (/v1/messages, Authorization/Bearer); 备注名已经跟着
    // 厂商名自动填成"中转站"; 跳过探测; 兜底槽留空。
    fill_custom(&mut a, relay_fill());
    pick_core_slots(&mut a, |_| PickerChoice::Custom("glm-4.6".into()));
    focus_row(&mut a, ZH.wiz_btn_create);

    let expected_input = CreateInput {
        display_name: "中转站".into(),
        api_key: Secret::new("sk-test"),
        model_slots: ModelSlots {
            fable: "glm-4.6".into(),
            opus: "glm-4.6".into(),
            sonnet: "glm-4.6".into(),
            haiku: "glm-4.6".into(),
            fallback: String::new(),
            jev: String::new(),
        },
        source: CreateSource::Custom(Box::new(CustomSource {
            provider_display_name: "中转站".into(),
            base_url: "https://relay.example.com".into(),
            messages_path: "/v1/messages".into(),
            auth_header_name: "Authorization".into(),
            auth_header_format: AuthHeaderFormat::Bearer,
            protocol: CustomProtocol::Anthropic,
            models_url: None,
        })),
    };
    let submit_action = a.handle_key(key(KeyCode::Enter)).expect("创建应该产出 Action");
    assert_eq!(
        submit_action,
        Action::WizardRequest(Box::new(WizardCmd::Create(expected_input.clone()))),
        "槽位应该是真实值 (不是 pending), source 应该是 Custom"
    );

    let cmds = a.update(submit_action);
    assert_eq!(cmds, vec![wizard_cmd(&a, Box::new(WizardCmd::Create(expected_input)))]);

    // Created(Ok) 之后向导应该直接关闭 (补一次重拉订阅列表), 不发 LoadModels/SaveSlots。
    let close_cmds = a.update(wizard_done(&a, WizardResult::Created(Ok(CreatedSubscription { id: "sub-9".into() }))));
    assert_eq!(
        close_cmds,
        vec![Cmd::Fetch(Fetch::Subscriptions)],
        "自定义路径 Created(Ok) 应该直接关向导, 不该像内置路径那样发 LoadModels"
    );
}

/// 自定义路径 `Created(Err)`: 回到可编辑的自定义单页 (不是内置路径的第一步), 说明行挂
/// `wiz_create_failed`, 表单真的能再操作 (与内置路径 `Created(Err)` 的同款要求)。
#[test]
fn a_failed_custom_create_returns_to_custom_and_stays_operable() {
    let mut a = wizard_custom(CustomProtocol::Anthropic);
    // 备注名已经跟着厂商名自动填成"中转站"。
    fill_custom(&mut a, relay_fill());
    pick_core_slots(&mut a, |_| PickerChoice::Custom("glm-4.6".into()));
    focus_row(&mut a, ZH.wiz_btn_create);
    let submit_action = a.handle_key(key(KeyCode::Enter)).expect("创建应该产出 Action");
    a.update(submit_action); // 创建在飞

    a.update(wizard_done(&a, WizardResult::Created(Err("上游炸了".into()))));
    let out = render(&mut a, 80, 24);
    assert!(out.contains(&(ZH.wiz_create_failed)("上游炸了")), "{out}");

    // 真的能再操作: 焦点还在 Submit (提交时没有移动焦点), 能再按一次 ⏎ 产出新请求。
    let retry = a.handle_key(key(KeyCode::Enter));
    assert!(matches!(retry, Some(Action::WizardRequest(_))), "回到 Custom 之后应该能再次提交, 实际 {retry:?}");
}

/// `Probed(Ok(Auto))` 应该记下 `ProbedModels`(base_url + models_url), 且**不**自动预填槽位；
/// 探测后不改 `base_url`、直接创建, `models_url` 应该被带上 (`CustomDraft::models_url()` 生效)。
#[test]
fn a_successful_probe_records_models_url_for_later_create() {
    let mut a = wizard_custom(CustomProtocol::Anthropic);
    // 备注名已经跟着厂商名自动填成"中转站"。
    fill_custom(&mut a, relay_fill());
    focus_row(&mut a, ZH.wiz_btn_probe);
    let probe_action = a.handle_key(key(KeyCode::Enter)).expect("Probe 应该产出 Action");
    a.update(probe_action); // 探测在飞

    a.update(wizard_done(&a, WizardResult::Probed {
        base_url: "https://relay.example.com".into(),
        result: Ok(ProbeModelsResult::Auto {
            models: vec![ModelInfo { id: "glm-4.6".into(), display_name: None }],
            models_url: "https://relay.example.com/v1/models".into(),
        }),
    }));
    let out = render(&mut a, 80, 24);
    assert!(!out.contains("glm-4.6"), "探测成功不该自动预填槽位, 应该留给用户手选\n{out}");

    pick_core_slots(&mut a, |_| PickerChoice::Item("glm-4.6".into()));
    focus_row(&mut a, ZH.wiz_btn_create);
    let submit_action = a.handle_key(key(KeyCode::Enter)).expect("创建应该产出 Action");
    let Action::WizardRequest(cmd) = submit_action else { panic!("应该是 WizardRequest") };
    let WizardCmd::Create(input) = *cmd else { panic!("应该是 Create") };
    let CreateSource::Custom(custom) = input.source else { panic!("应该是 Custom source") };
    assert_eq!(
        custom.models_url,
        Some("https://relay.example.com/v1/models".into()),
        "探测后 base_url 没再改过, 创建时应该带上 models_url"
    );
}

/// `Probed` 晚到 (没有探测在飞): 不该被采纳。
#[test]
fn a_stale_probed_result_is_discarded_outside_probing() {
    let mut a = wizard_custom(CustomProtocol::Anthropic);
    // 还没探测 (编辑中, 没有探测在飞), 喂一份晚到的 Probed 结果。
    a.update(wizard_done(&a, WizardResult::Probed {
        base_url: "https://late.example.com".into(),
        result: Ok(ProbeModelsResult::Auto {
            models: vec![ModelInfo { id: "late-model".into(), display_name: None }],
            models_url: "https://late.example.com/v1/models".into(),
        }),
    }));
    // 候选模型不会画进任何一行的显示文字 (槽位行显示的是**已选的值**, 不是候选列表), 所以不能靠
    // `render()` 的文字断言——必须打开槽位 picker, 检查候选里有没有混进这份晚到的模型。
    focus_row(&mut a, slot_row(Slot::Fable));
    let open_action = a.handle_key(key(KeyCode::Enter)).expect("Fable 行 ⏎ 应该产出 Action::OpenPicker");
    let Action::OpenPicker(spec) = open_action else { panic!("应该是 OpenPicker, 实际 {open_action:?}") };
    assert!(
        !spec.items.iter().any(|item| item.id == "late-model"),
        "阶段不是 Probing 时晚到的 Probed 结果不该被采纳, 候选不该混入\n{:?}",
        spec.items
    );
}

/// `Probed` 阶段守卫之外还要核对结果自带的 `base_url`——代次之外的纵深防御: 就算一份不属于这次
/// 探测的结果带着当前代次漏过来 (探测的是另一个 base_url), **阶段守卫拦不住 (此刻正在探测)**,
/// 也要靠 `base_url` 不一致丢弃——与 `a_stale_probed_result_is_discarded_outside_probing` (阶段不同)
/// 和 `a_probe_result_from_a_closed_wizard_is_dropped_even_for_the_same_base_url` (代次不同) 是三回事。
#[test]
fn a_probed_result_for_a_different_base_url_is_discarded() {
    let mut a = wizard_custom(CustomProtocol::Anthropic);
    fill_custom(
        &mut a,
        CustomFill { provider_name: Some("厂商"), base_url: Some("https://mine.example.com"), api_key: Some("sk-test"), ..Default::default() },
    );
    focus_row(&mut a, ZH.wiz_btn_probe);
    let probe_action = a.handle_key(key(KeyCode::Enter)).expect("Probe 应该产出 Action");
    a.update(probe_action); // 探测在飞, 草稿 base_url = "https://mine.example.com"

    // 一份探测 "https://other.example.com" 的结果, 带着当前代次到达。
    let cmds = a.update(wizard_done(&a, WizardResult::Probed {
        base_url: "https://other.example.com".into(),
        result: Ok(ProbeModelsResult::Auto {
            models: vec![ModelInfo { id: "other-model".into(), display_name: None }],
            models_url: "https://other.example.com/v1/models".into(),
        }),
    }));
    assert!(cmds.is_empty(), "不该产出任何 Cmd");
    // 应该仍然在探测——没被这份不属于自己的结果打回编辑 (打回去了才说明被误采纳了)。
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.wiz_probing), "应该仍然显示探测中, 没被 base_url 不一致的结果打断\n{out}");

    // 真正属于自己的结果 (base_url 一致) 随后到达, 应该被正常采纳, 且候选里不该混入刚才那份。
    let cmds2 = a.update(wizard_done(&a, WizardResult::Probed {
        base_url: "https://mine.example.com".into(),
        result: Ok(ProbeModelsResult::Auto {
            models: vec![ModelInfo { id: "mine-model".into(), display_name: None }],
            models_url: "https://mine.example.com/v1/models".into(),
        }),
    }));
    assert!(cmds2.is_empty());
    // 探测成功后焦点自动落到 Slot(Fable) (`apply_wizard_result` 的 `Probed(Ok(Auto))` 分支), 不用再移动。
    assert_focus(&mut a, slot_row(Slot::Fable));
    let open_action = a.handle_key(key(KeyCode::Enter)).expect("Fable 行 ⏎ 应该产出 Action::OpenPicker");
    let Action::OpenPicker(spec) = open_action else { panic!("应该是 OpenPicker, 实际 {open_action:?}") };
    assert!(spec.items.iter().any(|item| item.id == "mine-model"), "真正属于自己的探测结果应该被采纳\n{:?}", spec.items);
    assert!(!spec.items.iter().any(|item| item.id == "other-model"), "别人的探测结果不该混入候选\n{:?}", spec.items);
}

/// `Models` 同样要核对结果自带的 `id` (代次之外的纵深防御)——一份别的订阅的模型列表带着当前代次
/// 漏过来时, **阶段守卫拦不住 (此刻正在等模型列表)**, 要靠 `id` 不一致丢弃——否则四个核心槽会被
/// 预填成另一家厂商的模型名。
#[test]
fn a_models_result_for_a_different_subscription_is_discarded() {
    let mut a = wizard_after_create(); // 订阅 id = "sub-1", 等模型列表
    let cmds = a.update(wizard_done(&a, WizardResult::Models {
        id: "sub-999".into(), // 不是这个向导建的订阅
        result: Ok(RefreshModelsResult::Auto { models: vec![ModelInfo { id: "other-vendor-model".into(), display_name: None }], fetched_at: 0 }),
    }));
    assert!(cmds.is_empty(), "不该产出任何 Cmd");
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.wiz_loading_models), "应该仍然显示正在获取模型列表, 没被别的订阅的结果打断\n{out}");
    assert!(!out.contains("other-vendor-model"), "别的订阅的模型不该出现\n{out}");
}

/// 向导 A 以 OpenAI Responses 探测中转 U, 结果还没回来就退出; 向导 B 选 Chat Completions、同一个
/// U、另一个 key, 也在探测。A 的结果先到: 阶段 (都在探测) 与 `base_url` (同一个 U) 都对得上,
/// 只有代次能认出它不是 B 发的——采纳了的话, B 的候选模型来自别的协议 / key, A 的 `models_url`
/// 还会被 B 落库。
#[test]
fn a_probe_result_from_a_closed_wizard_is_dropped_even_for_the_same_base_url() {
    let mut a = wizard_custom(CustomProtocol::OpenaiResponses);
    fill_custom(&mut a, CustomFill { base_url: Some("https://relay.example.com"), api_key: Some("sk-a"), ..Default::default() });
    focus_row(&mut a, ZH.wiz_btn_probe);
    let probe_a = a.handle_key(key(KeyCode::Enter)).expect("Probe 应该产出 Action");
    let Some(Cmd::Wizard { epoch: epoch_a, .. }) = a.update(probe_a).into_iter().next() else { panic!("A 应该发出探测请求") };
    escape_and_confirm(&mut a);

    open_custom_wizard(&mut a, CustomProtocol::OpenaiChatCompletions);
    fill_custom(&mut a, CustomFill { base_url: Some("https://relay.example.com"), api_key: Some("sk-b"), ..Default::default() });
    focus_row(&mut a, ZH.wiz_btn_probe);
    let probe_b = a.handle_key(key(KeyCode::Enter)).expect("Probe 应该产出 Action");
    a.update(probe_b); // B 的探测在飞

    let stale = Action::WizardDone {
        epoch: epoch_a,
        result: Box::new(WizardResult::Probed {
            base_url: "https://relay.example.com".into(),
            result: Ok(ProbeModelsResult::Auto {
                models: vec![ModelInfo { id: "responses-model".into(), display_name: None }],
                models_url: "https://relay.example.com/v1/responses/models".into(),
            }),
        }),
    };
    assert!(a.update(stale).is_empty(), "不该产出任何 Cmd");
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.wiz_probing), "A 的结果不该打断 B 的探测\n{out}");

    a.update(wizard_done(
        &a,
        WizardResult::Probed {
            base_url: "https://relay.example.com".into(),
            result: Ok(ProbeModelsResult::Auto {
                models: vec![ModelInfo { id: "chat-model".into(), display_name: None }],
                models_url: "https://relay.example.com/v1/models".into(),
            }),
        },
    ));
    assert_focus(&mut a, slot_row(Slot::Fable));
    let open_action = a.handle_key(key(KeyCode::Enter)).expect("Fable 行 ⏎ 应该产出 Action::OpenPicker");
    let Action::OpenPicker(spec) = open_action else { panic!("应该是 OpenPicker, 实际 {open_action:?}") };
    assert!(spec.items.iter().any(|item| item.id == "chat-model"), "B 自己的探测结果应该被采纳\n{:?}", spec.items);
    assert!(!spec.items.iter().any(|item| item.id == "responses-model"), "A 的探测结果不该混入候选\n{:?}", spec.items);
}

/// 同一场景换成 `Models`: A 建好订阅、在等模型列表时退出, B 同样建好订阅在等模型列表。这里让
/// 两边的订阅 id 相同, 把 `id` 核对排除在外——挡住 A 的结果的只能是代次。
#[test]
fn a_models_result_from_a_closed_wizard_is_dropped_even_for_the_same_id() {
    let mut a = wizard_after_create(); // 订阅 id = "sub-1", LoadModels 在飞
    let epoch_a = a.wizard_epoch().expect("准备: 向导 A 开着");
    escape_and_confirm(&mut a);

    a.update(Action::OpenWizard);
    let providers = wizard_done(&a, WizardResult::Providers(Ok(vec![zhipu_provider()])));
    a.update(providers);
    let submit = submit_basics(&mut a);
    a.update(submit);
    a.update(wizard_done(&a, WizardResult::Created(Ok(CreatedSubscription { id: "sub-1".into() }))));

    let stale = Action::WizardDone {
        epoch: epoch_a,
        result: Box::new(WizardResult::Models {
            id: "sub-1".into(),
            result: Ok(RefreshModelsResult::Auto { models: vec![ModelInfo { id: "stale-model".into(), display_name: None }], fetched_at: 0 }),
        }),
    };
    assert!(a.update(stale).is_empty(), "不该产出任何 Cmd");
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.wiz_loading_models), "A 的结果不该把 B 推进到第二步\n{out}");
    assert!(!out.contains("stale-model"), "A 的模型不该出现\n{out}");

    a.update(wizard_done(
        &a,
        WizardResult::Models {
            id: "sub-1".into(),
            result: Ok(RefreshModelsResult::Auto { models: vec![ModelInfo { id: "glm-4.6".into(), display_name: None }], fetched_at: 0 }),
        },
    ));
    let out = render(&mut a, 80, 24);
    assert!(out.contains("glm-4.6"), "B 自己的模型列表应该被采纳并预填槽位\n{out}");
}

/// 自定义路径创建之前什么都没落库, `Esc` 的确认文案应该是 `confirm_discard`, 不是
/// `wiz_confirm_exit_pending` (那个专属「订阅已经创建」)——编辑中与探测在飞 (只读请求, `Esc`
/// 应该可用) 都要覆盖。
#[test]
fn custom_stage_escape_uses_the_discard_prompt_not_the_pending_one() {
    let mut a = wizard_custom(CustomProtocol::Anthropic);
    assert_eq!(
        a.handle_key(key(KeyCode::Esc)),
        Some(Action::OpenConfirm { prompt: ZH.confirm_discard.to_string(), on_yes: OnYes::discard_then(Action::CloseWizard) }),
        "Custom 阶段的 Esc 确认文案应该是「放弃修改」, 不是「订阅已创建」"
    );

    fill_custom(
        &mut a,
        CustomFill { provider_name: Some("厂商"), base_url: Some("https://relay.example.com"), api_key: Some("sk-test"), ..Default::default() },
    );
    focus_row(&mut a, ZH.wiz_btn_probe);
    let probe_action = a.handle_key(key(KeyCode::Enter)).expect("Probe 应该产出 Action");
    a.update(probe_action); // 探测在飞

    assert!(a.handle_key(key(KeyCode::Esc)).is_some(), "Probing 是只读请求, Esc 应该可用");
    assert_eq!(
        a.handle_key(key(KeyCode::Esc)),
        Some(Action::OpenConfirm { prompt: ZH.confirm_discard.to_string(), on_yes: OnYes::discard_then(Action::CloseWizard) }),
        "Probing 阶段 Esc 的确认文案同样应该是「放弃修改」"
    );
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

/// 底栏 (`Focus::List`) 在 `b 刷新余额` 后面追加了 `n 新建` / `d 删除`——120 列放得下全部提示,
/// 直接断言两个键都出现在渲染结果里 (与 `subscriptions_120x40` 快照互为印证, 这里只关心内容
/// 而不是逐字节布局)。
#[test]
fn the_subscriptions_footer_offers_new_and_delete() {
    let out = render(&mut subs_app(false), 120, 40);
    assert!(out.contains("n 新建"), "底栏应该有 n 新建\n{out}");
    assert!(out.contains("d 删除"), "底栏应该有 d 删除\n{out}");
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

/// Task 5 之前, 宽屏下 `⏎` 没有任何可见效果 (详情面板本来就一直画着)。现在两种宽度下 `⏎` 都会把
/// 焦点切进详情 (窄屏是切一整屏, 宽屏是边框换色), 这条覆盖 80 列的「切一整屏 + Esc 切回去」部分;
/// 宽屏的「`⏎` 确实换了焦点」由 `enter_moves_focus_into_the_detail_on_both_widths` /
/// `focused_pane_has_the_accent_border` 覆盖。
#[test]
fn enter_opens_detail_on_narrow_terminals() {
    let mut a = subs_app(false);
    let before = render(&mut a, 80, 24);
    assert!(before.contains(ZH.sub_col_name) && !before.contains(ZH.sub_f_endpoint), "{before}");

    a.handle_key(key(KeyCode::Enter));
    let after = render(&mut a, 80, 24);
    assert!(after.contains(ZH.sub_f_endpoint) && !after.contains(ZH.sub_col_name), "80 列: ⏎ 应该进详情\n{after}");

    a.handle_key(key(KeyCode::Esc));
    let back = render(&mut a, 80, 24);
    assert!(back.contains(ZH.sub_col_name), "{back}");
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

/// 版本不一致横幅在每种语言的 80 列上完整显示 (折行, 不截掉句尾「重新添加到 PATH」那半句)。
/// 去掉空白后逐字比较横幅几行的文字与原文, 折行位置不影响比较。
#[test]
fn version_mismatch_banner_is_shown_in_full_at_80x24() {
    for lang in Lang::ALL {
        use_lang(lang);
        let mut a = app(false);
        a.update(Action::Connected { app_version: "1.2.3".into() });
        // 订阅页的内容从一条面板上边框开始, 横幅的结束位置一目了然 (总览先画 logo)。
        a.update(Action::SwitchTab(Tab::Subscriptions));
        let buf = render_buffer(&mut a, 80, 24);
        let text = (s().version_mismatch)(VERSION, "1.2.3");
        let squash = |t: &str| t.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        let banner: String = (3..buf.area.height).map(|y| buffer_row_text(&buf, y)).take_while(|row| !row.trim_start().starts_with('╭')).collect();
        assert_eq!(squash(&banner), squash(&format!("⚠{text}")), "{lang:?}: 横幅没有完整显示\n{}", render(&mut a, 80, 24));
    }
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

/// 限额列宽必须只随「配置了哪些周期的上限」变化, 不随「哪个周期用量最紧」变化——同一份配置下,
/// 换一种用量分布让 `tightest_quota()` 从 Daily 翻到 Total (EN 下两个名字宽度差一倍: "Daily" 5列,
/// "Lifetime total" 14列) 不该让整屏的进度条跟着变宽变窄。
#[test]
fn quota_label_column_does_not_jitter_when_the_tightest_period_flips() {
    use_lang(Lang::En);

    fn find_gauge_span(row: &str) -> (usize, usize) {
        let chars: Vec<char> = row.chars().collect();
        let start = chars.iter().position(|c| *c == '━' || *c == '─').unwrap_or_else(|| panic!("找不到进度条\n{row}"));
        let len = chars[start..].iter().take_while(|c| **c == '━' || **c == '─').count();
        (start, len)
    }

    fn render_at(daily_used: u64, total_used: u64) -> String {
        let mut a = app(false);
        a.update(Action::Connected { app_version: VERSION.into() });
        let mut d = data();
        d.settings.preferred_language = "en".into();
        let mut example = sub("1", "Example", SubscriptionState::Healthy);
        example.quota_usage =
            vec![quota_period(QuotaPeriod::Daily, 100, daily_used), quota_period(QuotaPeriod::Total, 1000, total_used)];
        d.subscriptions = vec![example];
        a.update(overview_done(1, d));
        let buf = render_buffer(&mut a, 80, 24);
        let y = (0..buf.area.height).find(|&y| buffer_row_text(&buf, y).contains("Example")).unwrap_or_else(|| panic!("找不到 Example 所在的行"));
        buffer_row_text(&buf, y)
    }

    // 状态 A: daily 用量比例 (0.9) 高于 total (0.1) → tightest = Daily。
    let row_a = render_at(90, 100);
    // 状态 B: total 用量比例 (0.9) 高于 daily (0.1) → tightest = Total。
    let row_b = render_at(10, 900);

    assert_eq!(find_gauge_span(&row_a), find_gauge_span(&row_b), "进度条的位置/宽度不该随「哪个周期最紧」变化\nA: {row_a}\nB: {row_b}");
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
    assert_eq!(
        a.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Some(Action::ForceQuit),
        "Ctrl+C 应该映射到 ForceQuit, 不是会先确认的 Quit"
    );
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

/// Task 2: 帮助弹窗打开时页面键 (数字切页 / r 刷新) 被吞掉, 但 Esc / `?` / q 三个键仍然关闭它——
/// 与确认弹窗不同 (确认弹窗里 q 不等于关闭, 会被吞掉)。Ctrl+C 无论如何都立即退出。
#[test]
fn help_popup_still_closes_with_esc_question_mark_and_q() {
    let mut a = loaded(false);
    a.update(Action::ToggleHelp);
    assert_eq!(a.handle_key(key(KeyCode::Char('2'))), None);
    assert_eq!(a.handle_key(key(KeyCode::Char('r'))), None);
    assert_eq!(a.handle_key(key(KeyCode::Esc)), Some(Action::ClosePopup));
    assert_eq!(a.handle_key(key(KeyCode::Char('?'))), Some(Action::ClosePopup));
    assert_eq!(a.handle_key(key(KeyCode::Char('q'))), Some(Action::ClosePopup));
    assert_eq!(
        a.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Some(Action::ForceQuit),
        "弹窗打开时 Ctrl+C 依然立即退出, 不确认"
    );
}

/// Task 2: 确认弹窗与帮助弹窗的按键语义不同——`q` 在确认弹窗里不是「关闭」, 会被吞掉 (只有
/// y/Y/n/N/Esc/⏎ 有意义); `Action::OpenConfirm` 是公开 API, 不需要页面真的变 dirty 就能触发。
#[test]
fn confirm_popup_swallows_other_keys() {
    let mut a = loaded(false);
    a.update(Action::OpenConfirm { prompt: "测试提示".into(), on_yes: OnYes::discard_then(Action::Refresh) });
    for code in [KeyCode::Char('2'), KeyCode::Char('r'), KeyCode::Char('q'), KeyCode::Char('?'), KeyCode::Tab, KeyCode::Char('x')] {
        assert_eq!(a.handle_key(key(code)), None, "{code:?} 应该被确认弹窗吞掉");
    }
    assert_eq!(a.handle_key(key(KeyCode::Char('Y'))), Some(Action::Confirmed(OnYes::discard_then(Action::Refresh))), "大写 Y 也算「是」");
}

/// Task 2: `n`/`N`/`Esc`/`⏎` 四个键都等同「否」(只关弹窗, 不执行 on_yes); ⏎ 不能被误实现成「是」的默认值。
#[test]
fn enter_defaults_to_no() {
    let mut a = loaded(false);
    for no_key in [KeyCode::Enter, KeyCode::Char('n'), KeyCode::Char('N'), KeyCode::Esc] {
        a.update(Action::OpenConfirm { prompt: "测试提示".into(), on_yes: OnYes::discard_then(Action::Refresh) });
        assert_eq!(a.handle_key(key(no_key)), Some(Action::ClosePopup), "{no_key:?} 应该等同于「否」");
        a.update(Action::ClosePopup);
    }
}

/// Task 2: 干净页面 (`is_dirty()` 默认 `false`) 上 q / 切页应该直接执行, 不弹确认——`guard_dirty`
/// 只在真的 dirty 时才拦截。
#[test]
fn a_clean_page_never_asks() {
    let mut a = loaded(false);
    assert_eq!(a.update(Action::Quit), vec![Cmd::Quit], "干净的总览页 q 应该直接退出, 不弹确认");

    let mut b = loaded(false);
    b.update(Action::SwitchTab(Tab::Subscriptions));
    let out = render(&mut b, 80, 24);
    assert!(out.contains(ZH.sub_col_name), "干净页面切页应该直接执行\n{out}");
    assert!(!out.contains(ZH.confirm_discard), "不该弹确认\n{out}");
}

/// Task 2 新增守卫: 帮助弹窗高度 = 全局键 + 1 (分隔空行) + 页面键 + 4 (上下边框各 1 + 上下内距
/// 各 1) 必须放得下最小终端高度 (24 行), 每个 `Tab` 各自的页面键都要满足——覆盖以后加新页面时
/// 键位表写太长而在最小终端上把帮助弹窗顶穿的回归。
#[test]
fn every_page_help_fits_the_minimum_terminal() {
    let pages = Pages::default();
    for lang in Lang::ALL {
        let s = strings(lang);
        for tab in Tab::ALL {
            let rows = pages.get(tab).help(s);
            let total = s.help_rows.len() + 1 + rows.len() + 4;
            assert!(total <= usize::from(MIN_HEIGHT), "{lang:?} {tab:?}: 帮助弹窗需要 {total} 行, 超过最小终端高度 {MIN_HEIGHT}\n{rows:?}");
        }
    }
}

/// 帮助弹窗在最小终端 (80×24) 上, 每种语言、每一页的每条说明都完整显示——说明比弹窗宽时弹窗
/// 跟着放宽, 不能被右边框截断。
#[test]
fn every_help_row_is_shown_in_full_at_80x24() {
    for lang in Lang::ALL {
        use_lang(lang);
        let s = s();
        let mut a = every_page_loaded();
        for tab in Tab::ALL {
            a.update(Action::SwitchTab(tab));
            a.update(Action::ToggleHelp);
            let out = render(&mut a, 80, 24);
            let page_rows = Pages::default().get(tab).help(s);
            // 键名列宽与 `widgets::help` 的规则相同: 最宽键名 + 2, 不低于 18。整行 (定宽键名 + 说明)
            // 连续出现才算完整显示——分别找键名和说明会被别处碰巧出现的同样文字骗过。
            let all: Vec<&(&str, &str)> = s.help_rows.iter().chain(page_rows).collect();
            let key_col = (all.iter().map(|(key, _)| key.width()).max().unwrap_or(0) + 2).max(18);
            for (key, desc) in all {
                let row = format!("{}{desc}", fit(key, key_col));
                assert!(out.contains(&row), "{lang:?} {tab:?}: 帮助行「{row}」没有完整显示\n{out}");
            }
            a.update(Action::ToggleHelp);
        }
    }
}

/// 每一页都有数据的 `App` (总览 / 订阅 / 虚拟模型 / 实时路由 / 日志), 停在总览页——跨页面的守卫
/// 测试用。数据沿用各页快照的夹具。
fn every_page_loaded() -> App {
    let mut a = loaded(false);
    a.update(Action::SwitchTab(Tab::Subscriptions));
    a.update(subs_done(2, detail_subs()));
    a.update(Action::SwitchTab(Tab::VirtualModels));
    a.update(vm_done(3, vm_list()));
    a.update(Action::SwitchTab(Tab::Live));
    a.update(sse_started("model-sonnet", "1", NOW - 50_000));
    a.update(sse_finished("model-sonnet", "1", true, NOW - 48_200));
    a.update(Action::SwitchTab(Tab::Logs));
    a.update(requests_done(4, RequestQuery::default(), logs_fixture_rows(), 4));
    a.update(Action::SwitchTab(Tab::Overview));
    a
}

/// 80 列底栏在每种语言、每一页都保留 `? 帮助` / `q 退出` 两项全局键, 以及 `1-5` 与这一页的第一个
/// 键位 (页面键位放不下时从右往左丢, 丢到连第一个都放不下就说明译文太长了)。
#[test]
fn every_footer_keeps_help_and_quit_in_80_columns() {
    for lang in Lang::ALL {
        use_lang(lang);
        let s = s();
        let mut a = every_page_loaded();
        for tab in Tab::ALL {
            a.update(Action::SwitchTab(tab));
            let out = render(&mut a, 80, 24);
            let footer = out.lines().last().unwrap_or_else(|| panic!("{out}"));
            let (first_key, first_desc) = Pages::default().get(tab).hints(s)[0];
            for must in [
                format!("? {}", s.key_help),
                format!("q {}", s.key_quit),
                format!("1-5 {}", s.key_switch_tab),
                format!("{first_key} {first_desc}"),
            ] {
                assert!(footer.contains(&must), "{lang:?} {tab:?}: 80 列底栏缺了「{must}」\n{footer}");
            }
        }

        // 有草稿时的底栏: 全局键、保存 / 放弃与这个焦点下的编辑键都必须留下。
        let s_ = s;
        let hint = |k: &str, d: &str| format!("{k} {d}");
        let dirty_cases: [(&str, App, Vec<String>); 3] = [
            ("订阅详情", dirty_subscription_detail(), vec![
                hint("↑↓", s_.key_select),
                hint("s", s_.key_save),
                hint("Esc", s_.key_discard),
                hint("⏎", s_.key_edit_model),
                hint("o", s_.key_edit_effort),
            ]),
            ("虚拟模型成员", dirty_vm_members(), vec![
                hint("↑↓", s_.key_select),
                hint("s", s_.key_save),
                hint("Esc", s_.key_discard),
                hint("J K", s_.key_move),
            ]),
            ("虚拟模型列表", dirty_vm_models(), vec![
                hint("↑↓", s_.key_select),
                hint("s", s_.key_save),
                hint("Esc", s_.key_discard),
                hint("⏎", s_.key_members),
                hint("m", s_.key_mode),
            ]),
        ];
        for (what, mut a, keys) in dirty_cases {
            let out = render(&mut a, 80, 24);
            let footer = out.lines().last().unwrap_or_else(|| panic!("{out}"));
            let globals = [format!("? {}", s.key_help), format!("q {}", s.key_quit), format!("1-5 {}", s.key_switch_tab)];
            for must in globals.iter().chain(&keys) {
                assert!(footer.contains(must.as_str()), "{lang:?} {what} (有草稿): 80 列底栏缺了「{must}」\n{footer}");
            }
        }
    }
}

/// 订阅详情里改了 fable 槽位、还没保存 (与 `subscriptions_dirty_80x24` 同一条驱动路径)。
fn dirty_subscription_detail() -> App {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter));
    a.handle_key(key(KeyCode::Enter));
    a.update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }, choice: PickerChoice::Item("m3".into()) });
    a
}

/// 虚拟模型成员栏里挪了一次顺序、还没保存 (与 `virtual_models_dirty_80x24` 同一条驱动路径)。
fn dirty_vm_members() -> App {
    let mut a = vm_app(false);
    a.handle_key(key(KeyCode::Right));
    a.handle_key(key(KeyCode::Char('J')));
    a
}

/// 虚拟模型列表焦点下切了一次调度模式、还没保存。
fn dirty_vm_models() -> App {
    let mut a = vm_app(false);
    let action = a.handle_key(key(KeyCode::Char('m')));
    if let Some(action) = action {
        a.update(action);
    }
    a
}

#[test]
fn confirm_popup_80x24() {
    let mut a = loaded(false);
    a.update(Action::OpenConfirm { prompt: ZH.confirm_discard.into(), on_yes: OnYes::discard_then(Action::Quit) });
    insta::assert_snapshot!(render(&mut a, 80, 24));
}

/// Task 1: 只读详情弹窗——「基本信息」的 8 个字段 (含一个会折行的「端点」值)、「错误信息」的两行
/// `Text`、「上游响应」超出一屏、逼出右侧滚动条。
#[test]
fn detail_popup_80x24() {
    let mut a = loaded(false);
    a.update(Action::OpenDetail(detail_fixture()));
    insta::assert_snapshot!(render(&mut a, 80, 24));
}

/// Task 1: `j` 只改一行, 画面理应变化; `G` 应该滚到底, 能看到内容最后一段 (拼接的递增数字, 结尾是
/// "0499"); `Esc` 应该关掉弹窗 (标题与底部键位提示都不再出现)。
#[test]
fn detail_popup_scrolls_and_closes() {
    let mut a = loaded(false);
    a.update(Action::OpenDetail(detail_fixture()));
    let opened = render(&mut a, 80, 24);
    assert!(!opened.contains("0499"), "第一帧还没滚动, 不该看到内容末尾\n{opened}");

    a.handle_key(key(KeyCode::Char('j')));
    let after_j = render(&mut a, 80, 24);
    assert_ne!(opened, after_j, "j 之后第一行可见内容应该变了");

    a.handle_key(key(KeyCode::Char('G')));
    let after_g = render(&mut a, 80, 24);
    assert!(after_g.contains("0499"), "G 应该滚到底, 看到内容的最后一段\n{after_g}");

    let esc_action = a.handle_key(key(KeyCode::Esc)).expect("Esc 应该产出 Action::ClosePopup");
    a.update(esc_action);
    let closed = render(&mut a, 80, 24);
    assert!(!closed.contains("请求详情") && !closed.contains(ZH.detail_keys), "Esc 之后弹窗应该已经关闭\n{closed}");
}

/// Task 3: 12 项列表, 输入 "gl" 过滤到只剩 glm 系列 + 置顶的「使用「gl」」自定义行。
#[test]
fn picker_popup_80x24() {
    let mut a = loaded(false);
    a.update(Action::OpenPicker(picker_spec()));
    a.handle_key(key(KeyCode::Char('g')));
    a.handle_key(key(KeyCode::Char('l')));
    insta::assert_snapshot!(render(&mut a, 80, 24));
}

/// Task 3: 弹窗打开时, 全局键 (q / 数字切页 / r / ?) 应该全部被 picker 吞进输入框, 不产生对应的
/// `Action` (对照 `help_popup_still_closes_with_esc_question_mark_and_q`: 帮助弹窗里这些键有专门
/// 语义, picker 里它们只是普通字符)。
#[test]
fn picker_swallows_global_keys() {
    let mut a = loaded(false);
    a.update(Action::OpenPicker(picker_spec()));
    for c in ['q', '2', 'r', '?'] {
        assert_eq!(a.handle_key(key(KeyCode::Char(c))), None, "{c:?} 应该被 picker 吞进输入框");
    }
    let out = render(&mut a, 80, 24);
    assert!(out.contains("q2r?"), "四个字符应该都进了输入框\n{out}");

    // Fix round H: Tab / Shift+Tab 平时是 NextTab / PrevTab, 弹窗打开时同样归 picker 管 (tui-input
    // 自己认得 Tab 键, 返回 None 不改输入框内容), 不该冒出 SwitchTab 的 Action。
    assert_eq!(a.handle_key(key(KeyCode::Tab)), None, "Tab 不该在 picker 打开时切页");
    assert_eq!(a.handle_key(key(KeyCode::BackTab)), None, "BackTab 不该在 picker 打开时切页");
    let out_after_tab = render(&mut a, 80, 24);
    assert!(out_after_tab.contains("选择模型"), "picker 应该还开着, 没有被 Tab 意外关掉或切走\n{out_after_tab}");
}

/// Task 3: 光标只在 picker 弹窗打开时才出现 (ratatui 默认隐藏, 只有 `set_cursor_position` 当帧
/// 调用过才会显示), 而且应该落在弹窗内的输入框那一行, 不在别处。
#[test]
fn picker_shows_the_cursor_in_the_input_row() {
    let mut a = loaded(false);
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|f| a.draw(f, Duration::ZERO)).unwrap();
    assert!(!terminal.backend().cursor_visible(), "没有弹窗时不该显示光标");

    a.update(Action::OpenPicker(picker_spec()));
    a.handle_key(key(KeyCode::Char('g')));
    terminal.draw(|f| a.draw(f, Duration::ZERO)).unwrap();
    assert!(terminal.backend().cursor_visible(), "picker 打开时应该显示光标");
    let pos = terminal.backend().cursor_position();
    let popup = picker::area(ratatui::layout::Rect::new(0, 0, 80, 24));
    assert!(popup.contains(pos), "光标应该落在弹窗内, 实际 {pos:?}, 弹窗 {popup:?}");
}

/// Task 3: `PickerDone` 关掉弹窗并转给当前页面的 `update`。Task 5 补上真实断言: 在订阅详情页
/// 走一遍真实的 ⏎⏎ 输入 ⏎ 流程, 断言草稿真的落地 (显示新模型名 + 「已修改」+ 标题带 `*`)。
#[test]
fn picker_done_reaches_the_current_page() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24); // 记住这是窄屏
    a.handle_key(key(KeyCode::Enter)); // List -> Detail{Fable}
    let open_action = a.handle_key(key(KeyCode::Enter)); // 打开 Fable 槽的模型 picker
    a.update(open_action.expect("第二次 ⏎ 应该产出 Action::OpenPicker"));
    let opened = render(&mut a, 80, 24);
    assert!(opened.contains(&(ZH.pick_model_title)("fable")), "弹窗应该已经打开\n{opened}");

    // 输入框预填了 initial ("glm-4.6"), 先 Ctrl+U 清空再输入自定义值。
    a.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    for c in "glm-x".chars() {
        a.handle_key(key(KeyCode::Char(c)));
    }
    let action = a.handle_key(key(KeyCode::Enter));
    let Some(Action::PickerDone { tag, choice }) = action else {
        panic!("⏎ 应该产出 PickerDone, 实际 {action:?}");
    };
    assert_eq!(tag, PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable });
    assert_eq!(choice, PickerChoice::Custom("glm-x".into()));

    assert!(a.update(Action::PickerDone { tag, choice }).is_empty(), "PickerDone 不产出 Cmd");
    let out = render(&mut a, 80, 24);
    assert!(!opened_picker_title_visible(&out), "PickerDone 之后弹窗应该已经关闭\n{out}");
    assert!(out.contains("glm-x"), "草稿里的新模型名应该显示在详情里\n{out}");
    assert!(out.contains(ZH.sub_slot_modified), "该槽位应该标记为已修改\n{out}");
    assert!(out.contains(" *"), "标题应该带 * 表示有未保存修改\n{out}");
}

/// `picker_done_reaches_the_current_page` 用的小帮手: picker 关闭后弹窗标题理应不再出现——
/// 直接找 "选择" 这个词过于宽泛 (标题里就带这个字), 改成量输入框那一行的边框角标是否消失
/// 更麻烦, 这里用「弹窗的键位提示行不再出现」这个更稳的信号。
fn opened_picker_title_visible(out: &str) -> bool {
    out.contains(ZH.picker_keys)
}

/// Fix round A: `Clear` 本身不修复紧贴弹窗左右边缘、横跨边界的宽字符 (CJK) ——三个弹窗都要经过
/// `widgets::clear_popup_area` 才能保证边框在这种情况下依然完整。总览页的健康度面板里就有会撞上
/// 80×24 下 picker 弹窗左边缘的 "Kimi 备用" (`ui__picker_popup_80x24.snap` 曾经因为这个缺了左边
/// 框), 拿它做背景, 对 Help / Confirm / Picker 三种弹窗分别断言: 弹窗矩形每一行的第一列是左边框
/// 字符、最后一列是右边框字符。
#[test]
fn popup_borders_survive_wide_glyphs_underneath() {
    // 按**显示列**取字符, 不能按 `Vec<char>` 下标——这一行左边可能有 CJK 文本 (比如总览页的订阅
    // 名), 字符数和显示列数不是一回事, 按字符下标取会系统性偏移 (`format::fit` 同一个道理)。
    fn char_at_column(line: &str, target_col: usize) -> char {
        let mut col = 0usize;
        for c in line.chars() {
            let w = c.width().unwrap_or(0).max(1);
            if target_col < col + w {
                return c;
            }
            col += w;
        }
        ' '
    }

    fn assert_borders_intact(out: &str, popup: ratatui::layout::Rect, label: &str) {
        let lines: Vec<&str> = out.lines().map(plain).collect();
        for y in popup.top()..popup.bottom() {
            let line = lines[y as usize];
            let left = char_at_column(line, popup.left() as usize);
            let right = char_at_column(line, popup.right() as usize - 1);
            assert!(['│', '╭', '╰'].contains(&left), "{label} 第 {y} 行左边框缺失 (实际 {left:?})\n{out}");
            assert!(['│', '╮', '╯'].contains(&right), "{label} 第 {y} 行右边框缺失 (实际 {right:?})\n{out}");
        }
    }

    let screen = ratatui::layout::Rect::new(0, 0, 80, 24);

    let mut help_app = loaded(false);
    help_app.update(Action::ToggleHelp);
    let help_area = help::area(screen, &ZH, &[]); // 总览页 (Tab::Overview) 没有自己的页面键位
    assert_borders_intact(&render(&mut help_app, 80, 24), help_area, "Help");

    let mut confirm_app = loaded(false);
    confirm_app.update(Action::OpenConfirm { prompt: ZH.confirm_discard.into(), on_yes: OnYes::discard_then(Action::Quit) });
    let confirm_area = confirm::area(screen, ZH.confirm_discard);
    assert_borders_intact(&render(&mut confirm_app, 80, 24), confirm_area, "Confirm");

    let mut picker_app = loaded(false);
    picker_app.update(Action::OpenPicker(picker_spec()));
    let picker_area = picker::area(screen);
    assert_borders_intact(&render(&mut picker_app, 80, 24), picker_area, "Picker");

    // M3 (fix round final): toast 也贴着页面内容画, 同样可能撞上宽字符横跨边缘——80×24 下总览页的
    // 「鉴权 开启 · 4 个订阅 · 1 个可调度」这行 (y=5) 里, 「个可调度」的「个」字横跨列 66/67;
    // `toast::area` 的左边缘 x = 79 - width, `width = text.width() + 4` (未触顶 76 上限时) ——
    // 选一段显示宽度恰好 8 的文本, 算出 `width=12`, `x=67`, 左边缘前一列正好落在「个」字上,
    // 复现当初 `ui__picker_popup_80x24.snap` 那种「宽字符横跨左边缘」的场景 (`widgets::clear_popup_area`
    // 文档有完整解释)。toast 不是 `Popup` 的一种, 没有走同一个渲染入口, 需要单独验证它也修好了。
    const COLLISION_TEXT: &str = "12345678"; // 显示宽度 8 (全角字符宽度不是 1, 这里刻意用 ASCII)
    let mut toast_app = loaded(false);
    toast_app.update(Action::Notify { kind: ToastKind::Success, text: COLLISION_TEXT.into() });
    let toast_area = toast::area(screen, 3, COLLISION_TEXT);
    assert_eq!(toast_area.left(), 67, "这条用例的前提: 文本宽度选得不对, 撞不上「个」字横跨的那一列");
    assert_borders_intact(&render(&mut toast_app, 80, 24), toast_area, "Toast");
}

#[test]
fn tabs_wrap_around_and_returning_to_a_page_refreshes_it() {
    let mut a = loaded(false);
    // Task 8: 第 5 页 (`Tab::Logs`) 不再是不拉数据的占位页——从总览 `PrevTab` 绕到它应该像其它
    // 真页面一样立刻补拉一次 (第 1 页、无过滤), 并且在没有数据落地之前显示 `s.loading`。
    assert_eq!(a.update(Action::PrevTab), vec![Cmd::Fetch(Fetch::Requests(RequestQuery::default()))], "从总览 PrevTab 应该绕到日志页并补拉第一页");
    assert!(render(&mut a, 80, 24).contains(ZH.loading));
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

    // Task 8: `Tab::Logs` 从这时起也是真页面, 停在第 1 页时同样会轮询——与下面的订阅页一起验证
    // 「当前可见页」在 tick 时真的会发起自己的 Fetch (以前这里用仍是占位页的 `Tab::Logs` 验证
    // 相反的事情: 「占位页不轮询」, 那条断言现在不成立了)。
    let mut b = loaded(false);
    b.update(Action::SwitchTab(Tab::Logs));
    let logs_fetches: Vec<Cmd> = (1..=20).flat_map(|i| b.update(Action::Tick { now_ms: NOW + i * 250 })).collect();
    assert_eq!(logs_fetches, vec![Cmd::Fetch(Fetch::Requests(RequestQuery::default()))], "日志页第 1 页可见时轮询应该发 Fetch::Requests");

    // 停在订阅页时轮询发的是 Fetch::Subscriptions, 不是 Fetch::Overview。
    let mut c = loaded(false);
    c.update(Action::SwitchTab(Tab::Subscriptions));
    let sub_fetches: Vec<Cmd> = (1..=20).flat_map(|i| c.update(Action::Tick { now_ms: NOW + i * 250 })).collect();
    assert_eq!(sub_fetches, vec![Cmd::Fetch(Fetch::Subscriptions)], "订阅页可见时轮询应该发 Fetch::Subscriptions");
}

/// 订阅相关的 SSE 事件在总览页 / 订阅页 (Task 5) / 虚拟模型页 (Task 6) 都会重拉订阅列表——三个
/// 页面都在 `update()` 里消费 `client::events::SUBSCRIPTION_CHANGES`。实时路由页 (Task 7 起是真
/// 页面) 同样会忽略它——它只在 `Refresh`/`Poll`/`Connected` 时拉数据, 不消费这类 SSE (它自己的
/// 数据来自 `route_attempt_*` 事件, 走 `on_event` 不产出 `Cmd`)。
#[test]
fn subscription_events_refetch_the_list_only_on_the_overview() {
    let ev = |name: &str| Action::Sse { name: name.into(), data: "\"1\"".into(), at_ms: NOW };
    let mut a = loaded(false);
    assert_eq!(a.update(ev("subscription_state_changed")), vec![Cmd::Fetch(Fetch::Subscriptions)]);
    assert_eq!(a.update(ev("subscription_quota_reached")), vec![Cmd::Fetch(Fetch::Subscriptions)]);
    assert!(a.update(ev("route_attempt_started")).is_empty());
    a.update(Action::SwitchTab(Tab::Live));
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

/// 校验失败真的会播 `fx::field_err` (不只是移动焦点、挂错误文案)——开着动效提交一份空 API Key
/// 的表单, 这一帧应该切到快速重绘, 播完标称时长后应该停下来。
#[test]
fn a_rejected_submit_plays_the_field_err_effect() {
    let mut a = app(true);
    settle(&mut a); // 先把启动动效播完, 不干扰下面对 field_err 的断言
    a.update(Action::OpenWizard);
    a.update(wizard_done(&a, WizardResult::Providers(Ok(vec![zhipu_provider()]))));
    select_zhipu(&mut a);
    focus_row(&mut a, ZH.wiz_btn_next); // API Key 仍是空的

    assert!(a.handle_key(key(KeyCode::Enter)).is_none(), "校验失败不该产出 Action");
    render(&mut a, 80, 24); // 校验失败发生在 handle_key 里, 这一帧才真的画出错误行、触发动效
    assert!(a.wants_fast_frames(), "校验失败应该播一次 field_err, 主循环该切到快速重绘");

    render_with(&mut a, 80, 24, Duration::from_millis(u64::from(cc_router_tui::fx::ms::FIELD_ERR)));
    assert!(!a.wants_fast_frames(), "超过标称时长后 field_err 应该已经播完");
}

/// `Basics → Slots` 是唯一会播 `fx::wizard_step` 的转场——`Loading → Basics` (拉厂商列表) 不算
/// 「换步」, 不该播。
#[test]
fn advancing_from_basics_to_slots_plays_the_wizard_step_effect_but_loading_providers_does_not() {
    let mut a = app(true);
    settle(&mut a); // 先把启动动效播完

    a.update(Action::OpenWizard);
    a.update(wizard_done(&a, WizardResult::Providers(Ok(vec![zhipu_provider()]))));
    render(&mut a, 80, 24);
    assert!(!a.wants_fast_frames(), "Loading → Basics 不是换步, 不该播 wizard_step");

    let submit_action = submit_basics(&mut a);
    a.update(submit_action); // 创建在飞
    a.update(wizard_done(&a, WizardResult::Created(Ok(CreatedSubscription { id: "sub-1".into() }))));
    a.update(wizard_done(&a, WizardResult::Models {
        id: "sub-1".into(),
        result: Ok(RefreshModelsResult::Auto { models: vec![ModelInfo { id: "glm-4.6".into(), display_name: None }], fetched_at: 0 }),
    }));

    render(&mut a, 80, 24);
    assert!(a.wants_fast_frames(), "Basics → Slots 应该播一次 wizard_step");
    render_with(&mut a, 80, 24, Duration::from_millis(u64::from(cc_router_tui::fx::ms::WIZARD_STEP)));
    assert!(!a.wants_fast_frames(), "超过标称时长后 wizard_step 应该已经播完");
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
        barrier: 0,
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

    // Task 2: 确认弹窗打开的状态。
    let mut f = loaded(false);
    f.update(Action::OpenConfirm { prompt: ZH.confirm_discard.into(), on_yes: OnYes::discard_then(Action::Quit) });
    let first = render(&mut f, 80, 24);
    let second = render(&mut f, 80, 24);
    assert_eq!(first, second, "确认弹窗打开的状态应该幂等");

    // Task 3: picker 弹窗打开的状态 (含光标位置——`TestBackend` 也记录了光标, `render()` 的字符串
    // 比较不包含它, 但 `visual_cursor` / `visual_scroll` 本身必须是纯函数, 这里顺带覆盖)。
    let mut g = loaded(false);
    g.update(Action::OpenPicker(picker_spec()));
    g.handle_key(key(KeyCode::Char('g')));
    let first = render(&mut g, 80, 24);
    let second = render(&mut g, 80, 24);
    assert_eq!(first, second, "picker 弹窗打开的状态应该幂等");

    // Task 1: 详情弹窗打开且已滚动 2 行的状态 (`DetailState::scroll` 参与相等比较, `draw` 每次都要
    // 用同一份折行结果重算滚动条)。`DetailState::new` 的 `last_total` 初值是 0, 在第一次
    // `draw`/`render` 落地真实的 `last_rows`/`last_total` 之前 `max_scroll()` 恒为 0, 这时候按
    // `j` 会被原地夹住——所以必须先画一帧, `j` 才会真的移动。
    let mut n = loaded(false);
    n.update(Action::OpenDetail(detail_fixture()));
    let opened = render(&mut n, 80, 24);
    n.handle_key(key(KeyCode::Char('j')));
    n.handle_key(key(KeyCode::Char('j')));
    let first = render(&mut n, 80, 24);
    assert_ne!(opened, first, "滚动后画面应该真的变了, 不能退化成又在测未滚动的状态");
    let second = render(&mut n, 80, 24);
    assert_eq!(first, second, "详情弹窗滚动后的状态应该幂等");

    // Task 5: 宽屏下焦点在详情 (边框颜色跟 `self.focus` 走, `pane_border_style` 必须是纯函数)。
    let mut h = subs_app(false);
    render(&mut h, 120, 40);
    h.handle_key(key(KeyCode::Enter));
    let first = render(&mut h, 120, 40);
    let second = render(&mut h, 120, 40);
    assert_eq!(first, second, "宽屏详情焦点应该幂等");

    // Task 5: 有未保存草稿的状态 (标题带 `*`、槽位行带「已修改」, `dirty` 缓存必须是确定性重算)。
    let mut i = subs_app(false);
    render(&mut i, 80, 24);
    i.handle_key(key(KeyCode::Enter));
    i.handle_key(key(KeyCode::Enter));
    i.update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }, choice: PickerChoice::Item("m3".into()) });
    let first = render(&mut i, 80, 24);
    let second = render(&mut i, 80, 24);
    assert_eq!(first, second, "有草稿的状态应该幂等");

    // Task 6: 虚拟模型页 — Models 焦点 (默认态)。
    let mut j = vm_app(false);
    let first = render(&mut j, 80, 24);
    let second = render(&mut j, 80, 24);
    assert_eq!(first, second, "虚拟模型页 Models 焦点应该幂等");

    // Task 6: 焦点在右栏 (Members)。
    let mut k = vm_app(false);
    k.handle_key(key(KeyCode::Right));
    let first = render(&mut k, 80, 24);
    let second = render(&mut k, 80, 24);
    assert_eq!(first, second, "虚拟模型页 Members 焦点应该幂等");

    // Task 6: 有未保存草稿的状态 (标题带 `*`)。
    let mut l = vm_app(false);
    l.handle_key(key(KeyCode::Right));
    l.handle_key(key(KeyCode::Char('J')));
    let first = render(&mut l, 80, 24);
    let second = render(&mut l, 80, 24);
    assert_eq!(first, second, "虚拟模型页草稿态应该幂等");

    // Task 6: 保存中 (右栏标题 spinner) 的状态也应该幂等。model-opus (无 ghost 成员, V2 fix round
    // P3b 起 model-fable 自带的 ghost 会让 `s` 变成拒绝通知而不是真的进入保存中态)。
    let mut m = vm_app(false);
    m.handle_key(key(KeyCode::Down)); // model-opus
    m.handle_key(key(KeyCode::Right));
    m.handle_key(key(KeyCode::Char('J')));
    let mutation = m.handle_key(key(KeyCode::Char('s'))).expect("有草稿时 s 应该产出 Action");
    assert!(matches!(mutation, Action::Mutate(_)), "应该真的产出保存动作, 不是被 ghost 守卫拒绝: {mutation:?}");
    m.update(mutation);
    let first = render(&mut m, 80, 24);
    let second = render(&mut m, 80, 24);
    assert_eq!(first, second, "虚拟模型页忙碌态应该幂等");

    // Task 7: 实时路由页 — 跟随最新 (默认态)。
    let mut n = live_fixture_app(false);
    let first = render(&mut n, 80, 24);
    let second = render(&mut n, 80, 24);
    assert_eq!(first, second, "实时路由页跟随最新应该幂等");

    // Task 7: 选中一行 (离开跟随最新)。
    let mut o = live_fixture_app(false);
    o.handle_key(key(KeyCode::Up));
    let first = render(&mut o, 80, 24);
    let second = render(&mut o, 80, 24);
    assert_eq!(first, second, "实时路由页选中一行应该幂等");

    // Task 7: 暂停。
    let mut p = live_fixture_app(false);
    p.handle_key(key(KeyCode::Char(' ')));
    let first = render(&mut p, 80, 24);
    let second = render(&mut p, 80, 24);
    assert_eq!(first, second, "实时路由页暂停应该幂等");

    // Task 8: 日志页已加载且选中一行。
    let mut q = logs_app(false);
    q.update(requests_done(1, RequestQuery::default(), logs_fixture_rows(), 4));
    let first = render(&mut q, 80, 24);
    let second = render(&mut q, 80, 24);
    assert_eq!(first, second, "日志页已加载且选中一行应该幂等");

    // Task 8: 日志详情弹窗已滚动。
    let mut r = logs_app(false);
    r.update(requests_done(1, RequestQuery::default(), logs_fixture_rows(), 4));
    render(&mut r, 80, 24);
    let open_action = r.handle_key(key(KeyCode::Enter)).expect("⏎ 应该产出 Action::OpenDetail");
    r.update(open_action);
    let opened = render(&mut r, 80, 24);
    r.handle_key(key(KeyCode::Char('j')));
    r.handle_key(key(KeyCode::Char('j')));
    let first = render(&mut r, 80, 24);
    assert_ne!(opened, first, "滚动后画面应该真的变了");
    let second = render(&mut r, 80, 24);
    assert_eq!(first, second, "日志详情弹窗滚动后的状态应该幂等");

    // 向导打开 (加载厂商列表中) 的状态——throbber 是唯一读 tick 计数器的渲染路径,
    // 同一帧画两遍必须落在同一格上, 与订阅页忙碌行同一条纪律。
    let mut s = app(false);
    s.update(Action::OpenWizard);
    let first = render(&mut s, 80, 24);
    let second = render(&mut s, 80, 24);
    assert_eq!(first, second, "向导加载中的状态应该幂等");

    // 向导第一步 (已选厂商 + API Key 打了几个字符, 光标在 value 里的位置由
    // `visual_cursor()` 现算) 应该幂等。
    let mut t = wizard_with_providers(vec![zhipu_provider()]);
    select_zhipu(&mut t);
    type_str(&mut t, "sk");
    let first = render(&mut t, 80, 24);
    let second = render(&mut t, 80, 24);
    assert_eq!(first, second, "向导 Basics 阶段应该幂等");

    // 向导第二步 (自动发现的候选已经预填四个核心槽, 聚焦在 Fable 行) 应该幂等——第一次 `render`
    // 顺带取走了 `Basics → Slots` 的 `pending_step_fx`, 第二次不该再画出差异 (即便 fx 关着,
    // `mem::take` 本身也不能让两帧不一样)。
    let mut u = wizard_at_slots(vec![ModelInfo { id: "glm-4.6".into(), display_name: None }]);
    let first = render(&mut u, 80, 24);
    let second = render(&mut u, 80, 24);
    assert_eq!(first, second, "向导 Slots 阶段应该幂等");

    // 校验失败之后的 Basics 阶段 (错误行 + `pending_field_err` 已经被第一次 `render` 取走) 也应该
    // 幂等。
    let mut w = wizard_with_providers(vec![zhipu_provider()]);
    select_zhipu(&mut w);
    focus_row(&mut w, ZH.wiz_btn_next); // API Key 仍是空的
    w.handle_key(key(KeyCode::Enter));
    let first = render(&mut w, 80, 24);
    let second = render(&mut w, 80, 24);
    assert_eq!(first, second, "校验失败后的向导 Basics 阶段应该幂等");

    // 向导自定义单页 (已选 Anthropic 协议、正在填厂商名, 光标状态由
    // `visual_cursor()` 现算) 应该幂等。
    let mut v = wizard_custom(CustomProtocol::Anthropic);
    type_str(&mut v, "中转站");
    let first = render(&mut v, 80, 24);
    let second = render(&mut v, 80, 24);
    assert_eq!(first, second, "向导 Custom 阶段应该幂等");
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
    a.update(Action::MutationDone { mutation, barrier: 0, result: Ok(MutationOutcome::EnabledSet) });

    assert_eq!(
        a.handle_key(key(KeyCode::Char('e'))),
        Some(Action::Mutate(Mutation::SetEnabled { id: "1".into(), enabled: true })),
        "第一次 e 已经把它关掉了 (乐观更新), 这次 e 应该朝相反方向 (重新打开) 走"
    );
    assert_eq!(
        a.update(Action::Mutate(Mutation::SetEnabled { id: "1".into(), enabled: true })),
        vec![Cmd::Mutate(Box::new(Mutation::SetEnabled { id: "1".into(), enabled: true }))],
        "这次应该真的发出去, 不是 no-op"
    );
}

/// Task 1: 一份晚到的、`issued` 不大于变更完成时盖下的 `barrier` 的订阅列表, 就算内容还是变更前的
/// 旧值, 也不该把乐观更新过的状态冲回去——哪怕它的 `issued` 数值比首次加载 (`subs_app` 用的
/// issued=1) 还大。`barrier` 之后的列表则应该正常接受。
#[test]
fn a_list_issued_before_a_mutation_finished_cannot_revert_it() {
    let mut a = subs_app(false); // id "1" (智谱主号) 默认 enabled=true, 首次加载 issued=1。
    assert_eq!(
        a.handle_key(key(KeyCode::Char('e'))),
        Some(Action::Mutate(Mutation::SetEnabled { id: "1".into(), enabled: false }))
    );
    let mutation = Mutation::SetEnabled { id: "1".into(), enabled: false };
    a.update(Action::Mutate(mutation.clone()));
    // 变更完成, 带 barrier=10 (模拟此刻之前已经发起过好几轮加载)。
    let cmds = a.update(Action::MutationDone { mutation, barrier: 10, result: Ok(MutationOutcome::EnabledSet) });
    assert_eq!(cmds, vec![Cmd::Fetch(Fetch::Subscriptions)], "无论成败都该按 refetch() 追加一次订阅列表刷新");
    // 让这次操作的 toast 过期: 否则它的浮层会叠在列表行上面, 干扰下面按文字找行的断言
    // (与 `a_failed_test_connection_stays_readable_in_the_detail` 同一手法)。
    render(&mut a, 80, 24);
    a.update(Action::Tick { now_ms: NOW + 3_000 });

    // 一份晚到的列表 (issued=9 <= barrier=10), 内容是乐观更新之前的旧值 (enabled 仍是 true):
    // 应该被丢弃, 渲染里这条订阅必须保持乐观更新过的新状态 (停用, 符号从 ● 变成 ○)。
    let mut stale = detail_subs();
    stale[0].enabled = true;
    a.update(subs_done(9, stale));

    let out = render(&mut a, 80, 24);
    let zhipu_line = out.lines().find(|l| l.contains("智谱主号")).unwrap_or_else(|| panic!("{out}"));
    assert!(
        !zhipu_line.contains('●') && zhipu_line.contains('○'),
        "issued=9 <= barrier=10, 这份晚到的旧列表不该把乐观更新的停用状态冲回去\n{zhipu_line}"
    );

    // 此时再按 e, 应该发出反方向 (重新启用), 而不是从被丢弃的旧值算出同一个目标 (no-op)。
    assert_eq!(
        a.handle_key(key(KeyCode::Char('e'))),
        Some(Action::Mutate(Mutation::SetEnabled { id: "1".into(), enabled: true })),
        "晚到的旧列表被丢弃, 再按 e 应该基于乐观更新过的状态朝反方向走"
    );

    // 一份更新的列表 (issued=11 > barrier=10) 应该被正常接受。
    let mut fresh = detail_subs();
    fresh[0].enabled = true;
    a.update(subs_done(11, fresh));
    let out2 = render(&mut a, 80, 24);
    let zhipu_line2 = out2.lines().find(|l| l.contains("智谱主号")).unwrap_or_else(|| panic!("{out2}"));
    assert!(zhipu_line2.contains('●'), "issued=11 > barrier=10, 应该被接受\n{zhipu_line2}");
}

/// 同一订阅的操作同时只跑一个 (按订阅 id 判重, 不按 `Mutation` 整体): 连按两次 `t` 第二次被丢弃;
/// 另一条订阅同时 `t` 照发; `MutationDone` 之后可以再发。
#[test]
fn a_mutation_is_issued_once_per_subscription_until_it_finishes() {
    let mut a = subs_app(false);
    let m1 = Mutation::TestConnection { id: "1".into() };
    assert_eq!(a.update(Action::Mutate(m1.clone())), vec![Cmd::Mutate(Box::new(m1.clone()))]);
    assert!(a.update(Action::Mutate(m1.clone())).is_empty(), "同一订阅的第二次 t 应该被丢弃");

    // 忙碌表按订阅 id 判重, 不是按 `Mutation` 整体判重: 同一条订阅上换一种操作 (t 还在跑时按 e)
    // 也该被拒绝, 不能因为 Mutation 值不同就放过去。
    assert!(
        a.update(Action::Mutate(Mutation::SetEnabled { id: "1".into(), enabled: false })).is_empty(),
        "同一条订阅上, 另一种操作也该被忙碌表拒绝"
    );

    let m2 = Mutation::TestConnection { id: "2".into() };
    assert_eq!(a.update(Action::Mutate(m2)), vec![Cmd::Mutate(Box::new(Mutation::TestConnection { id: "2".into() }))], "另一条订阅应该照发");

    let result = Ok(MutationOutcome::Tested(TestConnectionResult {
        ok: true,
        message: "ok".into(),
        http_status: Some(200),
        model_used: None,
        state_reset: false,
    }));
    a.update(Action::MutationDone { mutation: m1.clone(), barrier: 0, result });
    assert_eq!(a.update(Action::Mutate(m1.clone())), vec![Cmd::Mutate(Box::new(m1))], "MutationDone 之后应该可以再发");
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

/// 测试连接还在飞时确认删除同一条订阅: 忙碌表会拒绝这次删除, 必须给出提示——否则用户确认过
/// 删除、却什么都没发生。
#[test]
fn deleting_a_busy_subscription_explains_why_nothing_happened() {
    let mut a = subs_app(false); // 已连接, 默认选中 "1" 智谱主号
    assert!(!a.update(Action::Mutate(Mutation::TestConnection { id: "1".into() })).is_empty(), "准备: 先让这条订阅忙起来");
    render(&mut a, 80, 24);

    let open = a.handle_key(key(KeyCode::Char('d'))).expect("d 应该打开删除确认");
    a.update(open);
    let yes = a.handle_key(key(KeyCode::Char('y'))).expect("确认弹窗里 y 应该产出 Action");
    assert!(a.update(yes).is_empty(), "忙碌中不该真的发删除请求");
    let out = render(&mut a, 80, 24);
    assert!(out.contains(&(ZH.toast_busy)("智谱主号")), "忙碌中删除应该弹提示\n{out}");
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
        let cmds = a.update(Action::MutationDone { mutation, barrier: 0, result });
        assert_eq!(cmds, vec![Cmd::Fetch(Fetch::Subscriptions)], "无论成败都该追加一次订阅列表刷新");
        let out = render(&mut a, 80, 24);
        assert!(out.contains(expect), "缺 {expect:?}\n{out}");
    }
}

/// Task 4: 两个新的保存类操作各自弹对了 toast, 并按 `Mutation::refetch()` 声明的目标追加
/// `Cmd`——`UpdateSlots` 只影响订阅 (`BusyKey::Subscription`, 与其它四个就地操作同一套 refetch);
/// `UpdateVirtualModel` 额外影响虚拟模型列表本身 (`BusyKey::VirtualModel`, refetch 顺序恰好是
/// `[VirtualModels, Subscriptions]`, 不含别的 `Cmd`)。
#[test]
fn saving_outcomes_toast_and_refetch_what_they_declare() {
    let mut a = subs_app(false);
    let slots_mutation = Mutation::UpdateSlots {
        id: "1".into(),
        model_slots: ModelSlots { fable: "f".into(), opus: "o".into(), sonnet: "s".into(), haiku: "h".into(), fallback: String::new(), jev: String::new() },
        slot_efforts: SlotEfforts::default(),
    };
    assert!(!a.update(Action::Mutate(slots_mutation.clone())).is_empty(), "应该真的发出去");
    let cmds = a.update(Action::MutationDone { mutation: slots_mutation, barrier: 0, result: Ok(MutationOutcome::SlotsSaved) });
    assert_eq!(cmds, vec![Cmd::Fetch(Fetch::Subscriptions)]);
    let out = render(&mut a, 80, 24);
    assert!(out.contains("智谱主号：槽位已保存"), "{out}");

    let mut b = app(false);
    assert_eq!(b.update(Action::Connected { app_version: VERSION.into() }), vec![Cmd::Fetch(Fetch::Overview)]);
    let vm_mutation =
        Mutation::UpdateVirtualModel { name: "model-sonnet".into(), mode: RoutingMode::RoundRobin, subscription_ids: vec!["1".into()] };
    assert!(!b.update(Action::Mutate(vm_mutation.clone())).is_empty(), "应该真的发出去");
    let cmds = b.update(Action::MutationDone { mutation: vm_mutation, barrier: 0, result: Ok(MutationOutcome::VirtualModelSaved) });
    assert_eq!(cmds, vec![Cmd::Fetch(Fetch::VirtualModels), Cmd::Fetch(Fetch::Subscriptions)], "顺序应该是先虚拟模型后订阅");
    let out = render(&mut b, 80, 24);
    assert!(out.contains("model-sonnet：已保存"), "{out}");
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
    a.update(Action::MutationDone { mutation, barrier: 0, result });
    let out = render(&mut a, 80, 24);
    assert!(out.contains("1：连接正常"), "订阅不在 Store 里时应该退回用 id\n{out}");
}

/// 80 列下页面键位放不下时应该被裁掉, 但全局的 `?` 帮助 / `q` 退出必须一直在。
#[test]
fn global_keys_survive_at_80_columns_on_the_subscriptions_page() {
    let out = render(&mut subs_app(false), 80, 24);
    assert!(out.contains(ZH.key_help) && out.contains(ZH.key_quit), "{out}");

    // Task 5: 详情态的键位栏比以前长了 (多了 ⏎ 改模型 / o 改档位 / s 保存, 排在 e/t/m/b 前面);
    // 80 列放不下时 keybar 从右往左丢, e/t/m/b 排最后, 现在轮到它们被裁掉——这是简报明确接受的
    // 取舍 (「后面仍跟 e t m b, 放不下由 keybar 规则丢」), 核心的改槽位三个键必须留下。
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter));
    let detail_out = render(&mut a, 80, 24);
    let footer = detail_out.lines().last().unwrap_or_else(|| panic!("{detail_out}"));
    assert!(
        footer.contains(ZH.key_edit_model) && footer.contains(ZH.key_edit_effort) && footer.contains(ZH.key_save),
        "详情态键位栏至少要放得下改模型 / 改档位 / 保存这三个核心键\n{footer}"
    );
    assert!(footer.contains(ZH.key_help) && footer.contains(ZH.key_quit), "{footer}");
}

/// S1(a) (fix round P3b): `⏎` 现在两种宽度下都会真的把焦点切进详情 (Task 5 起), 不该只在窄屏才
/// 提示——宽屏用户同样需要知道这个键。
#[test]
fn enter_hint_shows_in_the_list_focus_at_every_width() {
    for (w, h) in [(80, 24), (120, 40)] {
        let out = render(&mut subs_app(false), w, h);
        let footer = out.lines().last().unwrap_or_else(|| panic!("{w}x{h}: {out}"));
        assert!(footer.contains(ZH.key_detail), "{w}x{h}: 列表态键位栏应该显示 ⏎ 详情\n{footer}");
    }
}

/// S1(b) (fix round P3b): 焦点落在的槽位行应该整行 `REVERSED`, 之前没有单测直接检查过这一点。
#[test]
fn focused_slot_row_is_reversed() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter)); // Detail{Fable}
    let buf = render_buffer(&mut a, 80, 24);
    let y = (0..buf.area.height)
        .find(|&y| buffer_row_text(&buf, y).contains("fable"))
        .unwrap_or_else(|| panic!("找不到 fable 槽这一行"));
    let style = find_cell_style(&buf, y, "fable");
    assert!(style.add_modifier.contains(Modifier::REVERSED), "焦点落在的槽位行应该整行 REVERSED");

    // 移开焦点之后 (opus 槽), fable 那一行不再 REVERSED。
    a.handle_key(key(KeyCode::Char('j')));
    let buf2 = render_buffer(&mut a, 80, 24);
    let style2 = find_cell_style(&buf2, y, "fable");
    assert!(!style2.add_modifier.contains(Modifier::REVERSED), "焦点移开后不该再是 REVERSED\n{}", buffer_row_text(&buf2, y));
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
        barrier: 0,
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
        barrier: 0,
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

// ---------- 订阅详情里改槽位 (Task 5: 焦点模型 + 草稿 + 保存) ----------

/// 提取 `Action::OpenPicker` 里的 `PickerTag::SlotModel { slot, .. }`——测槽位光标移动时反复用。
fn opened_model_slot(action: Option<Action>) -> Slot {
    match action {
        Some(Action::OpenPicker(spec)) => match spec.tag {
            PickerTag::SlotModel { slot, .. } => slot,
            other => panic!("期待 SlotModel tag, 实际 {other:?}"),
        },
        other => panic!("期待打开模型 picker, 实际 {other:?}"),
    }
}

/// 两种宽度下 `⏎` 都能把焦点从列表切进详情: 用「再按一次 ⏎ 是否真的打开了模型 picker」这个
/// 可观察的副作用验证 (而不是宽屏下本来就一直显示详情内容这件事——那个在 Enter 前后不变,
/// 咬不住焦点有没有真的切过去)。
#[test]
fn enter_moves_focus_into_the_detail_on_both_widths() {
    for (w, h) in [(80, 24), (120, 40)] {
        let mut a = subs_app(false);
        render(&mut a, w, h);
        assert_eq!(a.handle_key(key(KeyCode::Enter)), None, "{w}x{h}: 第一次 ⏎ 只是切焦点, 不产出 Action");
        let action = a.handle_key(key(KeyCode::Enter));
        assert_eq!(opened_model_slot(action), Slot::Fable, "{w}x{h}: 第二次 ⏎ 应该已经在详情焦点, 打开 Fable 槽的模型 picker");
    }
}

/// 宽屏下有焦点的那一栏边框是 `theme.accent`, 另一栏是普通 `theme.border`。
#[test]
fn focused_pane_has_the_accent_border() {
    let theme = Theme::new(ColorMode::TrueColor);
    let mut a = subs_app(false);
    let buf = render_buffer(&mut a, 120, 40);
    // 内容区顶边在屏幕第 3 行 (标签栏占 3 行, 无版本不一致横幅); 左栏左上角在 x=0, 右栏左上角
    // 在 x=list_width (120<140, 应该是 58)。
    assert_eq!(buf[(0, 3)].style().fg, Some(theme.accent), "List 焦点时左栏边框应该是 accent 色");
    assert_eq!(buf[(58, 3)].style().fg, Some(theme.border), "List 焦点时右栏边框应该是普通 border 色");

    a.handle_key(key(KeyCode::Enter)); // 切到 Detail 焦点
    let buf2 = render_buffer(&mut a, 120, 40);
    assert_eq!(buf2[(0, 3)].style().fg, Some(theme.border), "Detail 焦点时左栏边框应该变回普通 border 色");
    assert_eq!(buf2[(58, 3)].style().fg, Some(theme.accent), "Detail 焦点时右栏边框应该是 accent 色");
}

/// 五个槽位间移动, 不绕回; `g`/`G` 跳首尾。
#[test]
fn slot_cursor_moves_and_clamps() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter)); // List -> Detail{Fable}

    assert_eq!(a.handle_key(key(KeyCode::Up)), None, "已经在第一个槽位, 不该绕回");
    assert_eq!(opened_model_slot(a.handle_key(key(KeyCode::Enter))), Slot::Fable);

    a.handle_key(key(KeyCode::Char('j')));
    assert_eq!(opened_model_slot(a.handle_key(key(KeyCode::Enter))), Slot::Opus);
    a.handle_key(key(KeyCode::Down));
    assert_eq!(opened_model_slot(a.handle_key(key(KeyCode::Enter))), Slot::Sonnet);
    a.handle_key(key(KeyCode::Char('k')));
    assert_eq!(opened_model_slot(a.handle_key(key(KeyCode::Enter))), Slot::Opus);

    a.handle_key(key(KeyCode::Char('G')));
    assert_eq!(opened_model_slot(a.handle_key(key(KeyCode::Enter))), Slot::Fallback);
    a.handle_key(key(KeyCode::Char('j')));
    assert_eq!(opened_model_slot(a.handle_key(key(KeyCode::Enter))), Slot::Fallback, "到底不该越界");

    a.handle_key(key(KeyCode::Char('g')));
    assert_eq!(opened_model_slot(a.handle_key(key(KeyCode::Enter))), Slot::Fable);
}

/// 「智谱主号」(detail_subs 里第一条) 有 12 个缓存模型 (m0..m11); Fable 槽的模型 picker 应该用
/// 它们填充, 允许自定义输入, `initial` 是该槽当前值。
#[test]
fn enter_opens_a_model_picker_with_cached_models_and_custom_allowed() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter)); // Detail{Fable}
    let action = a.handle_key(key(KeyCode::Enter));
    let Some(Action::OpenPicker(spec)) = action else { panic!("{action:?}") };
    assert_eq!(spec.tag, PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable });
    assert!(spec.allow_custom, "应该允许自定义输入");
    assert_eq!(spec.items.len(), 12, "应该用该订阅的 model_cache 填充\n{:?}", spec.items);
    assert!(spec.items.iter().any(|i| i.id == "m0"), "{:?}", spec.items);
    assert_eq!(spec.initial, "glm-4.6", "initial 应该是该槽当前值 (还没有草稿时是 Store 里的原值)");
}

/// 兜底槽的模型 picker 在 items 最前面多一项「清空」。
#[test]
fn fallback_slot_offers_a_clear_item_first() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter)); // Detail{Fable}
    a.handle_key(key(KeyCode::Char('G'))); // -> Fallback
    let action = a.handle_key(key(KeyCode::Enter));
    let Some(Action::OpenPicker(spec)) = action else { panic!("{action:?}") };
    assert_eq!(spec.tag, PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fallback });
    assert_eq!(spec.items.first().map(|i| i.id.as_str()), Some(""), "第一项应该是清空兜底槽\n{:?}", spec.items);
    assert_eq!(spec.items.first().map(|i| i.label.as_str()), Some(ZH.pick_clear_fallback));
}

/// 一条 System One 订阅 (只有 Jev 槽) 的订阅页。
fn systemone_subs_app(jev: &str) -> App {
    let mut s1 = sub("9", "Ollama 决策", SubscriptionState::Healthy);
    s1.provider_display_name = "Ollama".into();
    s1.endpoint_protocol = "systemone".into();
    s1.model_slots = ModelSlots::default();
    s1.model_slots.jev = jev.into();
    let mut a = app(false);
    a.update(Action::Connected { app_version: VERSION.into() });
    a.update(Action::SwitchTab(Tab::Subscriptions));
    a.update(subs_done(1, vec![s1]));
    a
}

/// System One 订阅的详情只有 Jev 一行 (空 = 透传), 不画四个主槽与兜底槽。
#[test]
fn systemone_detail_shows_only_the_jev_slot() {
    let mut a = systemone_subs_app("");
    let out = render(&mut a, 120, 40);
    let jev_row = out.lines().find(|l| l.contains("│   jev ")).unwrap_or_else(|| panic!("应该有 jev 槽位行\n{out}"));
    assert!(jev_row.contains(ZH.sub_slot_unset), "{out}");
    for slot in [Slot::Fable, Slot::Opus, Slot::Sonnet, Slot::Haiku, Slot::Fallback] {
        // 详情槽位行是「边框 + 内距 + 两格缩进 + 槽位名」; 列表表头的 sonnet 列不算。
        assert!(!out.lines().any(|l| l.contains(&format!("│   {} ", slot_row(slot)))), "不该画 {slot:?} 槽\n{out}");
    }
    let mut a = systemone_subs_app("clef-flash");
    let out = render(&mut a, 120, 40);
    assert!(out.lines().any(|l| l.contains("│   jev ") && l.contains("clef-flash")), "{out}");
}

#[test]
fn systemone_detail_80x24() {
    let mut a = systemone_subs_app("clef-flash");
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter));
    insta::assert_snapshot!("systemone_detail_80x24", render(&mut a, 80, 24));
}

/// Jev 槽: 光标只在这一行, `o` 拒绝, picker 置顶「清空 (透传)」, 清空是合法值。
#[test]
fn systemone_jev_slot_keys() {
    let mut a = systemone_subs_app("clef-flash");
    render(&mut a, 120, 40);
    a.handle_key(key(KeyCode::Enter)); // Detail{Jev}
    for code in [KeyCode::Down, KeyCode::Up, KeyCode::Char('G'), KeyCode::Char('g')] {
        a.handle_key(key(code));
    }
    assert_eq!(
        a.handle_key(key(KeyCode::Char('o'))),
        Some(Action::Notify { kind: ToastKind::Info, text: ZH.sub_effort_na_jev.into() })
    );
    let action = a.handle_key(key(KeyCode::Enter));
    let Some(Action::OpenPicker(spec)) = action else { panic!("{action:?}") };
    assert_eq!(spec.tag, PickerTag::SlotModel { sub_id: "9".into(), slot: Slot::Jev });
    assert_eq!(spec.items.first().map(|i| (i.id.as_str(), i.label.as_str())), Some(("", ZH.pick_clear_jev)));
    assert_eq!(spec.initial, "clef-flash");

    // 清空不是「必填项为空」: 进草稿, 保存时发空 Jev 槽。
    a.update(Action::OpenPicker(spec));
    a.update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "9".into(), slot: Slot::Jev }, choice: PickerChoice::Item(String::new()) });
    let out = render(&mut a, 120, 40);
    assert!(!out.contains(ZH.sub_model_required), "{out}");
    let Some(Action::Mutate(Mutation::UpdateSlots { model_slots, .. })) = a.handle_key(key(KeyCode::Char('s'))) else {
        panic!("应该发保存\n{out}")
    };
    assert_eq!(model_slots, ModelSlots::default());
}

/// System One 订阅没有模型列表: `m` (列表 / 详情焦点都一样) 只给提示, 不发刷新请求;
/// 对话订阅的 `m` 照旧发请求。
#[test]
fn m_on_a_systemone_subscription_toasts_instead_of_refreshing() {
    let mut a = systemone_subs_app("clef-flash");
    render(&mut a, 120, 40);
    let toast = Some(Action::Notify { kind: ToastKind::Info, text: ZH.sub_models_na_systemone.into() });
    assert_eq!(a.handle_key(key(KeyCode::Char('m'))), toast);
    a.handle_key(key(KeyCode::Enter)); // Detail{Jev}
    assert_eq!(a.handle_key(key(KeyCode::Char('m'))), toast);

    let mut b = app(false);
    b.update(Action::Connected { app_version: VERSION.into() });
    b.update(Action::SwitchTab(Tab::Subscriptions));
    b.update(subs_done(1, vec![sub("1", "智谱主号", SubscriptionState::Healthy)]));
    render(&mut b, 120, 40);
    assert_eq!(b.handle_key(key(KeyCode::Char('m'))), Some(Action::Mutate(Mutation::RefreshModels { id: "1".into() })));
}

/// 详情焦点下选中的订阅被别处删掉、选中项落到一条 System One 订阅上: 光标不该还停在它没有的
/// fable 槽, `⏎` 打开的是 Jev 槽的 picker。
#[test]
fn stale_slot_cursor_falls_back_to_the_jev_slot() {
    let mut a = systemone_subs_app("clef-flash");
    let mut s1 = sub("9", "Ollama 决策", SubscriptionState::Healthy);
    s1.provider_display_name = "Ollama".into();
    s1.endpoint_protocol = "systemone".into();
    s1.model_slots = ModelSlots::default();
    a.update(subs_done(2, vec![sub("1", "智谱主号", SubscriptionState::Healthy), s1.clone()]));
    render(&mut a, 120, 40);
    a.handle_key(key(KeyCode::Char('g')));
    a.handle_key(key(KeyCode::Enter)); // Detail{Fable} on "1"
    a.update(subs_done(3, vec![s1]));
    render(&mut a, 120, 40);
    let action = a.handle_key(key(KeyCode::Enter));
    let Some(Action::OpenPicker(spec)) = action else { panic!("{action:?}") };
    assert_eq!(spec.tag, PickerTag::SlotModel { sub_id: "9".into(), slot: Slot::Jev });
}

/// 选定一个模型: 创建草稿, 该槽位行标记「已修改」, 标题带 `*`。
#[test]
fn picking_a_model_creates_a_draft_and_marks_the_row() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter));
    a.handle_key(key(KeyCode::Enter)); // 打开 Fable picker

    a.update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }, choice: PickerChoice::Item("m3".into()) });
    let out = render(&mut a, 80, 24);
    assert!(out.contains("m3"), "{out}");
    assert!(out.contains(ZH.sub_slot_modified), "{out}");
    assert!(out.contains(" *"), "标题应该带 *\n{out}");
}

/// 选回原来的值: 草稿还在 (值等于 Store), 但 dirty 应该清掉——标题不再带 `*`, Esc 也不再弹确认。
#[test]
fn picking_the_original_value_again_clears_dirty() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter));
    a.handle_key(key(KeyCode::Enter));
    a.update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }, choice: PickerChoice::Item("m3".into()) });
    assert!(render(&mut a, 80, 24).contains(" *"));

    a.handle_key(key(KeyCode::Enter)); // 重新打开 picker
    a.update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }, choice: PickerChoice::Custom("glm-4.6".into()) });
    let clean_out = render(&mut a, 80, 24);
    assert!(!clean_out.contains(" *"), "改回原值应该清掉 dirty\n{clean_out}");
    assert!(!clean_out.contains(ZH.sub_slot_modified), "{clean_out}");

    // 不脏时 Esc 不该弹确认。
    assert_eq!(a.handle_key(key(KeyCode::Esc)), None, "不脏时 Esc 不该弹确认");
}

/// 主槽 (非兜底) 选了空白自定义值应该被拒绝 (不产生草稿, 弹 `sub_model_required`); 兜底槽的空白
/// 等于清空 (应该被接受)。
#[test]
fn custom_blank_is_rejected_for_main_slots_but_clears_the_fallback() {
    let mut a = subs_app(false);
    let mut subs = detail_subs();
    subs[0].model_slots.fallback = "glm-4.5".into(); // 让兜底槽本来就有值, 清空才算一次真的变化
    a.update(subs_done(2, subs));
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter)); // Fable

    a.handle_key(key(KeyCode::Enter));
    assert!(a
        .update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }, choice: PickerChoice::Custom("   ".into()) })
        .is_empty());
    let out = render(&mut a, 80, 24);
    assert!(!out.contains(" *"), "空白应该被拒绝, 不产生草稿\n{out}");
    assert!(out.contains(ZH.sub_model_required), "{out}");

    a.handle_key(key(KeyCode::Char('G'))); // -> Fallback (现在值 "glm-4.5")
    a.handle_key(key(KeyCode::Enter));
    a.update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fallback }, choice: PickerChoice::Custom("  ".into()) });
    let out2 = render(&mut a, 80, 24);
    assert!(out2.contains(" *"), "兜底槽清空 (从有值变没有值) 应该产生草稿\n{out2}");
    assert!(out2.contains(ZH.sub_slot_unset), "{out2}");
}

/// 思考档位 picker: `auto` (id 空串) + `EFFORT_CHOICES` 五档, 不允许自定义, initial 是当前 effort。
#[test]
fn effort_picker_lists_auto_plus_the_allowlist() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter)); // Fable
    a.handle_key(key(KeyCode::Char('j'))); // Opus (effort=high)
    let action = a.handle_key(key(KeyCode::Char('o')));
    let Some(Action::OpenPicker(spec)) = action else { panic!("{action:?}") };
    assert_eq!(spec.tag, PickerTag::SlotEffort { sub_id: "1".into(), slot: Slot::Opus });
    assert!(!spec.allow_custom, "不该允许自定义档位");
    assert_eq!(spec.items.len(), 1 + EFFORT_CHOICES.len());
    assert_eq!(spec.items[0].id, "");
    assert_eq!(spec.items[0].label, ZH.sub_effort_auto);
    for (item, expect) in spec.items[1..].iter().zip(EFFORT_CHOICES.iter()) {
        assert_eq!(item.id, *expect);
    }
    assert_eq!(spec.initial, "high", "initial 应该是当前 effort 值");
}

/// 兜底槽和 Kiro 订阅上按 `o` 应该直接拒绝, 不开弹窗。
#[test]
fn effort_is_refused_for_fallback_and_kiro() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter));
    a.handle_key(key(KeyCode::Char('G'))); // Fallback
    assert_eq!(
        a.handle_key(key(KeyCode::Char('o'))),
        Some(Action::Notify { kind: ToastKind::Info, text: ZH.sub_effort_na_fallback.into() })
    );

    let mut subs = detail_subs();
    subs[0].auth_type = "kiro_oauth".into();
    let mut b = subs_app(false);
    b.update(subs_done(2, subs));
    render(&mut b, 80, 24);
    b.handle_key(key(KeyCode::Enter)); // Fable, 非兜底但是 kiro 订阅
    assert_eq!(
        b.handle_key(key(KeyCode::Char('o'))),
        Some(Action::Notify { kind: ToastKind::Info, text: ZH.sub_effort_na_kiro.into() })
    );
}

/// `s` 保存整块 model_slots + slot_efforts (改一个槽, 其余槽位与原 effort 原样带上)。
#[test]
fn s_saves_the_whole_slots_and_efforts() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter)); // Fable
    a.handle_key(key(KeyCode::Enter));
    a.update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }, choice: PickerChoice::Item("m3".into()) });

    let action = a.handle_key(key(KeyCode::Char('s')));
    assert_eq!(
        action,
        Some(Action::Mutate(Mutation::UpdateSlots {
            id: "1".into(),
            model_slots: ModelSlots {
                fable: "m3".into(),
                opus: "glm-4.6".into(),
                sonnet: "glm-4.6".into(),
                haiku: "glm-4.5-air".into(),
                fallback: String::new(),
                jev: String::new(),
            },
            slot_efforts: SlotEfforts { opus: Some("high".into()), ..Default::default() },
        }))
    );
}

/// 不脏时 `s` 无动作; 有草稿时断线按 `s` 仍然产出 `Action::Mutate`——断线拒绝是 `App::start_mutation`
/// 统一处理的, 不是页面自己判断连接状态。
#[test]
fn s_does_nothing_when_clean_and_refuses_offline() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter));
    assert_eq!(a.handle_key(key(KeyCode::Char('s'))), None, "不脏时 s 不该有动作");

    a.handle_key(key(KeyCode::Enter)); // 打开 picker
    a.update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }, choice: PickerChoice::Item("m3".into()) });
    a.update(Action::ConnectionLost);
    let action = a.handle_key(key(KeyCode::Char('s')));
    assert!(matches!(action, Some(Action::Mutate(Mutation::UpdateSlots { .. }))), "{action:?}");
    assert!(a.update(action.unwrap()).is_empty(), "断线时应该被 App 拒绝, 不真的发");
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.toast_offline), "{out}");
}

/// 草稿期间来两次轮询刷新 (`FetchDone` 落地的新订阅列表) 不该冲掉草稿; `MutationDone(Err)` 保留
/// 草稿, 只有 `MutationDone(Ok(SlotsSaved))` 才清掉。
#[test]
fn draft_survives_polling_and_is_cleared_only_by_a_successful_save() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter));
    a.handle_key(key(KeyCode::Enter));
    a.update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }, choice: PickerChoice::Item("m3".into()) });
    let dirty_out = render(&mut a, 80, 24);
    assert!(dirty_out.contains("m3") && dirty_out.contains(" *"), "{dirty_out}");

    a.update(subs_done(3, detail_subs()));
    a.update(subs_done(4, detail_subs()));
    let out_after_polls = render(&mut a, 80, 24);
    assert!(out_after_polls.contains("m3") && out_after_polls.contains(" *"), "轮询不该冲掉草稿\n{out_after_polls}");

    let mutation = Mutation::UpdateSlots {
        id: "1".into(),
        model_slots: ModelSlots {
            fable: "m3".into(),
            opus: "glm-4.6".into(),
            sonnet: "glm-4.6".into(),
            haiku: "glm-4.5-air".into(),
            fallback: String::new(),
            jev: String::new(),
        },
        slot_efforts: SlotEfforts { opus: Some("high".into()), ..Default::default() },
    };
    a.update(Action::Mutate(mutation.clone()));
    a.update(Action::MutationDone { mutation: mutation.clone(), barrier: 0, result: Err("网络错误".into()) });
    let out_after_fail = render(&mut a, 80, 24);
    assert!(out_after_fail.contains("m3") && out_after_fail.contains(" *"), "失败应该保留草稿\n{out_after_fail}");

    a.update(Action::Mutate(mutation.clone()));
    a.update(Action::MutationDone { mutation, barrier: 0, result: Ok(MutationOutcome::SlotsSaved) });
    let out_after_ok = render(&mut a, 80, 24);
    assert!(!out_after_ok.contains(" *"), "成功后草稿应该被清掉\n{out_after_ok}");
}

// ---------- I1 (fix round final): 保存飞行中拒绝继续编辑 (两个页面各一份, 同名) ----------
//
// 取代旧版 `edits_made_while_a_save_is_in_flight_survive_it` (订阅页) /
// `reorders_made_while_a_save_is_in_flight_survive_it` (虚拟模型页, D1 fix round P3b) ——那两个
// 名字描述的行为 ("在途编辑照样生效, 只是不会被冲掉") 已经不对: I1 把"照样生效"改成了"直接拒绝"。

mod subscriptions_saving_in_flight {
    use super::*;

    /// 一次保存发出去之后 (还没等结果回来), 旧版靠 `on_mutation_done` 里"只有负载与当前草稿完全
    /// 相等才清空"这条规则保证飞行中的新编辑不被冲掉——但这只解决了"不丢失", 没有解决"用户不知道
    /// 这么做不安全"。I1 改成直接拒绝: 保存在飞行中时, 任何会继续修改草稿的按键 (`⏎`/`o`/再按一次
    /// `s`, 以及防御性覆盖的 `PickerDone` 直接落地) 都被拒绝, 弹 `saving_in_progress`, 草稿原样
    /// 不动; `MutationDone(Ok)` + 随后的 refetch 落地后页面变干净、显示保存后的真值;
    /// `MutationDone(Err)` 落地后草稿原样保留、且又能正常编辑 (`saving` 标记被摘掉)。
    #[test]
    fn edits_are_refused_while_a_save_is_in_flight() {
        let mut a = subs_app(false);
        render(&mut a, 80, 24);
        a.handle_key(key(KeyCode::Enter));
        a.handle_key(key(KeyCode::Enter)); // 打开 Fable picker
        a.update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }, choice: PickerChoice::Item("m3".into()) });

        let action = a.handle_key(key(KeyCode::Char('s')));
        let mutation = match action {
            Some(Action::Mutate(m)) => m,
            other => panic!("{other:?}"),
        };
        assert_eq!(a.update(Action::Mutate(mutation.clone())), vec![Cmd::Mutate(Box::new(mutation.clone()))]);

        // 保存 (m3) 还在飞行中: ⏎ / o / 再按一次 s 都该被拒绝, 弹同一条 saving_in_progress 提示。
        // 像真实运行时一样把 `handle_key` 的返回值转发进 `update` (`Action::Notify` 才会真的入队)。
        for code in [KeyCode::Enter, KeyCode::Char('o'), KeyCode::Char('s')] {
            let action = a.handle_key(key(code));
            assert_eq!(
                action,
                Some(Action::Notify { kind: ToastKind::Info, text: ZH.saving_in_progress.into() }),
                "{code:?} 应该在保存飞行中被拒绝"
            );
            a.update(action.unwrap());
        }
        // 防御性: 就算真的收到一条针对这条订阅的 `PickerDone` (正常流程走不到, 因为上面 ⏎ 已经被拒绝、
        // 不会真的打开新 picker), 也不该被应用。
        assert!(a
            .update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }, choice: PickerChoice::Item("m9".into()) })
            .is_empty());
        let out_while_saving = render(&mut a, 80, 24);
        assert!(out_while_saving.contains("m3") && !out_while_saving.contains("m9"), "飞行中的编辑都不该生效\n{out_while_saving}");
        assert!(out_while_saving.contains(ZH.saving_in_progress), "{out_while_saving}");

        // 结果回来了 (成功), 后端随后带回来的刷新结果也是 m3 (保存后的真值)。
        a.update(Action::MutationDone { mutation: mutation.clone(), barrier: 0, result: Ok(MutationOutcome::SlotsSaved) });
        let mut after_save = detail_subs();
        after_save[0].model_slots.fable = "m3".into();
        a.update(subs_done(2, after_save));
        let out_ok = render(&mut a, 80, 24);
        assert!(!out_ok.contains(" *"), "保存成功后应该变干净\n{out_ok}");
        assert!(out_ok.contains("m3"), "{out_ok}");

        // 另起一局: 保存失败之后草稿应该原样保留, 而且又能正常编辑了 (saving 标记被摘掉)。
        let mut b = subs_app(false);
        render(&mut b, 80, 24);
        b.handle_key(key(KeyCode::Enter));
        b.handle_key(key(KeyCode::Enter));
        b.update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }, choice: PickerChoice::Item("m3".into()) });
        let mutation_b = match b.handle_key(key(KeyCode::Char('s'))) {
            Some(Action::Mutate(m)) => m,
            other => panic!("{other:?}"),
        };
        b.update(Action::Mutate(mutation_b.clone()));
        b.update(Action::MutationDone { mutation: mutation_b, barrier: 0, result: Err("网络错误".into()) });
        let out_err = render(&mut b, 80, 24);
        assert!(out_err.contains("m3") && out_err.contains(" *"), "失败应该保留草稿\n{out_err}");
        let action_after_err = b.handle_key(key(KeyCode::Enter));
        assert!(matches!(action_after_err, Some(Action::OpenPicker(_))), "失败之后应该又能正常编辑, 实际 {action_after_err:?}");
    }
}

/// D2(a) (fix round P3b): 选中一个新模型再选回原值——草稿应该被真的丢弃 (不是「存在但不脏」的
/// 僵尸态), 之后 `t` 应该正常发出 `TestConnection`, 不该被 `sub_save_first` 拦下。
#[test]
fn a_reverted_draft_is_dropped_and_actions_work_again() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter));
    a.handle_key(key(KeyCode::Enter)); // 打开 Fable picker
    a.update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }, choice: PickerChoice::Item("m3".into()) });
    assert!(render(&mut a, 80, 24).contains(" *"));

    a.handle_key(key(KeyCode::Enter)); // 重新打开 picker
    a.update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }, choice: PickerChoice::Custom("glm-4.6".into()) }); // 改回原值
    let clean_out = render(&mut a, 80, 24);
    assert!(!clean_out.contains(" *"), "{clean_out}");

    let action = a.handle_key(key(KeyCode::Char('t')));
    assert_eq!(
        action,
        Some(Action::Mutate(Mutation::TestConnection { id: "1".into() })),
        "改回原值后草稿应该被真的丢弃, t 应该正常生效, 不该被「先保存」拦下\n{action:?}"
    );
}

/// D2(c) (fix round P3b): 没有草稿的页面, 别的客户端 (桌面 app) 改了这条订阅的槽位——不该凭空
/// 冒出 `*`。
#[test]
fn a_clean_page_never_turns_dirty_from_someone_elses_change() {
    // 订阅详情页。
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter)); // 进详情, 但不编辑, 没有草稿
    let mut changed = detail_subs();
    changed[0].model_slots.fable = "someone-else-changed-it".into();
    a.update(subs_done(2, changed));
    a.update(Action::Refresh);
    let out = render(&mut a, 80, 24);
    assert!(!out.contains(" *"), "没有草稿时, 别的客户端改动不该显示 *\n{out}");
    assert!(out.contains("someone-else-changed-it"), "应该正常显示新值\n{out}");

    // 虚拟模型页: 同理。
    let mut b = vm_app(false);
    b.handle_key(key(KeyCode::Right)); // 进 Members, 没有编辑, 没有草稿
    let mut vms = vm_list();
    vms.iter_mut().find(|vm| vm.name == "model-fable").unwrap().mode = RoutingMode::RoundRobin;
    b.update(vm_done(2, vms));
    b.update(Action::Refresh);
    let out2 = render(&mut b, 80, 24);
    assert!(!out2.contains(" *"), "{out2}");
}

/// 有草稿时 Esc 弹确认, 选「是」丢弃草稿并回到列表。
#[test]
fn esc_with_a_draft_asks_and_yes_returns_to_the_list() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter));
    a.handle_key(key(KeyCode::Enter));
    a.update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }, choice: PickerChoice::Item("m3".into()) });

    let action = a.handle_key(key(KeyCode::Esc));
    assert_eq!(action, Some(Action::OpenConfirm { prompt: ZH.confirm_discard.into(), on_yes: OnYes::discard_then(Action::DiscardDraft) }));
    assert!(a.update(action.unwrap()).is_empty());
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.confirm_discard), "{out}");

    assert_eq!(a.handle_key(key(KeyCode::Char('y'))), Some(Action::Confirmed(OnYes::discard_then(Action::DiscardDraft))));
    a.update(Action::Confirmed(OnYes::discard_then(Action::DiscardDraft)));
    let out2 = render(&mut a, 80, 24);
    assert!(out2.contains(ZH.sub_col_name), "确认放弃后应该回到列表\n{out2}");
}

/// 有草稿时切页 / 退出也该先弹确认 (走的是 `App::guard_dirty`, 与 Task 2 的确认流程共用)。
#[test]
fn leaving_the_tab_or_quitting_with_a_draft_asks() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter));
    a.handle_key(key(KeyCode::Enter));
    a.update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }, choice: PickerChoice::Item("m3".into()) });

    assert!(a.update(Action::Quit).is_empty(), "有草稿时 q 应该先确认");
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.confirm_discard), "{out}");
    a.update(Action::ClosePopup);

    assert!(a.update(Action::SwitchTab(Tab::Overview)).is_empty(), "有草稿时切页应该先确认");
    let out2 = render(&mut a, 80, 24);
    assert!(out2.contains(ZH.confirm_discard), "{out2}");
}

/// 有草稿时 e/t/m/b 一律被拒绝, 弹 `sub_save_first`。
#[test]
fn mutation_keys_are_refused_while_dirty() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter));
    a.handle_key(key(KeyCode::Enter));
    a.update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }, choice: PickerChoice::Item("m3".into()) });

    for code in [KeyCode::Char('e'), KeyCode::Char('t'), KeyCode::Char('m'), KeyCode::Char('b')] {
        assert_eq!(
            a.handle_key(key(code)),
            Some(Action::Notify { kind: ToastKind::Info, text: ZH.sub_save_first.into() }),
            "{code:?} 应该被拒绝"
        );
    }
}

/// 草稿对应的订阅从 `Store` 消失 (被别处删除) 时, 下一次 `update` 应该丢弃草稿、焦点退回列表、
/// 弹 `sub_gone`。
#[test]
fn draft_is_dropped_when_the_subscription_disappears() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter));
    a.handle_key(key(KeyCode::Enter));
    a.update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }, choice: PickerChoice::Item("m3".into()) });
    assert!(render(&mut a, 80, 24).contains(" *"));

    let mut without_zhipu = detail_subs();
    without_zhipu.remove(0);
    a.update(subs_done(2, without_zhipu));
    // S1(c) (fix round P3b) 起 `subs_done` 广播 `on_subscriptions_changed` 那一刻本身就已经核对过
    // 草稿了 (见 `sub_gone_is_noticed_as_soon_as_the_list_arrives`); 这里再补一次真正的 `update()`
    // (`Refresh`, 与切页/轮询同一条路) 纯粹是双重确认——即便没有这一行, 上面 `subs_done` 那一刻
    // 就已经该生效了。
    a.update(Action::Refresh);
    let out = render(&mut a, 80, 24);
    assert!(!out.contains(" *"), "订阅消失后草稿应该被丢弃\n{out}");
    assert!(out.contains(ZH.sub_gone), "{out}");
    assert!(out.contains(ZH.sub_col_name), "焦点应该退回列表\n{out}");
}

/// S1(c) (fix round P3b): 草稿对应的订阅消失这件事, 不用等下一次真正的 `update()` 动作——`Store`
/// 刚接受新列表 (`on_subscriptions_changed`) 那一刻就该立刻生效, 不留一帧的延迟。
#[test]
fn sub_gone_is_noticed_as_soon_as_the_list_arrives() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter));
    a.handle_key(key(KeyCode::Enter));
    a.update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }, choice: PickerChoice::Item("m3".into()) });
    assert!(render(&mut a, 80, 24).contains(" *"));

    let mut without_zhipu = detail_subs();
    without_zhipu.remove(0);
    // 只喂 `subs_done`, 不额外调 `Action::Refresh`——如果通知本身不会立刻核对草稿, 这里应该还
    // 看得到 `*` 和详情面板。
    a.update(subs_done(2, without_zhipu));
    let out = render(&mut a, 80, 24);
    assert!(!out.contains(" *"), "订阅消失后草稿应该立刻被丢弃, 不用等下一次 update()\n{out}");
    assert!(out.contains(ZH.sub_gone), "{out}");
    assert!(out.contains(ZH.sub_col_name), "焦点应该立刻退回列表\n{out}");
}

/// I5: picker 打开时 (还没等用户 ⏎ 完成选择) 引用的订阅被别处删掉了——`resolve_selection` 会把
/// `selected_id` 重新指向新列表里同一下标的另一条 (焦点仍停在 `Detail`, 因为这时候还没有草稿,
/// `sync_draft_with_store` 提前 return, 不会主动把焦点挪回列表)。这时候用户在弹窗里按下 ⏎, 落地
/// 的 `PickerDone` 仍然带着弹窗打开那一刻钉住的旧 `sub_id`——不该被套用到现在选中的另一条订阅上。
#[test]
fn a_picker_result_for_a_vanished_subscription_is_ignored() {
    let mut a = subs_app(false); // 默认选中 "1" (智谱主号)
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter)); // List -> Detail{Fable}
    let open_action = a.handle_key(key(KeyCode::Enter)).expect("第二次 ⏎ 应该打开 picker");
    match &open_action {
        Action::OpenPicker(spec) => assert_eq!(spec.tag, PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }),
        other => panic!("{other:?}"),
    }
    a.update(open_action); // 弹窗打开, tag 钉住 sub_id == "1"

    // "1" 被删除 (别处), 轮询带回新列表——resolve_selection 把 selected_id 重新指向新列表同一
    // 下标的订阅 ("2", Kimi 备用)。
    let mut without_zhipu = detail_subs();
    without_zhipu.remove(0);
    a.update(subs_done(2, without_zhipu));
    render(&mut a, 80, 24); // 触发一次 resolve_selection, 把 selected_id 重新指向 "2"

    // 用户这时候在 (已经过时的) 弹窗里选中了一个模型——落地的 PickerDone 仍然带着旧 sub_id "1"。
    assert!(a
        .update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }, choice: PickerChoice::Item("m3".into()) })
        .is_empty());
    let out = render(&mut a, 80, 24);
    assert!(!out.contains(" *"), "消失订阅的 picker 结果不该产生任何草稿\n{out}");
    assert!(!out.contains("m3"), "更不该把结果错误地写进现在选中的另一条订阅\n{out}");
}

/// P3b 遗留项 (Task 5): 选择器弹窗打开时 (用户还没在里面选过任何值, **没有草稿**) 它所属的订阅从
/// `Store` 消失了——旧版靠订阅详情页发出的 `sub_gone` 通知按文案特判着关弹窗, 那条通知只在已经有
/// 草稿时才会产出, 这个场景根本不会触发, 弹窗会一直挂着一条已经不存在的订阅。`App` 现在直接查
/// 弹窗自己的 `tag().subscription_id()`, 不依赖任何页面通知。顺带断言只有一条 `sub_gone` toast
/// (订阅页自己因为没有草稿, `sync_draft_with_store` 提前 return, 不会重复发出同一条)——推进
/// `Tick` 超过 `toast::LIFETIME_MS` 之后这条应该彻底消失, 不会有第二条排在后面顶上来。
#[test]
fn a_picker_closes_when_its_subscription_disappears_even_without_a_draft() {
    let mut a = subs_app(false); // 默认选中 "1" (智谱主号)
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter)); // List -> Detail{Fable}
    let open_action = a.handle_key(key(KeyCode::Enter)).expect("第二次 ⏎ 应该打开 picker");
    match &open_action {
        Action::OpenPicker(spec) => assert_eq!(spec.tag, PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }),
        other => panic!("{other:?}"),
    }
    a.update(open_action); // 弹窗打开, 还没有任何草稿
    assert!(opened_picker_title_visible(&render(&mut a, 80, 24)), "准备: 弹窗应该已经打开");

    let mut without_zhipu = detail_subs();
    without_zhipu.remove(0);
    a.update(subs_done(2, without_zhipu));

    let out = render(&mut a, 80, 24);
    assert!(!opened_picker_title_visible(&out), "订阅消失后弹窗应该被自动关掉\n{out}");
    assert!(out.contains(ZH.sub_gone), "应该弹出 sub_gone 提示\n{out}");

    a.update(Action::Tick { now_ms: NOW + toast::LIFETIME_MS + 1 });
    let out2 = render(&mut a, 80, 24);
    assert!(!out2.contains(ZH.sub_gone), "toast 过期之后不该还看得到, 证明没有第二条排在后面\n{out2}");
}

/// I5: 虚拟模型页同理——弹窗结果里的 `vm` 对不上当前选中的虚拟模型时静默忽略。虚拟模型固定只有
/// 5 个、选中项在有草稿时也换不掉, 这个场景理论上走不到, 但 `PickerTag` 已经带着 `vm` 字段, 这里
/// 直接注入一条不一致的 `PickerDone` 覆盖这条防御性分支。
#[test]
fn a_picker_result_for_another_virtual_model_is_ignored() {
    let mut a = vm_app(false); // 默认选中 model-fable (ids: ["1", ghost], 2 个成员)
    assert!(a
        .update(Action::PickerDone { tag: PickerTag::VmAddSubscription { vm: "model-sonnet".into() }, choice: PickerChoice::Item("2".into()) })
        .is_empty());
    let out = render(&mut a, 80, 24);
    assert!(!out.contains(" *"), "跟当前选中项不一致的虚拟模型不该被应用\n{out}");
    let fable_line = vm_list_row(&out, "model-fable");
    assert!(fable_line.contains('2'), "model-fable 的成员数不该被这条无关的 PickerDone 改变\n{fable_line}");
}

/// I2 修正: 短模型名不该把 effort 列推到很远的右边——模型名结尾与 "high" 之间的距离不该超过
/// 模型列下限 (24), 用「智谱主号」的 opus 槽 (`glm-4.6`, effort=high) 断言。
#[test]
fn effort_column_hugs_the_model_name() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter));
    let out = render(&mut a, 80, 24);

    let opus_row = out.lines().map(plain).find(|l| l.contains("opus") && l.contains("high")).unwrap_or_else(|| panic!("缺 opus 槽这一行\n{out}"));
    let model_end = opus_row.find("glm-4.6").unwrap() + "glm-4.6".len();
    let effort_start = opus_row.find("high").unwrap();
    assert!(
        effort_start > model_end && effort_start - model_end <= 24,
        "「high」应该紧跟在模型名后面 (间距 ≤ 24), 不该被推到很远的右边 (实际间距 {})\n{opus_row}",
        effort_start - model_end
    );
}

/// I3: 140 列起左栏放宽到 72 列, 放得下状态列 (120–139 仍是 58 列, 见既有的
/// `list_has_a_status_column_with_cooldown_when_there_is_room`)。
#[test]
fn wide_140_shows_the_status_column() {
    let out = render(&mut subs_app(false), 140, 40);
    let header_line = out.lines().find(|l| l.contains(ZH.sub_col_name)).unwrap_or_else(|| panic!("缺表头行\n{out}"));
    let list_half = header_line.split("││").next().unwrap_or(header_line);
    assert!(list_half.contains(ZH.sub_col_state), "140 列左栏应该有 72 列, 放得下状态列\n{header_line}");
}

#[test]
fn subscriptions_detail_focus_120x40() {
    let mut a = subs_app(false);
    render(&mut a, 120, 40);
    a.handle_key(key(KeyCode::Enter));
    insta::assert_snapshot!(render(&mut a, 120, 40));
}

#[test]
fn subscriptions_dirty_80x24() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter));
    a.handle_key(key(KeyCode::Enter));
    a.update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }, choice: PickerChoice::Item("m3".into()) });
    insta::assert_snapshot!(render(&mut a, 80, 24));
}

// ---------- 删除订阅 + `n` 新建向导的触发点 ----------

/// 按 `d` 应该先弹确认, 列出引用它的两个虚拟模型 (`detail_subs()` 的"智谱主号",
/// `referenced_by = ["model-sonnet", "model-opus"]`)。`y` 应该真的产出 `Cmd::Mutate(Delete)`。
#[test]
fn deleting_asks_first_and_lists_the_referencing_virtual_models() {
    let mut a = subs_app(false); // 默认选中 "1" 智谱主号
    render(&mut a, 80, 24);

    let expected_prompt =
        format!("{}\n{}\n{}", (ZH.sub_confirm_delete)("智谱主号"), (ZH.sub_delete_refs)(2), "model-sonnet、model-opus");
    let action = a.handle_key(key(KeyCode::Char('d')));
    assert_eq!(
        action,
        Some(Action::OpenConfirm {
            prompt: expected_prompt,
            on_yes: OnYes::run(Action::Mutate(Mutation::Delete { id: "1".into() })),
        })
    );
    a.update(action.unwrap());
    let out = render(&mut a, 80, 24);
    assert!(out.contains("智谱主号") && out.contains("model-sonnet") && out.contains("model-opus"), "{out}");

    // n → 只关掉弹窗, 什么都不产出 (不发 Delete, 没有任何 Cmd)。
    assert_eq!(a.handle_key(key(KeyCode::Char('n'))), Some(Action::ClosePopup), "n 应该只关弹窗");
    assert!(a.update(Action::ClosePopup).is_empty(), "n 之后不该产出任何 Cmd");

    // 重新走一遍, 这次按 y → 产出 Cmd::Mutate(Delete)。
    let action2 = a.handle_key(key(KeyCode::Char('d'))).expect("重新打开确认弹窗");
    a.update(action2);
    assert_eq!(
        a.handle_key(key(KeyCode::Char('y'))),
        Some(Action::Confirmed(OnYes::run(Action::Mutate(Mutation::Delete { id: "1".into() })))),
        "y 应该产出 Confirmed(Mutate(Delete))"
    );
    let cmds = a.update(Action::Confirmed(OnYes::run(Action::Mutate(Mutation::Delete { id: "1".into() }))));
    assert_eq!(cmds, vec![Cmd::Mutate(Box::new(Mutation::Delete { id: "1".into() }))], "确认后应该真的发出删除请求");
}

/// 删除确认是执行类 (`OnYes::Run`): 选「是」只发删除, 不碰当前页草稿。今天 `d` 只在列表焦点下
/// 生效 (列表焦点 ⇒ 没有草稿), 这里绕过按键守卫直接投递确认, 模拟将来在详情焦点下也能删除的
/// 情形——草稿必须还在。对照: 同一份草稿遇上放弃类 (`OnYes::DiscardThen`) 确认, 哪怕 `inner`
/// 与草稿无关, 草稿也要被丢掉。
#[test]
fn a_delete_confirmation_keeps_the_draft_but_a_discard_confirmation_drops_it() {
    fn app_with_draft() -> App {
        let mut a = subs_app(false);
        render(&mut a, 80, 24);
        a.handle_key(key(KeyCode::Enter));
        a.handle_key(key(KeyCode::Enter));
        a.update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }, choice: PickerChoice::Item("m3".into()) });
        a
    }
    let asks_before_leaving = |a: &mut App| {
        a.handle_key(key(KeyCode::Esc))
            == Some(Action::OpenConfirm { prompt: ZH.confirm_discard.into(), on_yes: OnYes::discard_then(Action::DiscardDraft) })
    };

    let mut a = app_with_draft();
    assert!(asks_before_leaving(&mut a), "准备: 页面应该有草稿");
    a.update(Action::ClosePopup);
    let delete = Mutation::Delete { id: "3".into() };
    a.update(Action::OpenConfirm { prompt: (ZH.sub_confirm_delete)("示例中转"), on_yes: OnYes::run(Action::Mutate(delete.clone())) });
    let yes = a.handle_key(key(KeyCode::Char('y'))).expect("确认弹窗里 y 应该产出 Action");
    assert_eq!(a.update(yes), vec![Cmd::Mutate(Box::new(delete))], "确认后应该真的发出删除请求");
    assert!(asks_before_leaving(&mut a), "删除确认不该丢掉草稿\n{}", render(&mut a, 80, 24));

    let mut b = app_with_draft();
    b.update(Action::OpenConfirm { prompt: ZH.confirm_discard.into(), on_yes: OnYes::discard_then(Action::Refresh) });
    let yes = b.handle_key(key(KeyCode::Char('y'))).expect("确认弹窗里 y 应该产出 Action");
    b.update(yes);
    assert!(!asks_before_leaving(&mut b), "放弃类确认应该丢掉草稿\n{}", render(&mut b, 80, 24));
}

/// 没有任何虚拟模型引用的订阅 ("3" 示例中转, `detail_subs()` 里没设置 `referenced_by`, 默认空) 上
/// 按 `d`, prompt 应该只有一行 (没有第二 / 三行)。
#[test]
fn deleting_a_subscription_nothing_references_shows_a_one_line_prompt() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Char('j')));
    a.handle_key(key(KeyCode::Char('j'))); // -> "3" 示例中转

    let action = a.handle_key(key(KeyCode::Char('d')));
    assert_eq!(
        action,
        Some(Action::OpenConfirm {
            prompt: (ZH.sub_confirm_delete)("示例中转"),
            on_yes: OnYes::run(Action::Mutate(Mutation::Delete { id: "3".into() })),
        }),
        "没有引用方时 prompt 不该有第二 / 三行"
    );
}

/// 引用方超过 4 个时, 第三行只列前 4 个再接 `sub_delete_refs_more`; 参数是**剩余**个数 (5 - 4 = 1),
/// 不是总数。
#[test]
fn many_referencing_virtual_models_are_truncated() {
    let mut a = app(false);
    a.update(Action::Connected { app_version: VERSION.into() });
    a.update(Action::SwitchTab(Tab::Subscriptions));
    let mut popular = sub("1", "热门订阅", SubscriptionState::Healthy);
    popular.referenced_by =
        vec!["model-fable".into(), "model-opus".into(), "model-sonnet".into(), "model-haiku".into(), "model-fallback".into()];
    a.update(subs_done(1, vec![popular]));
    render(&mut a, 80, 24);

    let action = a.handle_key(key(KeyCode::Char('d'))).expect("应该产出确认弹窗");
    let Action::OpenConfirm { prompt, .. } = action else { panic!("{action:?}") };
    assert!(prompt.contains("model-fable、model-opus、model-sonnet、model-haiku"), "应该只列前 4 个\n{prompt}");
    assert!(prompt.contains(&(ZH.sub_delete_refs_more)(1)), "剩余 1 个应该折成 sub_delete_refs_more(1)\n{prompt}");
    assert!(!prompt.contains("model-fallback"), "第 5 个不该原样出现在列表里\n{prompt}");
}

/// `n` 只在 `Focus::List` 下有对应的按键分支 (打开新建向导); `Focus::Detail` 下 (没有草稿时,
/// 顶部守卫不拦) `d`/`n` 落到那边既有的 `_ => None`, 什么都不做——与 e/t/m/b 不同, 这两个键刻意
/// 不在 `Focus::Detail` 复制一份分支。
#[test]
fn n_opens_the_wizard_only_from_the_list_focus() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    assert_eq!(a.handle_key(key(KeyCode::Char('n'))), Some(Action::OpenWizard), "List 焦点下 n 应该打开向导");

    a.handle_key(key(KeyCode::Enter)); // List -> Detail{Fable}, 没有草稿
    assert_eq!(a.handle_key(key(KeyCode::Char('d'))), None, "Detail 焦点下 d 不该有反应");
    assert_eq!(a.handle_key(key(KeyCode::Char('n'))), None, "Detail 焦点下 n 不该有反应");
}

/// 有草稿时 `d`/`n` 一律被拒绝, 弹 `sub_save_first`——不开弹窗、不开向导 (返回值本身就是
/// `Notify`, 不是 `OpenConfirm`/`OpenWizard`, `handle_key` 又是纯函数, 没有调用方去
/// `App::update` 它就不会产生任何副作用)。
#[test]
fn delete_and_new_are_refused_while_the_page_has_a_draft() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    a.handle_key(key(KeyCode::Enter));
    a.handle_key(key(KeyCode::Enter));
    a.update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }, choice: PickerChoice::Item("m3".into()) });

    for code in [KeyCode::Char('d'), KeyCode::Char('n')] {
        assert_eq!(
            a.handle_key(key(code)),
            Some(Action::Notify { kind: ToastKind::Info, text: ZH.sub_save_first.into() }),
            "{code:?} 应该被拒绝, 不该是 OpenConfirm/OpenWizard"
        );
    }
}

/// 删除成功: toast 文案是 `toast_deleted(名字)`, 并且按 `refetch()` 补一次 `Fetch::Subscriptions`。
#[test]
fn a_successful_delete_toasts_and_refetches() {
    let mut a = subs_app(false); // "1" 智谱主号
    let mutation = Mutation::Delete { id: "1".into() };
    assert_eq!(a.update(Action::Mutate(mutation.clone())), vec![Cmd::Mutate(Box::new(mutation.clone()))]);
    let cmds = a.update(Action::MutationDone { mutation, barrier: 0, result: Ok(MutationOutcome::Deleted) });
    assert_eq!(cmds, vec![Cmd::Fetch(Fetch::Subscriptions)], "无论成败都该按 refetch() 追加一次订阅列表刷新");
    let out = render(&mut a, 80, 24);
    assert!(out.contains(&(ZH.toast_deleted)("智谱主号")), "{out}");
}

#[test]
fn delete_confirm_80x24() {
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    let action = a.handle_key(key(KeyCode::Char('d'))).expect("应该产出确认弹窗");
    a.update(action);
    insta::assert_snapshot!("delete_confirm_80x24", render(&mut a, 80, 24));
}

// ---------- 虚拟模型页 (Task 6) ----------

/// 定位左栏「这个虚拟模型」所在的列表行——不能只 `find(|l| l.contains(name))`: 右栏边框的
/// `title_top` 在选中这个虚拟模型时也会显示同一个名字, 且排在左栏列表行前面 (border 行先画)。
/// 左栏列表行同时带着模式短名, 用这个多一条件把它跟边框标题行区分开。
fn vm_list_row<'a>(out: &'a str, name: &str) -> &'a str {
    let modes = [ZH.vm_mode_seq, ZH.vm_mode_rr, ZH.vm_mode_sticky, ZH.vm_mode_unknown];
    out.lines()
        .find(|l| l.contains(name) && modes.iter().any(|m| l.contains(m)))
        .unwrap_or_else(|| panic!("找不到 {name} 所在的列表行\n{out}"))
}

/// 5 条订阅, 覆盖虚拟模型页要用到的每条规则:
///   - "1" 智谱主号: 健康, api_key, 无兜底槽。
///   - "2" Kimi 备用: 限流, api_key。
///   - "3" 示例中转: 凭证失效, api_key——放进 model-fallback 验证「api_key 类永远不标记跳过」。
///   - "4" Gemini 中转: 翻译类 (`auth_type` 非 api_key) 且没配兜底槽——放进 model-fallback 验证
///     「将被跳过」标记。
///   - "5" Gemini 有兜底: 翻译类但配了兜底槽——验证「配了兜底槽的不标记」。
fn vm_subs() -> Vec<Subscription> {
    let zhipu = sub("1", "智谱主号", SubscriptionState::Healthy);
    let mut kimi = sub("2", "Kimi 备用", SubscriptionState::RateLimited);
    kimi.provider_display_name = "Moonshot".into();
    let mut relay = sub("3", "示例中转", SubscriptionState::AuthFailed);
    relay.provider_display_name = "自定义".into();
    let mut gemini = sub("4", "Gemini 中转", SubscriptionState::Healthy);
    gemini.provider_display_name = "Gemini".into();
    gemini.auth_type = "gemini_api_key".into();
    let mut gemini_fb = sub("5", "Gemini 有兜底", SubscriptionState::Healthy);
    gemini_fb.provider_display_name = "Gemini".into();
    gemini_fb.auth_type = "gemini_api_key".into();
    gemini_fb.model_slots.fallback = "gemini-2.5-flash".into();
    let mut ollama_s1 = sub("6", "Ollama 决策", SubscriptionState::Healthy);
    ollama_s1.provider_display_name = "Ollama".into();
    ollama_s1.endpoint_protocol = "systemone".into();
    ollama_s1.model_slots = ModelSlots::default();
    ollama_s1.model_slots.jev = "clef-flash".into();
    vec![zhipu, kimi, relay, gemini, gemini_fb, ollama_s1]
}

/// 5 个虚拟模型, 后端固定顺序。`model-fable` 的列表里混进一个 `Store` 里找不到的 id (「已删除」的
/// 订阅), `model-haiku` 留空 (测 `vm_empty`), `model-fallback` 覆盖三种「将被跳过」判定。
fn vm_list() -> Vec<VirtualModel> {
    vec![
        VirtualModel {
            name: "model-fable".into(),
            mode: RoutingMode::Sequential,
            subscription_ids: vec!["1".into(), "ghost-legacy-sub-999".into()],
        },
        VirtualModel { name: "model-opus".into(), mode: RoutingMode::RoundRobin, subscription_ids: vec!["1".into(), "2".into(), "3".into()] },
        VirtualModel { name: "model-sonnet".into(), mode: RoutingMode::Sticky, subscription_ids: vec!["1".into(), "2".into(), "4".into()] },
        VirtualModel { name: "model-haiku".into(), mode: RoutingMode::Sequential, subscription_ids: vec![] },
        VirtualModel {
            name: "model-fallback".into(),
            mode: RoutingMode::Sequential,
            subscription_ids: vec!["3".into(), "4".into(), "5".into()],
        },
        VirtualModel { name: "model-jev".into(), mode: RoutingMode::Sequential, subscription_ids: vec!["6".into()] },
    ]
}

fn vm_done(issued: u64, vms: Vec<VirtualModel>) -> Action {
    Action::FetchDone { fetch: Fetch::VirtualModels, issued, result: Ok(FetchData::VirtualModels(vms)) }
}

/// 连上 + 切到虚拟模型页 + 喂一份虚拟模型列表与订阅列表。
fn vm_app_with(fx_enabled: bool, vms: Vec<VirtualModel>, subs: Vec<Subscription>) -> App {
    let mut a = app(fx_enabled);
    a.update(Action::Connected { app_version: VERSION.into() });
    a.update(Action::SwitchTab(Tab::VirtualModels));
    a.update(vm_done(1, vms));
    a.update(subs_done(1, subs));
    a
}

fn vm_app(fx_enabled: bool) -> App {
    vm_app_with(fx_enabled, vm_list(), vm_subs())
}

/// 按一次键, 产出的 `Action` (若有) 立刻交给 `update`——`a` 这类键靠它打开弹窗 / 弹 toast。
fn press(a: &mut App, code: KeyCode) {
    if let Some(action) = a.handle_key(key(code)) {
        a.update(action);
    }
}

#[test]
fn jev_is_listed_and_its_picker_only_offers_systemone_subscriptions() {
    let mut a = vm_app(false);
    let out = render(&mut a, 80, 24);
    assert!(vm_list_row(&out, "model-jev").contains('1'), "{out}");
    // 走到 model-jev (第 6 项), 进成员栏, 按 a
    for _ in 0..5 {
        press(&mut a, KeyCode::Down);
    }
    press(&mut a, KeyCode::Enter);
    let out = render(&mut a, 80, 24);
    assert!(out.contains("JEV"), "成员栏要有 JEV 标识\n{out}");
    press(&mut a, KeyCode::Char('a'));
    let out = render(&mut a, 80, 24);
    // 唯一的 systemone 订阅已经在列表里 → 没有可加入的
    assert!(out.contains(ZH.vm_nothing_to_add_jev), "{out}");
}

#[test]
fn opus_picker_never_offers_systemone_subscriptions() {
    let mut a = vm_app(false);
    press(&mut a, KeyCode::Down); // model-opus
    press(&mut a, KeyCode::Enter);
    press(&mut a, KeyCode::Char('a'));
    let out = render(&mut a, 80, 24);
    // 弹窗确实开了 (对话订阅 4 / 5 还没加入 opus), 但里面没有 System One 订阅。
    assert!(out.contains("Gemini 中转"), "{out}");
    assert!(!out.contains("Ollama 决策"), "{out}");
}

#[test]
fn virtual_models_80x24() {
    insta::assert_snapshot!(render(&mut vm_app(false), 80, 24));
}

#[test]
fn virtual_models_120x40() {
    insta::assert_snapshot!(render(&mut vm_app(false), 120, 40));
}

#[test]
fn virtual_models_dirty_80x24() {
    let mut a = vm_app(false);
    a.handle_key(key(KeyCode::Right));
    a.handle_key(key(KeyCode::Char('J')));
    insta::assert_snapshot!(render(&mut a, 80, 24));
}

#[test]
fn lists_five_models_with_mode_and_count() {
    let out = render(&mut vm_app(false), 80, 24);
    for name in ["model-fable", "model-opus", "model-sonnet", "model-haiku", "model-fallback"] {
        assert!(out.contains(name), "缺 {name}\n{out}");
    }
    assert!(out.contains(ZH.vm_mode_seq) && out.contains(ZH.vm_mode_rr) && out.contains(ZH.vm_mode_sticky), "{out}");

    let fable_line = vm_list_row(&out, "model-fable");
    assert!(fable_line.contains('2'), "model-fable 应该显示 2 个订阅\n{fable_line}");
    let haiku_line = vm_list_row(&out, "model-haiku");
    assert!(haiku_line.contains('0'), "model-haiku 应该显示 0 个订阅\n{haiku_line}");
    let fallback_line = vm_list_row(&out, "model-fallback");
    assert!(fallback_line.contains('3'), "model-fallback 应该显示 3 个订阅\n{fallback_line}");
}

#[test]
fn members_show_names_badges_and_missing_ids() {
    // 默认选中 model-fable (ids: ["1", "ghost-legacy-sub-999"])。
    let out = render(&mut vm_app(false), 80, 24);
    let zhipu_line = out.lines().find(|l| l.contains("智谱主号")).unwrap_or_else(|| panic!("{out}"));
    assert!(zhipu_line.contains('●'), "健康订阅应该带对应的 badge 符号\n{zhipu_line}");
    assert!(out.contains(ZH.vm_missing), "缺失的订阅 id 应该标 vm_missing\n{out}");
    assert!(out.contains("ghost-le"), "缺失订阅应该显示 id 前 8 位\n{out}");
}

/// 被删除的成员整行显示「id 前 8 位 + 已删除标记」, 标记在每种语言、两种终端尺寸下都完整显示
/// (没有厂商可显示, 这一行占用名字列 + 厂商列)。
#[test]
fn missing_member_marker_is_shown_in_full_in_every_language() {
    for lang in Lang::ALL {
        use_lang(lang);
        for (w, h) in [(80, 24), (120, 40)] {
            let out = render(&mut vm_app(false), w, h);
            let expected = format!("ghost-le {}", s().vm_missing);
            assert!(out.contains(&expected), "{lang:?} {w}x{h}: 「{expected}」没有完整显示\n{out}");
        }
    }
}

/// I4: 订阅列表还没加载完时 (`list_subscriptions` 还没回来), `store.subscription` 对任何 id 都会
/// 返回 `None`——不该被误判成"已删除" (不显示 `vm_missing`), `a`/`x`/`J`/`K`/`s` 也该统一拒绝
/// (弹 `vm_subs_not_loaded`), 而不是把用户导向"请先移除已删除的订阅"这种具有误导性的提示。订阅
/// 列表到了之后, 才恢复正常的 ghost 判定。
#[test]
fn members_are_not_called_deleted_before_subscriptions_load() {
    let mut a = app(false);
    a.update(Action::Connected { app_version: VERSION.into() });
    a.update(Action::SwitchTab(Tab::VirtualModels));
    a.update(vm_done(1, vm_list())); // 虚拟模型列表到了, 订阅列表还没到

    let out = render(&mut a, 80, 24);
    assert!(!out.contains(ZH.vm_missing), "订阅列表还没加载完时不该显示已删除\n{out}");
    assert!(out.contains("ghost-le"), "应该显示 id 前缀\n{out}");

    a.handle_key(key(KeyCode::Right)); // Members, model-fable ids=["1", ghost]
    for code in [KeyCode::Char('a'), KeyCode::Char('x'), KeyCode::Char('J'), KeyCode::Char('K'), KeyCode::Char('s')] {
        assert_eq!(
            a.handle_key(key(code)),
            Some(Action::Notify { kind: ToastKind::Info, text: ZH.vm_subs_not_loaded.into() }),
            "{code:?} 应该在订阅列表还没加载完时被拒绝"
        );
    }

    // 订阅列表到了之后, 才恢复正常的 ghost 判定。
    a.update(subs_done(1, vm_subs()));
    let out2 = render(&mut a, 80, 24);
    assert!(out2.contains(ZH.vm_missing), "订阅列表到了之后应该正常显示已删除\n{out2}");
}

/// P3b 遗留项 (Task 5): 上面这条用例覆盖的「订阅列表没加载完时 `s` 一律拒绝」只是旧版的整体行为
/// ——真正没有 id 需要核对是否已删除的场景 (成员为空、只改了调度模式的草稿) 不该被这条守卫挡住。
/// `model-haiku` 没有成员, 同样的 `m`/`s` 操作在 `model-fable` (有成员) 上仍然被拒绝, 与旧行为
/// 保持一致。两个场景各起一个全新的 `App`, 避免「先在 model-haiku 上造出草稿, 再切到 model-fable」
/// 这一步本身会被 `move_model_selection` 的脏页面确认拦住, 与本用例想验证的东西无关。
#[test]
fn a_mode_only_save_of_an_empty_virtual_model_is_allowed_before_subscriptions_load() {
    // model-haiku (无成员): 只改模式的草稿应该允许保存。
    let mut haiku_app = app(false);
    haiku_app.update(Action::Connected { app_version: VERSION.into() });
    haiku_app.update(Action::SwitchTab(Tab::VirtualModels));
    haiku_app.update(vm_done(1, vm_list())); // 虚拟模型列表到了, 订阅列表还没到
    render(&mut haiku_app, 80, 24);
    for _ in 0..3 {
        haiku_app.handle_key(key(KeyCode::Down)); // fable -> opus -> sonnet -> haiku (无成员)
    }
    assert_eq!(haiku_app.handle_key(key(KeyCode::Char('m'))), None, "m 只改草稿, 不产出 Action");
    assert_eq!(
        haiku_app.handle_key(key(KeyCode::Char('s'))),
        Some(Action::Mutate(Mutation::UpdateVirtualModel {
            name: "model-haiku".into(),
            mode: RoutingMode::RoundRobin,
            subscription_ids: vec![],
        })),
        "成员为空时, 订阅列表未加载也该允许保存"
    );

    // model-fable (有成员): 同样的操作仍然被拒绝。
    let mut fable_app = app(false);
    fable_app.update(Action::Connected { app_version: VERSION.into() });
    fable_app.update(Action::SwitchTab(Tab::VirtualModels));
    fable_app.update(vm_done(1, vm_list()));
    render(&mut fable_app, 80, 24); // 默认选中 model-fable (ids: ["1", ghost])
    assert_eq!(fable_app.handle_key(key(KeyCode::Char('m'))), None, "m 只改草稿, 不产出 Action");
    assert_eq!(
        fable_app.handle_key(key(KeyCode::Char('s'))),
        Some(Action::Notify { kind: ToastKind::Info, text: ZH.vm_subs_not_loaded.into() }),
        "成员非空时, 订阅列表未加载应该仍然拒绝"
    );
}

#[test]
fn fallback_marks_translated_subscriptions_without_a_fallback_slot() {
    let mut a = vm_app(false);
    for _ in 0..4 {
        a.handle_key(key(KeyCode::Down)); // fable -> opus -> sonnet -> haiku -> fallback
    }
    let out = render(&mut a, 80, 24);

    let relay_line = out.lines().find(|l| l.contains("示例中转")).unwrap_or_else(|| panic!("{out}"));
    assert!(!relay_line.contains(ZH.vm_will_skip), "api_key 类不该标记\n{relay_line}");

    let gemini_line = out.lines().find(|l| l.contains("Gemini 中转")).unwrap_or_else(|| panic!("{out}"));
    assert!(gemini_line.contains(ZH.vm_will_skip), "无兜底槽的翻译类应该标记\n{gemini_line}");

    let gemini_fb_line = out.lines().find(|l| l.contains("Gemini 有兜底")).unwrap_or_else(|| panic!("{out}"));
    assert!(!gemini_fb_line.contains(ZH.vm_will_skip), "配了兜底槽的不该标记\n{gemini_fb_line}");

    // V3 (fix round P3b): 后端 `ModelSlots::fallback_model()` 是 `trim()` 之后判空的, 纯空白的
    // 兜底槽也该被视为未配置——否则会漏标一条实际会被 pipeline 统一跳过守卫拦下的订阅。用独立的
    // fixture (不复用 `vm_subs`/`vm_list`, 避免牵动其它依赖那两个函数固定候选列表的用例)。
    let mut subs = vm_subs();
    // id 用 "7": `vm_subs()` 的 "6" 已经是 System One 订阅。
    let mut gemini_blank = sub("7", "Gemini 空白兜底", SubscriptionState::Healthy);
    gemini_blank.provider_display_name = "Gemini".into();
    gemini_blank.auth_type = "gemini_api_key".into();
    gemini_blank.model_slots.fallback = "  ".into();
    subs.push(gemini_blank);
    let mut vms = vm_list();
    vms.iter_mut().find(|vm| vm.name == "model-fallback").unwrap().subscription_ids.push("7".into());
    let mut b = vm_app_with(false, vms, subs);
    for _ in 0..4 {
        b.handle_key(key(KeyCode::Down));
    }
    let out2 = render(&mut b, 80, 24);
    let blank_line = out2.lines().find(|l| l.contains("Gemini 空白兜底")).unwrap_or_else(|| panic!("{out2}"));
    assert!(blank_line.contains(ZH.vm_will_skip), "纯空白的兜底槽应该视为未配置\n{blank_line}");
}

#[test]
fn focus_moves_between_panes_and_the_border_follows() {
    let theme = Theme::new(ColorMode::TrueColor);
    let mut a = vm_app(false);
    // 内容区顶边在屏幕第 3 行; 左栏左上角 x=0, 右栏左上角 x=LEFT_WIDTH=32。
    let buf = render_buffer(&mut a, 80, 24);
    assert_eq!(buf[(0, 3)].style().fg, Some(theme.accent), "Models 焦点时左栏边框应该是 accent 色");
    assert_eq!(buf[(32, 3)].style().fg, Some(theme.border), "Models 焦点时右栏边框应该是普通 border 色");

    a.handle_key(key(KeyCode::Right)); // 切到 Members 焦点
    let buf2 = render_buffer(&mut a, 80, 24);
    assert_eq!(buf2[(0, 3)].style().fg, Some(theme.border), "Members 焦点时左栏边框应该变回普通 border 色");
    assert_eq!(buf2[(32, 3)].style().fg, Some(theme.accent), "Members 焦点时右栏边框应该是 accent 色");
}

/// Step 4 咬合检查 (a): `J` 在最后一项上应该原地不动而不是尝试越界 swap; `K` 在第一项上同理。
#[test]
fn capital_j_k_reorder_and_clamp() {
    let mut a = vm_app(false);
    a.handle_key(key(KeyCode::Right)); // Members, model-fable ids=["1", ghost]
    let out0 = render(&mut a, 80, 24);
    assert!(out0.find("智谱主号").unwrap() < out0.find(ZH.vm_missing).unwrap(), "初始顺序: 智谱主号在前\n{out0}");

    a.handle_key(key(KeyCode::Char('J'))); // 下移第一项 -> [ghost, 1]
    let out1 = render(&mut a, 80, 24);
    assert!(out1.find(ZH.vm_missing).unwrap() < out1.find("智谱主号").unwrap(), "J 之后缺失项应该排到前面\n{out1}");

    a.handle_key(key(KeyCode::Char('J'))); // 已经在末尾, 不该再变
    let out2 = render(&mut a, 80, 24);
    assert_eq!(out1, out2, "到底不该再交换 (越界保护去掉的话这里会 panic 或错位)");

    a.handle_key(key(KeyCode::Char('K')));
    a.handle_key(key(KeyCode::Char('K'))); // 已经在顶部, 不该再变
    let out3 = render(&mut a, 80, 24);
    assert_eq!(out3, out0, "K 两次移回顶部之后应该恢复原始顺序 (与 Store 相等, 视觉上与初始帧一致)");
}

#[test]
fn a_offers_only_unbound_subscriptions_and_appends() {
    let mut a = vm_app(false);
    a.handle_key(key(KeyCode::Right)); // Members, model-fable ids=["1", ghost]
    let action = a.handle_key(key(KeyCode::Char('a')));
    let Some(Action::OpenPicker(spec)) = action else { panic!("{action:?}") };
    assert_eq!(spec.tag, PickerTag::VmAddSubscription { vm: "model-fable".into() });
    let ids: Vec<&str> = spec.items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ids, vec!["2", "3", "4", "5"], "候选应该排除已经在列表里的 \"1\"\n{ids:?}");

    a.update(Action::OpenPicker(spec));
    let done = Action::PickerDone { tag: PickerTag::VmAddSubscription { vm: "model-fable".into() }, choice: PickerChoice::Item("2".into()) };
    assert!(a.update(done).is_empty(), "PickerDone 不产出 Cmd");
    let out = render(&mut a, 80, 24);
    assert!(out.contains("Kimi 备用"), "新加入的订阅应该出现在成员列表里\n{out}");

    let action2 = a.handle_key(key(KeyCode::Char('a')));
    let Some(Action::OpenPicker(spec2)) = action2 else { panic!("{action2:?}") };
    let ids2: Vec<&str> = spec2.items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ids2, vec!["3", "4", "5"], "刚加入的 \"2\" 不该再出现在候选里\n{ids2:?}");
}

#[test]
fn a_with_nothing_left_toasts() {
    let mut a = vm_app(false);
    a.handle_key(key(KeyCode::Right)); // Members, model-fable ids=["1", ghost]
    for id in ["2", "3", "4", "5"] {
        let action = a.handle_key(key(KeyCode::Char('a')));
        let Some(Action::OpenPicker(spec)) = action else { panic!("{action:?}") };
        a.update(Action::OpenPicker(spec));
        assert!(a
            .update(Action::PickerDone { tag: PickerTag::VmAddSubscription { vm: "model-fable".into() }, choice: PickerChoice::Item(id.into()) })
            .is_empty());
    }
    let action = a.handle_key(key(KeyCode::Char('a')));
    assert_eq!(action, Some(Action::Notify { kind: ToastKind::Info, text: ZH.vm_nothing_to_add.into() }), "所有订阅都绑定后应该就地提示");
}

#[test]
fn x_removes_and_keeps_the_cursor_in_range() {
    let mut a = vm_app(false);
    a.handle_key(key(KeyCode::Down)); // model-opus, ids=["1","2","3"]
    a.handle_key(key(KeyCode::Right)); // Members
    a.handle_key(key(KeyCode::Down));
    a.handle_key(key(KeyCode::Down)); // 光标移到最后一项 "示例中转"
    a.handle_key(key(KeyCode::Char('x')));
    let out = render(&mut a, 80, 24);
    assert!(!out.contains("示例中转"), "移除的订阅不该再出现\n{out}");

    // 光标应该钳制在新的最后一项 ("Kimi 备用") 上, 再删一次应该删掉它而不是越界。
    a.handle_key(key(KeyCode::Char('x')));
    let out2 = render(&mut a, 80, 24);
    assert!(!out2.contains("Kimi 备用"), "{out2}");
    assert!(out2.contains("智谱主号"), "剩下的那条不该被误删\n{out2}");
}

#[test]
fn m_cycles_the_mode_in_either_pane() {
    let mut a = vm_app(false); // model-fable 初始是 Sequential (顺序)
    a.handle_key(key(KeyCode::Char('m'))); // Models 焦点下也可用: 顺序 -> 轮询
    let out1 = render(&mut a, 80, 24);
    let fable_line = vm_list_row(&out1, "model-fable");
    assert!(fable_line.contains(ZH.vm_mode_rr), "第一次 m 应该切到轮询\n{fable_line}");

    a.handle_key(key(KeyCode::Right)); // 进 Members, 草稿还在
    a.handle_key(key(KeyCode::Char('m'))); // 轮询 -> 会话
    let out2 = render(&mut a, 80, 24);
    let fable_line2 = vm_list_row(&out2, "model-fable");
    assert!(fable_line2.contains(ZH.vm_mode_sticky), "Members 焦点下 m 应该继续切到会话\n{fable_line2}");
}

/// I3: `s`/`Esc` 现在 Models 焦点下也可用, 不用先进 Members 才能保存/放弃 (`m` 造草稿早就是这样,
/// 保存/放弃理应对称)。用 model-opus (无 ghost 成员, 避免被 V2 的守卫拦下, 干扰这条用例本身要
/// 验证的东西)。
#[test]
fn mode_change_in_models_focus_can_be_saved_and_discarded_there() {
    let mut a = vm_app(false);
    a.handle_key(key(KeyCode::Down)); // model-opus (初始 round_robin)
    a.handle_key(key(KeyCode::Char('m'))); // round_robin -> sticky, 仍在 Models 焦点
    let action = a.handle_key(key(KeyCode::Char('s')));
    assert_eq!(
        action,
        Some(Action::Mutate(Mutation::UpdateVirtualModel {
            name: "model-opus".into(),
            mode: RoutingMode::Sticky,
            subscription_ids: vec!["1".into(), "2".into(), "3".into()],
        })),
        "Models 焦点下应该也能直接保存, 不用先进 Members, 实际 {action:?}"
    );

    // 另开一局: Models 焦点造草稿, 直接按 Esc 也该弹确认放弃, 与 Members 焦点同一套流程。
    let mut b = vm_app(false);
    b.handle_key(key(KeyCode::Down));
    b.handle_key(key(KeyCode::Char('m')));
    let esc_action = b.handle_key(key(KeyCode::Esc));
    assert_eq!(
        esc_action,
        Some(Action::OpenConfirm { prompt: ZH.confirm_discard.into(), on_yes: OnYes::discard_then(Action::DiscardDraft) }),
        "Models 焦点下 Esc 也该弹确认放弃"
    );
    b.update(esc_action.unwrap());
    assert_eq!(b.handle_key(key(KeyCode::Char('y'))), Some(Action::Confirmed(OnYes::discard_then(Action::DiscardDraft))));
    b.update(Action::Confirmed(OnYes::discard_then(Action::DiscardDraft)));
    let out = render(&mut b, 80, 24);
    assert!(!out.contains(" *"), "确认放弃后应该干净\n{out}");
}

#[test]
fn reverting_every_change_clears_dirty() {
    let mut a = vm_app(false); // model-fable 初始是 Sequential
    a.handle_key(key(KeyCode::Char('m'))); // 顺序 -> 轮询
    a.handle_key(key(KeyCode::Char('m'))); // 轮询 -> 会话
    a.handle_key(key(KeyCode::Char('m'))); // 会话 -> 顺序 (改回原值)
    let out = render(&mut a, 80, 24);
    assert!(!out.contains(" *"), "改回原值应该清脏\n{out}");
}

/// D2(b) (fix round P3b): `x`/`J`/`K` 在空列表上、以及 `K` 在第一项 / `J` 在最后一项上, 都是
/// no-op——旧版 `draft_mut` 会在这些场景里无条件新建一份 (跟 `Store` 完全相等的) 草稿, 表现成
/// 「什么都没做但页面莫名其妙变脏」。
#[test]
fn noop_edits_do_not_create_a_draft() {
    let mut a = vm_app(false);
    for _ in 0..3 {
        a.handle_key(key(KeyCode::Down)); // fable -> opus -> sonnet -> haiku (空列表)
    }
    a.handle_key(key(KeyCode::Right)); // Members, model-haiku ids=[]

    a.handle_key(key(KeyCode::Char('x')));
    a.handle_key(key(KeyCode::Char('J')));
    a.handle_key(key(KeyCode::Char('K')));
    let out = render(&mut a, 80, 24);
    assert!(!out.contains(" *"), "空列表上 x/J/K 都不该产生草稿\n{out}");

    // 不脏时切页不该弹确认, 应该真的切过去 (总览页的「今日」面板标题是静态文案, 与是否已经加载
    // 过总览数据无关, 用它确认真的切走了)。
    a.update(Action::SwitchTab(Tab::Overview));
    let out2 = render(&mut a, 80, 24);
    assert!(!out2.contains(ZH.confirm_discard), "不脏时切页不该弹确认\n{out2}");
    assert!(out2.contains(ZH.ov_today), "应该已经真的切到总览页\n{out2}");

    // 没有草稿时, 别的客户端把这个虚拟模型的成员列表改了——应该正常显示新值, 不带 *。
    let mut a2 = vm_app(false);
    for _ in 0..3 {
        a2.handle_key(key(KeyCode::Down));
    }
    a2.handle_key(key(KeyCode::Right));
    a2.handle_key(key(KeyCode::Char('x')));
    a2.handle_key(key(KeyCode::Char('J')));
    a2.handle_key(key(KeyCode::Char('K')));
    let mut vms2 = vm_list();
    vms2.iter_mut().find(|vm| vm.name == "model-haiku").unwrap().subscription_ids = vec!["1".into()];
    a2.update(vm_done(2, vms2));
    let out3 = render(&mut a2, 80, 24);
    assert!(!out3.contains(" *"), "没有草稿时, 别处的改动不该显示 *\n{out3}");
    assert!(out3.contains("智谱主号"), "应该正常显示 Store 的新值\n{out3}");

    // K 在第一项 / J 在最后一项上也是 no-op (model-opus, ids=["1","2","3"])。
    let mut b = vm_app(false);
    b.handle_key(key(KeyCode::Down)); // model-opus
    b.handle_key(key(KeyCode::Right));
    b.handle_key(key(KeyCode::Char('K'))); // 已经在第一项
    let out4 = render(&mut b, 80, 24);
    assert!(!out4.contains(" *"), "K 在第一项上应该是 no-op\n{out4}");

    b.handle_key(key(KeyCode::Down));
    b.handle_key(key(KeyCode::Down)); // 光标移到最后一项
    b.handle_key(key(KeyCode::Char('J'))); // 已经在最后一项
    let out5 = render(&mut b, 80, 24);
    assert!(!out5.contains(" *"), "J 在最后一项上应该是 no-op\n{out5}");
}

/// V2 (fix round P3b): 草稿里还有 `Store` 找不到的 id (「已删除」的订阅) 时, `s` 应该拒绝并
/// 引导用户先移除, 不能把裸 id 发给后端换回一句英文报错; 移除之后 `s` 应该正常生效。
#[test]
fn saving_with_ghost_members_is_refused_with_guidance() {
    let mut a = vm_app(false); // model-fable ids=["1", "ghost-legacy-sub-999"]
    a.handle_key(key(KeyCode::Right)); // Members
    a.handle_key(key(KeyCode::Char('m'))); // 造一个脏草稿 (只改调度模式, ghost 仍在列表里)

    let action = a.handle_key(key(KeyCode::Char('s')));
    assert_eq!(
        action,
        Some(Action::Notify { kind: ToastKind::Info, text: ZH.vm_remove_ghosts_first.into() }),
        "草稿里还有已删除的订阅时应该拒绝保存, 引导用户先移除\n{action:?}"
    );

    a.handle_key(key(KeyCode::Down)); // 光标移到下标 1 (ghost)
    a.handle_key(key(KeyCode::Char('x'))); // 移除它
    let out = render(&mut a, 80, 24);
    assert!(!out.contains(ZH.vm_missing), "移除后不该再显示 ghost 行\n{out}");

    let action2 = a.handle_key(key(KeyCode::Char('s')));
    assert!(matches!(action2, Some(Action::Mutate(Mutation::UpdateVirtualModel { .. }))), "移除 ghost 后 s 应该正常生效\n{action2:?}");
}

/// V4 (fix round P3b): 成员超过面板高度、光标滚到后面时, 闪烁 rect 必须按渲染之后的真实可视窗口
/// 换算——用未经换算的原始下标算 rect 会闪错行 / 闪到面板外。这里只断言不 panic (`fx_enabled: true`
/// 才会真的把 rect 交给 tachyonfx 处理); 「只有可视行才会闪」由 `member_flash_rect` 的纯函数单测
/// 覆盖几何计算本身。
#[test]
fn flash_rects_stay_inside_the_scrolled_visible_window() {
    let subs: Vec<Subscription> = (1..=30).map(|i| sub(&i.to_string(), &format!("订阅{i}"), SubscriptionState::Healthy)).collect();
    let ids: Vec<String> = subs.iter().map(|s| s.id.clone()).collect();
    let vms = vec![VirtualModel { name: "model-fable".into(), mode: RoutingMode::Sequential, subscription_ids: ids }];
    let mut a = vm_app_with(true, vms.clone(), subs.clone()); // fx_enabled: true, 否则闪烁不会真的进 tachyonfx
    a.handle_key(key(KeyCode::Right)); // Members
    for _ in 0..29 {
        a.handle_key(key(KeyCode::Down)); // 光标滚到最后一项 ("订阅30"), 列表自动滚动
    }

    // 让一条已经滚出视野的 ("订阅1") 和一条仍在视野内的 ("订阅30") 同时变化, 排进 flash_rows。
    let mut changed_subs = subs.clone();
    changed_subs[0].state = SubscriptionState::RateLimited;
    changed_subs[0].is_dispatchable = false;
    changed_subs[29].state = SubscriptionState::RateLimited;
    changed_subs[29].is_dispatchable = false;
    a.update(subs_done(2, changed_subs));

    // 不该 panic (滚动之后 offset 换算错误会把 rect 的 y 算到面板高度以外)。
    render(&mut a, 80, 24);
}

#[test]
fn s_sends_the_draft_in_order() {
    let mut a = vm_app(false);
    // model-opus (ids=["1","2","3"], 无 ghost 成员) 而不是 model-fable——V2 (fix round P3b) 起草稿
    // 里有 `Store` 找不到的 id 会被 `s` 拒绝, model-fable 自带的 ghost 成员会跟这个用例真正想测的
    // 「保存按草稿顺序发送」这件事互相干扰, 见 `saving_with_ghost_members_is_refused_with_guidance`。
    a.handle_key(key(KeyCode::Down)); // model-opus
    a.handle_key(key(KeyCode::Right)); // Members, ids=["1","2","3"]
    a.handle_key(key(KeyCode::Char('J'))); // -> ["2","1","3"]
    let action = a.handle_key(key(KeyCode::Char('s')));
    assert_eq!(
        action,
        Some(Action::Mutate(Mutation::UpdateVirtualModel {
            name: "model-opus".into(),
            mode: RoutingMode::RoundRobin,
            subscription_ids: vec!["2".into(), "1".into(), "3".into()],
        }))
    );
}

#[test]
fn s_refuses_offline_and_ignores_while_busy() {
    let mut a = vm_app(false);
    // model-opus: 同上, 避开 model-fable 自带的 ghost 成员 (与本用例要测的断线/忙碌拒绝无关)。
    a.handle_key(key(KeyCode::Down)); // model-opus
    a.handle_key(key(KeyCode::Right)); // Members
    assert_eq!(a.handle_key(key(KeyCode::Char('s'))), None, "不脏时 s 不该有动作");

    a.handle_key(key(KeyCode::Char('J'))); // 造草稿
    let mutation = Mutation::UpdateVirtualModel {
        name: "model-opus".into(),
        mode: RoutingMode::RoundRobin,
        subscription_ids: vec!["2".into(), "1".into(), "3".into()],
    };
    assert_eq!(a.handle_key(key(KeyCode::Char('s'))), Some(Action::Mutate(mutation.clone())));

    a.update(Action::ConnectionLost);
    assert!(a.update(Action::Mutate(mutation.clone())).is_empty(), "断线时不该真的发");
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.toast_offline), "{out}");

    // 恢复在线, 真的发一次占住忙碌键; 忙碌期间再发一次应该被拒绝。
    a.update(Action::Connected { app_version: VERSION.into() });
    assert_eq!(a.update(Action::Mutate(mutation.clone())), vec![Cmd::Mutate(Box::new(mutation.clone()))]);
    assert!(a.update(Action::Mutate(mutation)).is_empty(), "同一虚拟模型忙碌中应该被忽略");
}

mod virtual_models_saving_in_flight {
    use super::*;

    /// 一次保存发出去之后 (还没等结果回来), 旧版靠 `on_mutation_done` 的"负载与当前草稿完全相等
    /// 才清空"规则保证不丢——I1 改成直接拒绝: 保存飞行中时 `J`/`K`/`a`/`x`/`m`/再按一次 `s` 都被
    /// 拒绝, 弹 `saving_in_progress`, 草稿不动; `MutationDone(Ok)` + refetch 落地后页面变干净;
    /// `MutationDone(Err)` 落地后草稿保留且又能编辑。用 model-opus (无 ghost 成员, 避免和 V2 的
    /// 守卫互相干扰)。
    #[test]
    fn edits_are_refused_while_a_save_is_in_flight() {
        let mut a = vm_app(false);
        a.handle_key(key(KeyCode::Down)); // model-opus, ids=["1","2","3"]
        a.handle_key(key(KeyCode::Right)); // Members
        a.handle_key(key(KeyCode::Char('J'))); // cursor 0 -> swap(0,1): ["2","1","3"], cursor=1
        let action = a.handle_key(key(KeyCode::Char('s')));
        let mutation = match action {
            Some(Action::Mutate(m)) => m,
            other => panic!("{other:?}"),
        };
        assert_eq!(a.update(Action::Mutate(mutation.clone())), vec![Cmd::Mutate(Box::new(mutation.clone()))]);

        // 保存 (["2","1","3"]) 还在飞行中: J/K/a/x/m/再按一次 s 都该被拒绝, 弹同一条提示, 草稿不变。
        // 像真实运行时一样把 `handle_key` 的返回值转发进 `update` (`Action::Notify` 才会真的入队)。
        for code in [KeyCode::Char('J'), KeyCode::Char('K'), KeyCode::Char('x'), KeyCode::Char('m'), KeyCode::Char('s')] {
            let action = a.handle_key(key(code));
            assert_eq!(
                action,
                Some(Action::Notify { kind: ToastKind::Info, text: ZH.saving_in_progress.into() }),
                "{code:?} 应该在保存飞行中被拒绝"
            );
            a.update(action.unwrap());
        }

        let out_while_saving = render(&mut a, 80, 24);
        let pos = |out: &str, needle: &str| out.find(needle).unwrap_or_else(|| panic!("缺 {needle}\n{out}"));
        assert!(
            pos(&out_while_saving, "Kimi 备用") < pos(&out_while_saving, "智谱主号")
                && pos(&out_while_saving, "智谱主号") < pos(&out_while_saving, "示例中转"),
            "飞行中的编辑都不该生效, 顺序应该还是第一次保存时的 [\"2\",\"1\",\"3\"]\n{out_while_saving}"
        );
        assert!(out_while_saving.contains(ZH.saving_in_progress), "{out_while_saving}");

        // 结果回来了 (成功), 后端随后带回来的刷新结果也是第一次保存的顺序。
        a.update(Action::MutationDone { mutation: mutation.clone(), barrier: 0, result: Ok(MutationOutcome::VirtualModelSaved) });
        let mut vms_after_save = vm_list();
        vms_after_save.iter_mut().find(|vm| vm.name == "model-opus").unwrap().subscription_ids = vec!["2".into(), "1".into(), "3".into()];
        a.update(vm_done(2, vms_after_save));
        let out_ok = render(&mut a, 80, 24);
        assert!(!out_ok.contains(" *"), "保存成功后应该变干净\n{out_ok}");

        // 另起一局: 保存失败之后草稿应该原样保留, 而且又能正常编辑了 (saving 标记被摘掉)。
        let mut b = vm_app(false);
        b.handle_key(key(KeyCode::Down)); // model-opus
        b.handle_key(key(KeyCode::Right));
        b.handle_key(key(KeyCode::Char('J')));
        let mutation_b = match b.handle_key(key(KeyCode::Char('s'))) {
            Some(Action::Mutate(m)) => m,
            other => panic!("{other:?}"),
        };
        b.update(Action::Mutate(mutation_b.clone()));
        b.update(Action::MutationDone { mutation: mutation_b, barrier: 0, result: Err("网络错误".into()) });
        // 失败会弹一条提到 "model-opus" 的 toast, 80 列下贴右边缘正好盖住 model-fable 那一行的右半边
        // (第一个虚拟模型, 排在 model-opus 前面)——toast 文本本身包含 "model-opus" 这个子串, 会让
        // `vm_list_row` 的朴素匹配误认成 model-opus 自己那一行。先画一帧让 toast 记住 `shown_at`,
        // 再把时间推到它的生命周期之后, 干净地验证草稿状态, 不用跟 toast 抢地盘。
        render(&mut b, 80, 24);
        b.update(Action::Tick { now_ms: NOW + 4_000 });
        let out_err = render(&mut b, 80, 24);
        assert!(vm_list_row(&out_err, "model-opus").contains(" *"), "失败应该保留草稿\n{out_err}");
        // 光标停在下标 1 (["2","1","3"] 的第二项); 再按一次 J (swap(1,2): ["2","3","1"]) 而不是 K
        // (swap(0,1) 会换回 ["1","2","3"], 正好等于 base, 反而被 `Draft::edit` 判定为"改回原值"清空
        // 草稿——不是这条用例想验证的东西, 只是想证明"失败之后又能正常编辑了")。
        let action_after_err = b.handle_key(key(KeyCode::Char('J')));
        assert_eq!(action_after_err, None, "J 应该正常执行 (不再被拒绝), 无返回值是这个键本身的正常语义");
        let out_after_err_edit = render(&mut b, 80, 24);
        assert!(vm_list_row(&out_after_err_edit, "model-opus").contains(" *"), "失败之后应该又能正常编辑\n{out_after_err_edit}");
    }
}

#[test]
fn draft_survives_polling_and_clears_on_successful_save_only() {
    let mut a = vm_app(false);
    a.handle_key(key(KeyCode::Right));
    a.handle_key(key(KeyCode::Char('J')));
    let dirty_out = render(&mut a, 80, 24);
    assert!(dirty_out.contains(" *"), "{dirty_out}");

    a.update(vm_done(2, vm_list()));
    a.update(subs_done(2, vm_subs()));
    let out_after_polls = render(&mut a, 80, 24);
    assert!(out_after_polls.contains(" *"), "轮询不该冲掉草稿\n{out_after_polls}");

    let mutation = Mutation::UpdateVirtualModel {
        name: "model-fable".into(),
        mode: RoutingMode::Sequential,
        subscription_ids: vec!["ghost-legacy-sub-999".into(), "1".into()],
    };
    a.update(Action::Mutate(mutation.clone()));
    a.update(Action::MutationDone { mutation: mutation.clone(), barrier: 0, result: Err("网络错误".into()) });
    // V5 (fix round P3b): 失败会弹一条 toast, 80 列下它贴右边缘, 会盖住右栏标题上那颗 `*`——不用
    // 再等 toast 过期才能干净地看到, 现在左栏这个虚拟模型自己的列表行也带 `*` (toast 不会盖住
    // 最左边这一栏), 直接查它就够了 (不再需要 `render` + `Tick` 的过期工作区)。
    let out_after_fail = render(&mut a, 80, 24);
    let fable_line = vm_list_row(&out_after_fail, "model-fable");
    assert!(fable_line.contains(" *"), "失败应该保留草稿, 左栏列表行应该显示 *\n{fable_line}");

    a.update(Action::Mutate(mutation.clone()));
    a.update(Action::MutationDone { mutation, barrier: 0, result: Ok(MutationOutcome::VirtualModelSaved) });
    let out_after_ok = render(&mut a, 80, 24);
    assert!(!out_after_ok.contains(" *"), "成功后草稿应该被清掉\n{out_after_ok}");
}

/// M1: `Store` 刚接受一份新的虚拟模型列表这一刻 (`Ok(FetchData::VirtualModels(..))` 分支只更新
/// `Store`, 不会触发这个页面的 `Component::update()`) 就该立刻核对一遍草稿是否已经和它相等——不用
/// 等下一次真正的 `update()` 调用 (最多要等 5 秒的轮询)。只喂 `vm_done`, 不额外调
/// `Action::Refresh`: 如果 `on_store_changed` 没有立刻核对, 这里应该还看得到 `*`。
#[test]
fn vm_list_equal_to_the_draft_clears_dirty_immediately_on_arrival() {
    let mut a = vm_app(false);
    a.handle_key(key(KeyCode::Right)); // Members, model-fable ids=["1", ghost]
    a.handle_key(key(KeyCode::Char('J'))); // -> ["ghost", "1"]
    assert!(render(&mut a, 80, 24).contains(" *"));

    let mut vms = vm_list();
    vms[0].subscription_ids = vec!["ghost-legacy-sub-999".into(), "1".into()];
    a.update(vm_done(2, vms));
    let out = render(&mut a, 80, 24);
    assert!(!out.contains(" *"), "新列表与草稿相等时应该立刻变干净, 不用等下一次真正的 update()\n{out}");

    // 干净之后切页不该弹确认, 应该真的切过去。
    a.update(Action::SwitchTab(Tab::Overview));
    let out2 = render(&mut a, 80, 24);
    assert!(!out2.contains(ZH.confirm_discard), "不脏时切页不该弹确认\n{out2}");
    assert!(out2.contains(ZH.ov_today), "应该已经真的切到总览页\n{out2}");
}

#[test]
fn changing_the_selected_model_with_a_draft_asks_first() {
    let mut a = vm_app(false);
    a.handle_key(key(KeyCode::Char('m'))); // Models 焦点造草稿 (fable: 顺序 -> 轮询)
    let action = a.handle_key(key(KeyCode::Down));
    assert_eq!(action, Some(Action::OpenConfirm { prompt: ZH.confirm_discard.into(), on_yes: OnYes::discard_then(Action::DiscardDraft) }));
    assert!(a.update(action.unwrap()).is_empty());
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.confirm_discard), "{out}");

    assert_eq!(a.handle_key(key(KeyCode::Char('y'))), Some(Action::Confirmed(OnYes::discard_then(Action::DiscardDraft))));
    a.update(Action::Confirmed(OnYes::discard_then(Action::DiscardDraft)));
    let out2 = render(&mut a, 80, 24);
    assert!(!out2.contains(" *"), "{out2}");
    let fable_line = vm_list_row(&out2, "model-fable");
    assert!(fable_line.contains(ZH.vm_mode_seq), "确认后应该停在原位 (草稿被丢弃, 选中项没变)\n{fable_line}");
}

#[test]
fn esc_in_members_with_a_draft_asks_and_yes_returns_to_models() {
    let theme = Theme::new(ColorMode::TrueColor);
    let mut a = vm_app(false);
    a.handle_key(key(KeyCode::Right));
    a.handle_key(key(KeyCode::Char('J')));
    let action = a.handle_key(key(KeyCode::Esc));
    assert_eq!(action, Some(Action::OpenConfirm { prompt: ZH.confirm_discard.into(), on_yes: OnYes::discard_then(Action::DiscardDraft) }));
    assert!(a.update(action.unwrap()).is_empty());
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.confirm_discard), "{out}");

    assert_eq!(a.handle_key(key(KeyCode::Char('y'))), Some(Action::Confirmed(OnYes::discard_then(Action::DiscardDraft))));
    a.update(Action::Confirmed(OnYes::discard_then(Action::DiscardDraft)));
    let out2 = render(&mut a, 80, 24);
    assert!(!out2.contains(" *"), "{out2}");
    let buf = render_buffer(&mut a, 80, 24);
    assert_eq!(buf[(0, 3)].style().fg, Some(theme.accent), "确认放弃后焦点应该回到 Models (左栏)");
}

#[test]
fn leaving_the_tab_with_a_draft_asks() {
    let mut a = vm_app(false);
    a.handle_key(key(KeyCode::Right));
    a.handle_key(key(KeyCode::Char('J')));
    assert!(a.update(Action::SwitchTab(Tab::Overview)).is_empty(), "有草稿时切页应该先确认");
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.confirm_discard), "{out}");
}

#[test]
fn entering_the_tab_fetches_models_and_subscriptions() {
    let mut a = loaded(false);
    let cmds = a.update(Action::SwitchTab(Tab::VirtualModels));
    assert_eq!(cmds, vec![Cmd::Fetch(Fetch::VirtualModels), Cmd::Fetch(Fetch::Subscriptions)], "进这一页应该同时补拉两份数据");

    // 停在这一页时重连也该照样补一次。
    let cmds2 = a.update(Action::Connected { app_version: VERSION.into() });
    assert_eq!(cmds2, vec![Cmd::Fetch(Fetch::VirtualModels), Cmd::Fetch(Fetch::Subscriptions)]);
}

#[test]
fn polling_on_this_tab_fetches_both() {
    let mut a = loaded(false);
    a.update(Action::SwitchTab(Tab::VirtualModels));
    let cmds: Vec<Cmd> = (1..=20).flat_map(|i| a.update(Action::Tick { now_ms: NOW + i * 250 })).collect();
    assert_eq!(cmds, vec![Cmd::Fetch(Fetch::VirtualModels), Cmd::Fetch(Fetch::Subscriptions)], "5 秒后应该轮询一次, 两条 Fetch 都要");
}

/// 草稿的调度模式是 `RoutingMode::Unknown` (后端某天加的新模式, 这版 TUI 不认得) 时, `s` 必须拒绝,
/// 不能静默保存——`RoutingMode::Unknown.as_wire()` 会降级成 `"sequential"`, 静默发出去等于替用户
/// 悄悄改了调度模式。`m` 应该把它挪到 `Sequential`, 之后才允许保存。
#[test]
fn unknown_mode_is_never_saved_silently() {
    let mut vms = vm_list();
    vms[0].mode = RoutingMode::Unknown; // model-fable
    let mut a = vm_app_with(false, vms, vm_subs());

    a.handle_key(key(KeyCode::Right)); // Members
    a.handle_key(key(KeyCode::Char('J'))); // 造草稿 (mode 仍是 Unknown, 只是重排了订阅)
    let action = a.handle_key(key(KeyCode::Char('s')));
    assert_eq!(
        action,
        Some(Action::Notify { kind: ToastKind::Info, text: ZH.vm_unknown_mode.into() }),
        "Unknown 模式不该被静默保存"
    );

    a.handle_key(key(KeyCode::Char('m'))); // Unknown -> Sequential

    // V2 (fix round P3b): model-fable 自带一个 ghost 成员 (`Store` 里找不到的 id), 光是模式挪到
    // 已知值还不够保存——先把它挪走 (`J` 已经把它换到下标 0), 否则会被 V2 的守卫拦下 (那不是这个
    // 用例要验证的东西, 专门的守卫行为见 `saving_with_ghost_members_is_refused_with_guidance`)。
    a.handle_key(key(KeyCode::Up)); // 光标回到下标 0 (ghost)
    a.handle_key(key(KeyCode::Char('x'))); // 移除它

    let action2 = a.handle_key(key(KeyCode::Char('s')));
    assert!(
        matches!(action2, Some(Action::Mutate(Mutation::UpdateVirtualModel { mode: RoutingMode::Sequential, .. }))),
        "挪到已知模式并清掉 ghost 成员之后应该允许保存, 实际 {action2:?}"
    );
}

// ---------- V1(b): 脏页面 80 列下 `s 保存` 不该被裁掉 (两个页面各一份, fix round P3b) ----------
//
// 两边字面上是同一个测试名 (`save_hint_survives_at_80_columns_when_dirty`), 分别按页面套一层
// module 让 Rust 允许重名——这份重复正是 spec 要求的「两个页面都要有一条同名测试」。

mod subscriptions_save_hint {
    use super::*;

    /// 详情态脏的时候, `hints()` 把 `s 保存` 排到 `↑↓ 选择` 右边第一个 (在 `⏎ 改模型` / `o 改档位`
    /// 前面)——keybar 放不下时从右往左丢, 这样 `s` 是最后才会被裁掉的那批, 不脏时它退回原位
    /// (跟在 `o` 后面)。
    #[test]
    fn save_hint_survives_at_80_columns_when_dirty() {
        let mut a = subs_app(false);
        render(&mut a, 80, 24);
        a.handle_key(key(KeyCode::Enter));
        a.handle_key(key(KeyCode::Enter));
        a.update(Action::PickerDone { tag: PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable }, choice: PickerChoice::Item("m3".into()) });
        let out = render(&mut a, 80, 24);
        let footer = out.lines().last().unwrap_or_else(|| panic!("{out}"));
        assert!(footer.contains(ZH.key_save), "脏页面 80 列下 s 保存不该被丢\n{footer}");
    }
}

mod virtual_models_save_hint {
    use super::*;

    /// 同上, 虚拟模型页的 Members 焦点。旧版把 `s` 排在 `hints()` 最后一个, 80 列下反而是最先被
    /// 丢掉的那个 (`m 模式` 排它前面, 会先留下)——这正是简报里描述的观测到的 bug。
    #[test]
    fn save_hint_survives_at_80_columns_when_dirty() {
        let mut a = vm_app(false);
        a.handle_key(key(KeyCode::Right));
        a.handle_key(key(KeyCode::Char('J')));
        let out = render(&mut a, 80, 24);
        let footer = out.lines().last().unwrap_or_else(|| panic!("{out}"));
        assert!(footer.contains(ZH.key_save), "脏页面 80 列下 s 保存不该被丢\n{footer}");
    }
}

// ---------- 实时路由页 (Task 7) ----------

fn route_started_data(vm: &str, sub: &str) -> String {
    format!(r#"{{"subscription_id":"{sub}","virtual_model":"{vm}"}}"#)
}

fn route_finished_data(vm: &str, sub: &str, ok: bool) -> String {
    format!(r#"{{"subscription_id":"{sub}","virtual_model":"{vm}","success":{ok}}}"#)
}

fn sse_started(vm: &str, sub: &str, at_ms: i64) -> Action {
    Action::Sse { name: ROUTE_ATTEMPT_STARTED.into(), data: route_started_data(vm, sub), at_ms }
}

fn sse_finished(vm: &str, sub: &str, ok: bool, at_ms: i64) -> Action {
    Action::Sse { name: ROUTE_ATTEMPT_FINISHED.into(), data: route_finished_data(vm, sub, ok), at_ms }
}

/// 连上 + 切到实时路由页 + 喂三条订阅 (与 `data()`/`detail_subs()` 同一批名字, 好让快照读起来
/// 眼熟): "1" 智谱主号 / "2" Kimi 备用 / "3" 示例中转。
fn live_app(fx_enabled: bool) -> App {
    let mut a = app(fx_enabled);
    a.update(Action::Connected { app_version: VERSION.into() });
    a.update(Action::SwitchTab(Tab::Live));
    a.update(subs_done(
        1,
        vec![
            sub("1", "智谱主号", SubscriptionState::Healthy),
            sub("2", "Kimi 备用", SubscriptionState::RateLimited),
            sub("3", "示例中转", SubscriptionState::AuthFailed),
        ],
    ));
    a
}

/// 快照用的完整状态: 成功 / 失败 / 进行中 / 只有 finished / 被断线中断 / 一条断线分隔行, 时间戳
/// 都相对 `NOW` 固定。先 `Tick { now_ms: NOW − 35_000 }` 再 `ConnectionLost`, 让分隔行落在中间;
/// 最后 `Tick { now_ms: NOW }`。
fn live_fixture_app(fx_enabled: bool) -> App {
    let mut a = live_app(fx_enabled);
    a.update(sse_started("model-sonnet", "1", NOW - 50_000));
    a.update(sse_finished("model-sonnet", "1", true, NOW - 48_200)); // 成功, 1.8s
    a.update(sse_started("model-opus", "2", NOW - 40_000));
    a.update(sse_finished("model-opus", "2", false, NOW - 39_600)); // 失败, 0.4s
    a.update(sse_started("model-haiku", "3", NOW - 36_000)); // 待断线中断
    a.update(Action::Tick { now_ms: NOW - 35_000 });
    a.update(Action::ConnectionLost); // 中断上面那条 pending, 插入一条 Gap (at_ms = NOW - 35_000)
    a.update(sse_finished("model-fable", "1", true, NOW - 20_000)); // 只有 finished, 耗时未知
    a.update(sse_started("model-opus", "3", NOW - 10_000)); // 进行中
    a.update(Action::Tick { now_ms: NOW });
    a
}

#[test]
fn live_80x24() {
    insta::assert_snapshot!(render(&mut live_fixture_app(false), 80, 24));
}

#[test]
fn live_120x40() {
    insta::assert_snapshot!(render(&mut live_fixture_app(false), 120, 40));
}

#[test]
fn live_paused_80x24() {
    let mut a = live_app(false);
    a.update(sse_started("model-sonnet", "1", NOW - 5_000));
    a.update(sse_finished("model-sonnet", "1", true, NOW - 3_000));
    a.handle_key(key(KeyCode::Char(' '))); // 暂停
    a.update(sse_started("model-opus", "2", NOW - 1_000)); // 暂停后又来了新事件
    a.update(Action::Tick { now_ms: NOW });
    insta::assert_snapshot!(render(&mut a, 80, 24));
}

/// Task 2 review 遗留的测试缺口 (a): 重连期间 `Lost` 会反复到来, 断线分隔行必须只出现一条——
/// 走 `App` (不是直接调页面), 与真实运行时的路径一致。
#[test]
fn connection_lost_twice_only_adds_one_gap_row() {
    let mut a = live_app(false);
    a.update(sse_started("model-sonnet", "1", NOW - 5_000));
    a.update(Action::ConnectionLost);
    a.update(Action::ConnectionLost);
    a.update(Action::Tick { now_ms: NOW });
    let out = render(&mut a, 80, 24);
    let gap_rows = out.lines().filter(|l| l.contains(ZH.live_gap)).count();
    assert_eq!(gap_rows, 1, "两次 ConnectionLost 只该出现一条断线分隔行\n{out}");
}

/// Task 2 review 遗留的测试缺口 (b): 耗时来自 `Sse.at_ms`, 不是 `Tick` 推进的 `now_ms`。
#[test]
fn elapsed_time_comes_from_the_sse_at_ms_values() {
    let mut a = live_app(false);
    a.update(sse_started("model-sonnet", "1", NOW - 50_000));
    a.update(sse_finished("model-sonnet", "1", true, NOW - 48_200));
    a.update(Action::Tick { now_ms: NOW });
    let out = render(&mut a, 80, 24);
    assert!(out.contains("1.8s"), "耗时应该是 finished.at_ms - started.at_ms = 1800ms = 1.8s\n{out}");
}

/// 实时路由页不可见时也要记事件 (走 `on_event` 广播), 切回来能看到之前发生的尝试。
#[test]
fn hidden_live_page_still_records_events() {
    // 准备: 新建的 App 默认停在总览页 (`Tab::Overview`), 实时路由页此刻不可见, 但仍要收事件广播。
    let mut a = app(false);
    a.update(Action::Connected { app_version: VERSION.into() });
    a.update(subs_done(1, vec![sub("1", "智谱主号", SubscriptionState::Healthy)]));
    a.update(sse_started("model-sonnet", "1", NOW - 5_000));
    a.update(sse_finished("model-sonnet", "1", true, NOW - 3_000));
    a.update(Action::SwitchTab(Tab::Live));
    a.update(Action::Tick { now_ms: NOW });
    let out = render(&mut a, 80, 24);
    assert!(out.contains("→ 智谱主号"), "{out}");
    assert!(out.contains('✓'), "{out}");
}

#[test]
fn live_filter_picker_lists_virtual_models_and_subscriptions() {
    let mut a = live_app(false);
    a.update(vm_done(1, vm_list()));
    let action = a.handle_key(key(KeyCode::Char('/')));
    assert!(matches!(action, Some(Action::OpenPicker(_))), "{action:?}");
    a.update(action.unwrap());
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.live_filter_title), "{out}");
    assert!(out.contains(ZH.live_filter_all), "{out}");
    for name in ["model-fable", "model-opus", "model-sonnet", "model-haiku", "model-fallback"] {
        assert!(out.contains(name), "缺虚拟模型 {name}\n{out}");
    }
    assert!(out.contains("智谱主号"), "{out}");
    assert!(out.contains("Kimi 备用"), "{out}");
}

/// Task 8: 实时路由页 `⏎` 应该产出 `Action::OpenLogsFor`, `App` 收到后跳到日志页并发出带着这条
/// 订阅过滤的查询。
#[test]
fn enter_on_the_live_page_opens_logs_for_that_subscription() {
    let mut a = live_app(false);
    a.update(sse_started("model-sonnet", "1", NOW - 5_000));
    a.update(sse_finished("model-sonnet", "1", true, NOW - 3_000));
    a.handle_key(key(KeyCode::Up)); // 选中这一条 (订阅 "1")
    let action = a.handle_key(key(KeyCode::Enter));
    assert_eq!(action, Some(Action::OpenLogsFor { subscription_id: "1".into() }), "{action:?}");

    let cmds = a.update(action.unwrap());
    assert_eq!(
        cmds,
        vec![Cmd::Fetch(Fetch::Requests(RequestQuery {
            page: 1,
            filters: RequestFilters { subscription_id: Some("1".into()), virtual_model_name: None, status: None },
        }))],
        "应该带着这条订阅的过滤条件重新发起第 1 页的查询"
    );
    let out = render(&mut a, 80, 24);
    assert!(out.contains(ZH.lg_title), "应该已经切到日志页\n{out}");
}

// ---------- 请求日志页 (Task 8) ----------

fn base_log(id: &str, sub_id: &str, vm: &str, status: RequestStatus, ts: i64) -> RequestLog {
    RequestLog {
        id: id.into(),
        timestamp: ts,
        virtual_model_name: vm.into(),
        subscription_id: sub_id.into(),
        provider_id: "zhipu".into(),
        endpoint_id: "default".into(),
        real_model_name: "glm-4.6".into(),
        response_model_name: None,
        is_streaming: true,
        status,
        http_status: None,
        total_latency_ms: None,
        input_tokens: None,
        output_tokens: None,
        cache_creation_tokens: None,
        cache_read_tokens: None,
        error_message: None,
        upstream_response_body: None,
        client_tool: None,
        client_user_agent: None,
        client_version: None,
        client_ip: None,
        entry_kind: None,
        downstream_http_version: None,
        client_effort: None,
        effective_effort: None,
        effort_source: None,
        upstream_effort: None,
        stop_reason: None,
        tools_offered_count: None,
        tool_result_count: None,
        tool_use_count: None,
        tool_use_names: None,
    }
}

/// 快照/交互测试共用的四行: 成功 (字段填满, 供宽屏客户端列 / 详情弹窗滚动测试用) / 失败 (带错误
/// 信息) / 超时 (字段稀疏, 模型名足够长以在窄屏 Fill 列里截断) / 非今天 (跨天时间戳, 覆盖
/// `short_stamp` 的日期分支)。四条都关联 `logs_app()` 里的三条假订阅。
fn logs_fixture_rows() -> Vec<RequestLog> {
    let mut success = base_log("r1", "1", "model-sonnet", RequestStatus::Success, NOW - 400_000);
    success.http_status = Some(200);
    success.total_latency_ms = Some(1_800);
    success.input_tokens = Some(12_300);
    success.output_tokens = Some(3_400);
    success.client_tool = Some("claude-code".into());
    success.client_version = Some("1.2.3".into());
    success.client_ip = Some("127.0.0.1".into());
    success.client_user_agent = Some("cc/1.2.3".into());
    success.entry_kind = Some("messages".into());
    success.downstream_http_version = Some("HTTP/2".into());
    success.client_effort = Some("high".into());
    success.effective_effort = Some("high".into());
    success.effort_source = Some("slot".into());
    success.upstream_effort = Some("high".into());
    success.stop_reason = Some("end_turn".into());
    success.tools_offered_count = Some(2);
    success.tool_result_count = Some(1);
    success.tool_use_count = Some(2);
    success.tool_use_names = Some(r#"["Read","Bash"]"#.into());
    success.upstream_response_body = Some(r#"{"id":"msg_1","usage":{"input_tokens":12300,"output_tokens":3400}}"#.into());

    let mut error = base_log("r2", "2", "model-opus", RequestStatus::Error, NOW - 600_000);
    error.http_status = Some(429);
    error.total_latency_ms = Some(400);
    error.error_message = Some("上游返回 429 Too Many Requests, 已达到本分钟请求数上限".into());
    error.real_model_name = "kimi-k2".into();

    let mut timeout = base_log("r3", "3", "model-haiku", RequestStatus::Timeout, NOW - 900_000);
    timeout.real_model_name = "claude-3-5-sonnet-20250219".into();

    let mut yesterday = base_log("r4", "1", "model-sonnet", RequestStatus::Success, NOW - 7 * 3_600_000);
    yesterday.http_status = Some(200);
    yesterday.total_latency_ms = Some(2_000);
    yesterday.input_tokens = Some(500);
    yesterday.output_tokens = Some(200);

    vec![success, error, timeout, yesterday]
}

fn requests_done(issued: u64, query: RequestQuery, items: Vec<RequestLog>, total: i64) -> Action {
    Action::FetchDone { fetch: Fetch::Requests(query), issued, result: Ok(FetchData::Requests(RequestPage { items, total })) }
}

fn requests_failed(query: RequestQuery, issued: u64, message: &str) -> Action {
    Action::FetchDone { fetch: Fetch::Requests(query), issued, result: Err(message.into()) }
}

/// 连上 + 切到日志页 + 喂三条订阅 (与 `live_app`/`subs_app` 同一批名字)。不自带任何请求数据——
/// 测试各自决定要不要喂一份 `requests_done`。
fn logs_app(fx_enabled: bool) -> App {
    let mut a = app(fx_enabled);
    a.update(Action::Connected { app_version: VERSION.into() });
    a.update(Action::SwitchTab(Tab::Logs));
    a.update(subs_done(
        1,
        vec![
            sub("1", "智谱主号", SubscriptionState::Healthy),
            sub("2", "Kimi 备用", SubscriptionState::RateLimited),
            sub("3", "示例中转", SubscriptionState::AuthFailed),
        ],
    ));
    a
}

#[test]
fn logs_80x24() {
    let mut a = logs_app(false);
    a.update(requests_done(1, RequestQuery::default(), logs_fixture_rows(), 4));
    insta::assert_snapshot!(render(&mut a, 80, 24));
}

#[test]
fn logs_120x40() {
    let mut a = logs_app(false);
    a.update(requests_done(1, RequestQuery::default(), logs_fixture_rows(), 4));
    insta::assert_snapshot!(render(&mut a, 120, 40));
}

#[test]
fn logs_detail_80x24() {
    let mut a = logs_app(false);
    a.update(requests_done(1, RequestQuery::default(), logs_fixture_rows(), 4));
    a.handle_key(key(KeyCode::Down)); // 选中「失败」那一行
    let action = a.handle_key(key(KeyCode::Enter)).expect("⏎ 应该产出 Action::OpenDetail");
    a.update(action);
    insta::assert_snapshot!(render(&mut a, 80, 24));
}

#[test]
fn logs_filtered_80x24() {
    let mut a = logs_app(false);
    // 走真实的 PickerDone 落地流程 (与其它页面的既有测试同一套写法, 不直接开真弹窗): 先按订阅
    // 过滤, 再按状态过滤——两次都应该把页码重置回 1。
    a.update(Action::PickerDone { tag: PickerTag::LogsFilter, choice: PickerChoice::Item("sub:1".into()) });
    a.update(Action::PickerDone { tag: PickerTag::LogsFilter, choice: PickerChoice::Item("status:error".into()) });
    let query = RequestQuery {
        page: 1,
        filters: RequestFilters { subscription_id: Some("1".into()), virtual_model_name: None, status: Some(RequestStatus::Error) },
    };
    a.update(requests_done(1, query, vec![], 0));
    insta::assert_snapshot!(render(&mut a, 80, 24));
}

/// 结果到达时日志页不可见, 切回来应该立刻能看到 (不用等新的一次加载)。
#[test]
fn request_results_reach_the_logs_page_while_it_is_hidden() {
    let mut a = logs_app(false);
    a.update(Action::SwitchTab(Tab::Overview)); // 切走, 日志页此刻不可见
    a.update(requests_done(1, RequestQuery::default(), logs_fixture_rows(), 4));
    a.update(Action::SwitchTab(Tab::Logs)); // 切回来
    let out = render(&mut a, 80, 24);
    assert!(out.contains("智谱主号"), "{out}");
}

/// Task 8 review item #3: `FetchDone(Requests)` 的 `Err` 路径以前从没在 `App` 这一层测过——加载
/// 失败要照常弹出错误 toast、且不产出任何 `Cmd`。
///
/// Review fix round 1: 「data 为 None 时一律显示加载中, 不看 `loading`」(与 `Subscriptions::
/// draw_placeholder` 同一套约定, 见 `logs.rs::draw_table`) 落地之后, 这条路径在渲染文本层面已经
/// 看不出「加载指示真的停了」这件事——数据从没落地过时应该继续显示 `s.loading`, 不能因为一次
/// 失败就误报成「没有记录」(断线 / 重连期间 `loading` 本来就一直是 false)。所以这里只保留还能靠
/// 渲染文本断言的两件事 (toast / 无 Cmd), 「加载指示真的停了」这件事挪到 `app.rs::mod tests` 用
/// `Logs::is_loading()` 这个测试专用钩子验证 (`a_failed_requests_fetch_clears_the_logs_pages_loading_flag`)。
#[test]
fn a_failed_requests_fetch_toasts_and_returns_no_cmd() {
    let mut a = logs_app(false);
    let cmds = a.update(requests_failed(RequestQuery::default(), 1, "网络错误"));
    assert!(cmds.is_empty(), "失败的加载不该产出任何 Cmd");

    let out = render(&mut a, 80, 24);
    assert!(out.contains(&(ZH.toast_load_failed)("网络错误")), "应该弹出错误 toast\n{out}");
    assert!(out.contains(ZH.loading), "还没有任何结果落地过, 应该继续显示加载中, 不能误报「没有记录」\n{out}");
}

// ---------- 英文界面 ----------

#[test]
fn en_overview_80x24() {
    use_lang(Lang::En);
    insta::assert_snapshot!(render(&mut loaded(false), 80, 24));
}

#[test]
fn en_subscriptions_120x40() {
    use_lang(Lang::En);
    insta::assert_snapshot!(render(&mut subs_app(false), 120, 40));
}

#[test]
fn en_wizard_custom_80x24() {
    use_lang(Lang::En);
    insta::assert_snapshot!(render(&mut custom_wizard_probed_and_filled(), 80, 24));
}

#[test]
fn en_delete_confirm_80x24() {
    use_lang(Lang::En);
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    let action = a.handle_key(key(KeyCode::Char('d'))).expect("应该产出确认弹窗");
    a.update(action);
    insta::assert_snapshot!(render(&mut a, 80, 24));
}

/// 订阅页的帮助是所有页面里最长的一份 (刚好顶满 24 行)。
#[test]
fn en_help_subscriptions_80x24() {
    use_lang(Lang::En);
    let mut a = subs_app(false);
    a.update(Action::ToggleHelp);
    insta::assert_snapshot!(render(&mut a, 80, 24));
}

#[test]
fn en_logs_120x40() {
    use_lang(Lang::En);
    let mut a = logs_app(false);
    a.update(requests_done(1, RequestQuery::default(), logs_fixture_rows(), 4));
    insta::assert_snapshot!(render(&mut a, 120, 40));
}

#[test]
fn en_live_80x24() {
    use_lang(Lang::En);
    insta::assert_snapshot!(render(&mut live_fixture_app(false), 80, 24));
}

#[test]
fn en_wizard_slots_80x24() {
    use_lang(Lang::En);
    let mut a = wizard_at_slots(vec![ModelInfo { id: "glm-4.6".into(), display_name: None }]);
    insta::assert_snapshot!(render(&mut a, 80, 24));
}

/// 总览顶部「认证 · 订阅数 · 可用数」一行与 logo 同一行, 在每种语言的 80 列上完整显示。
#[test]
fn overview_status_line_is_shown_in_full_at_80x24_in_every_language() {
    for lang in Lang::ALL {
        use_lang(lang);
        let s = s();
        let out = render(&mut loaded(false), 80, 24);
        let line = format!("{} · {}", s.ov_auth_on, (s.ov_subs_summary)(4, 1));
        assert!(out.contains(&line), "{lang:?}: 「{line}」没有完整显示\n{out}");
    }
}

/// 周期名最宽的是英文「Lifetime total」: 设了累计上限时它在总览与订阅详情里都完整显示; 周期名一列
/// 只按实际出现的周期量宽, 没有累计上限的画面不为它让出进度条 (见 `en_overview_80x24` 快照)。
#[test]
fn lifetime_total_quota_label_is_shown_in_full_in_every_language() {
    for lang in Lang::ALL {
        use_lang(lang);
        let s = s();
        let mut subs = detail_subs();
        subs[0].quota_usage = vec![quota_period(QuotaPeriod::Total, 1000, 620)];
        let mut overview = data();
        overview.subscriptions[1].quota_usage = vec![quota_period(QuotaPeriod::Total, 100, 62)];
        let mut a = app(false);
        a.update(Action::Connected { app_version: VERSION.into() });
        a.update(overview_done(1, overview));
        let out = render(&mut a, 80, 24);
        assert!(out.contains(&format!("{} ", s.q_total)), "{lang:?}: 总览里「{}」没有完整显示\n{out}", s.q_total);

        let mut a = app(false);
        a.update(Action::Connected { app_version: VERSION.into() });
        a.update(Action::SwitchTab(Tab::Subscriptions));
        a.update(subs_done(1, subs));
        a.handle_key(key(KeyCode::Enter));
        let out = render(&mut a, 80, 24);
        assert!(out.contains(&format!("{} ", s.q_total)), "{lang:?}: 订阅详情里「{}」没有完整显示\n{out}", s.q_total);
    }
}

/// 虚拟模型页左栏的模式短名在每种语言下都完整显示 (左栏宽度随最长的短名放宽)。
#[test]
fn routing_mode_names_are_not_truncated_in_any_language() {
    for lang in Lang::ALL {
        use_lang(lang);
        let s = s();
        let out = render(&mut vm_app(false), 80, 24);
        for name in [s.vm_mode_seq, s.vm_mode_rr, s.vm_mode_sticky] {
            assert!(out.contains(name), "{lang:?}: 模式短名「{name}」被截断\n{out}");
        }
    }
}

/// 80 列日志表的模型列在每种语言下至少放得下表头——其余定宽列的表头更宽的语言 (日文) 从订阅列借差额。
#[test]
fn logs_model_header_is_not_truncated_at_80x24_in_any_language() {
    for lang in Lang::ALL {
        use_lang(lang);
        let s = s();
        let mut a = logs_app(false);
        a.update(requests_done(1, RequestQuery::default(), logs_fixture_rows(), 4));
        let out = render(&mut a, 80, 24);
        assert!(out.contains(&format!(" {} ", s.lg_col_model)), "{lang:?}: 模型列表头「{}」被截断\n{out}", s.lg_col_model);
    }
}

/// 请求详情弹窗的字段标签在每种语言下都完整显示 (标签列随最宽的标签放宽)。
#[test]
fn request_detail_labels_are_not_truncated_in_any_language() {
    for lang in Lang::ALL {
        use_lang(lang);
        let s = s();
        let mut a = logs_app(false);
        a.update(requests_done(1, RequestQuery::default(), logs_fixture_rows(), 4));
        a.handle_key(key(KeyCode::Down)); // 选中「失败」那一行
        let action = a.handle_key(key(KeyCode::Enter)).expect("⏎ 应该产出 Action::OpenDetail");
        a.update(action);
        let out = render(&mut a, 80, 40);
        for label in [s.lg_d_time, s.lg_d_id, s.lg_d_status, s.lg_d_vm, s.lg_d_real_model, s.lg_d_sub, s.lg_d_provider, s.lg_d_latency] {
            assert!(out.contains(&format!("{label} ")), "{lang:?}: 字段标签「{label}」被截断或贴着值\n{out}");
        }
    }
}

/// 超时的短形只用在 80 列日志表的状态列; 详情弹窗与过滤选择器用全称 (日文两者不同:
/// 時間切れ / タイムアウト)。
#[test]
fn timeout_short_form_is_only_used_in_the_logs_table() {
    for lang in Lang::ALL {
        use_lang(lang);
        let s = s();
        let mut a = logs_app(false);
        a.update(requests_done(1, RequestQuery::default(), logs_fixture_rows(), 4));
        let table = render(&mut a, 80, 24);
        assert!(table.contains(&format!("✕ {}", s.lg_status_timeout_short)), "{lang:?}: 表格状态列应该用短形\n{table}");

        a.handle_key(key(KeyCode::Down));
        a.handle_key(key(KeyCode::Down)); // 超时那一行
        let action = a.handle_key(key(KeyCode::Enter)).expect("⏎ 应该产出 Action::OpenDetail");
        a.update(action);
        let detail = render(&mut a, 80, 40);
        assert!(detail.contains(s.lg_status_timeout), "{lang:?}: 详情弹窗应该用全称「{}」\n{detail}", s.lg_status_timeout);
        if let Some(close) = a.handle_key(key(KeyCode::Esc)) {
            a.update(close);
        }

        let action = a.handle_key(key(KeyCode::Char('/'))).expect("/ 应该产出 Action::OpenPicker");
        a.update(action);
        let picker = render(&mut a, 80, 24);
        assert!(picker.contains(s.lg_status_timeout), "{lang:?}: 过滤选择器应该用全称「{}」\n{picker}", s.lg_status_timeout);
    }
}

/// 订阅详情「最近错误」按词折行时, 行数按实际折行结果算: 一段只占约两行宽度、按词却要折成五行的
/// 英文, 放不下的部分以省略号标出, 不能被静默吞掉 (按「总宽 ÷ 列宽 + 1」估算行数时第五行会消失,
/// 且没有任何省略号)。值列宽从渲染结果里量, 不写死布局常量。
#[test]
fn a_word_wrapped_last_error_never_loses_its_tail_silently() {
    use_lang(Lang::En);
    let open_kimi = |error: &str| {
        let mut subs = detail_subs();
        subs[1].last_error_message = Some(error.to_string());
        let mut a = app(false);
        a.update(Action::Connected { app_version: VERSION.into() });
        a.update(Action::SwitchTab(Tab::Subscriptions));
        a.update(subs_done(1, subs));
        a.handle_key(key(KeyCode::Down));
        a.handle_key(key(KeyCode::Enter));
        a
    };
    let buf = render_buffer(&mut open_kimi("Z"), 80, 24);
    let row = (0..buf.area.height).find(|&y| buffer_row_text(&buf, y).contains(EN.sub_f_last_error)).expect("应该有「最近错误」一行");
    let text = buffer_row_text(&buf, row);
    let value_x = text.find('Z').expect("占位错误文字");
    let border_x = text.rfind('│').expect("右边框");
    let width = border_x - 1 - value_x; // 右内距一列

    // 1 列的词与 (width - 1) 列的词交替: 相邻两个词都凑不进一行, 共 5 行; 总宽约两行。
    let long = |c: char| c.to_string().repeat(width - 1);
    let error = format!("a {} b {} tail", long('x'), long('y'));
    let out = render(&mut open_kimi(&error), 80, 24);
    assert!(out.contains("tail") || out.contains(&format!("{}…", long('y'))), "最近错误的尾部被静默吞掉\n{out}");
}

/// 每种语言的固定文案确认弹窗在 80×24 上完整显示 (超长的行折行, 不被右边框截断)。去掉全部空白后
/// 逐字比较弹窗内部的文字与提示原文——折行位置不影响比较, 丢字就会不相等。
#[test]
fn fixed_confirm_prompts_are_shown_in_full_at_80x24() {
    for lang in Lang::ALL {
        use_lang(lang);
        let s = s();
        let refs = ["model-fallback", "model-sonnet", "model-haiku", "model-opus"].join(s.list_sep);
        let delete = format!("{}\n{}\n{}", (s.sub_confirm_delete)("智谱主号"), (s.sub_delete_refs)(4), refs);
        for prompt in [s.confirm_discard.to_string(), s.wiz_confirm_exit_pending.to_string(), delete] {
            let mut a = loaded(false);
            a.update(Action::OpenConfirm { prompt: prompt.clone(), on_yes: OnYes::discard_then(Action::Quit) });
            let buf = render_buffer(&mut a, 80, 24);
            let area = confirm::area(buf.area, &prompt);
            let mut shown = String::new();
            for y in area.y + 1..area.bottom() - 1 {
                for x in area.x + 1..area.right() - 1 {
                    shown.push_str(buf[(x, y)].symbol());
                }
            }
            let squash = |t: &str| t.chars().filter(|c| !c.is_whitespace()).collect::<String>();
            assert_eq!(squash(&shown), squash(&prompt), "{lang:?}: 确认弹窗没有完整显示\n{}", render(&mut a, 80, 24));
        }
    }
}

/// 英文虚拟模型页, 选中 `model-fallback`: 成员里有未配兜底槽的翻译类订阅 (「将被跳过」必须完整
/// 显示), 左栏模式短名与右栏底部摘要都不截断。
#[test]
fn en_virtual_models_80x24() {
    use_lang(Lang::En);
    let mut a = vm_app(false);
    for _ in 0..4 {
        a.handle_key(key(KeyCode::Down));
    }
    let out = render(&mut a, 80, 24);
    assert!(out.contains(EN.vm_will_skip), "「will be skipped」应该完整显示\n{out}");
    insta::assert_snapshot!(out);
}

/// 会话亲和 (sticky) 模式的全名 + 成员数摘要在每种语言的 80 列右栏底边完整显示 (会话亲和是
/// 最长的模式全名)。
#[test]
fn sticky_mode_summary_fits_at_80x24_in_every_language() {
    for lang in Lang::ALL {
        use_lang(lang);
        let s = s();
        let mut a = vm_app(false);
        a.handle_key(key(KeyCode::Down));
        a.handle_key(key(KeyCode::Down)); // model-sonnet (sticky, 3 个成员)
        let out = render(&mut a, 80, 24);
        let summary = (s.vm_members_summary)(s.vm_mode_full_sticky, 3);
        assert!(out.contains(&format!(" {summary} ")), "{lang:?}: 底部摘要「{summary}」应该完整显示\n{out}");
    }
}

// ---------- 日文界面 ----------

#[test]
fn ja_overview_80x24() {
    use_lang(Lang::Ja);
    insta::assert_snapshot!(render(&mut loaded(false), 80, 24));
}

#[test]
fn ja_subscriptions_120x40() {
    use_lang(Lang::Ja);
    insta::assert_snapshot!(render(&mut subs_app(false), 120, 40));
}

#[test]
fn ja_wizard_custom_80x24() {
    use_lang(Lang::Ja);
    insta::assert_snapshot!(render(&mut custom_wizard_probed_and_filled(), 80, 24));
}

#[test]
fn ja_delete_confirm_80x24() {
    use_lang(Lang::Ja);
    let mut a = subs_app(false);
    render(&mut a, 80, 24);
    let action = a.handle_key(key(KeyCode::Char('d'))).expect("应该产出确认弹窗");
    a.update(action);
    insta::assert_snapshot!(render(&mut a, 80, 24));
}

#[test]
fn ja_help_subscriptions_80x24() {
    use_lang(Lang::Ja);
    let mut a = subs_app(false);
    a.update(Action::ToggleHelp);
    insta::assert_snapshot!(render(&mut a, 80, 24));
}

#[test]
fn ja_logs_120x40() {
    use_lang(Lang::Ja);
    let mut a = logs_app(false);
    a.update(requests_done(1, RequestQuery::default(), logs_fixture_rows(), 4));
    insta::assert_snapshot!(render(&mut a, 120, 40));
}

#[test]
fn ja_live_80x24() {
    use_lang(Lang::Ja);
    insta::assert_snapshot!(render(&mut live_fixture_app(false), 80, 24));
}

#[test]
fn ja_wizard_slots_80x24() {
    use_lang(Lang::Ja);
    let mut a = wizard_at_slots(vec![ModelInfo { id: "glm-4.6".into(), display_name: None }]);
    insta::assert_snapshot!(render(&mut a, 80, 24));
}

/// 日文虚拟模型页, 选中 `model-fallback` (同英文那张): 「スキップ対象」、左栏模式短名、右栏底部摘要
/// 都完整显示。
#[test]
fn ja_virtual_models_80x24() {
    use_lang(Lang::Ja);
    let mut a = vm_app(false);
    for _ in 0..4 {
        a.handle_key(key(KeyCode::Down));
    }
    let out = render(&mut a, 80, 24);
    assert!(out.contains(JA.vm_will_skip), "「スキップ対象」应该完整显示\n{out}");
    insta::assert_snapshot!(out);
}

