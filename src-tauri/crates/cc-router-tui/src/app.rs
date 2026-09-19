//! 应用状态 + 外壳 (标签栏 / 底栏 / 帮助 / toast)。不碰网络也不碰真实终端:
//! `handle_key` / `update` 是同步纯逻辑, `draw` 只写 `Frame` —— 所以整个文件可以用 `TestBackend` 测。
//! 把它接到键盘、定时器、HTTP 上的是 `runtime.rs`。

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Tabs};
use ratatui::Frame;
use throbber_widgets_tui::{Throbber, BRAILLE_SIX};

use crate::action::{Action, Cmd, Fetch, FetchData, Mutation, MutationOutcome, Tab};
use crate::client::dto::{RefreshBalanceResult, RefreshModelsResult};
use crate::fx::{self, Dir, Fx};
use crate::i18n::Strings;
use crate::pages::overview::Overview;
use crate::pages::placeholder::Placeholder;
use crate::pages::subscriptions::Subscriptions;
use crate::pages::{Component, DrawCtx};
use crate::store::Store;
use crate::theme::Theme;
use crate::widgets::toast::{self, Toast, ToastKind};
use crate::widgets::{help, keybar, spinner_state};

pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;
/// 可见页面每 5 秒重拉一次用量类数字 (20 × 250ms)。
const POLL_EVERY_TICKS: u64 = 20;
/// toast 队列上限, 含正在屏幕上的那条; 超出丢最旧的排队项 (F4)。
const MAX_TOASTS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Conn {
    Connecting,
    Connected,
    Reconnecting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Popup {
    Help,
}

/// 等下一帧知道几何信息之后才能触发的弹窗动效。
#[derive(Debug, Clone, Copy)]
enum PopupFx {
    Open,
    Close(Rect),
}

pub struct AppOptions {
    pub strings: &'static Strings,
    pub theme: Theme,
    pub fx_enabled: bool,
    /// Unix 毫秒; 之后由 `Action::Tick` 推进。
    pub now_ms: i64,
    /// 生产传 `env!("CARGO_PKG_VERSION")`。做成参数是为了快照测试不随发版改版本号而失效。
    pub tui_version: &'static str,
}

pub struct App {
    s: &'static Strings,
    theme: Theme,
    fx: Fx,
    tui_version: &'static str,
    tab: Tab,
    store: Store,
    overview: Overview,
    subscriptions: Subscriptions,
    placeholder: Placeholder,
    popup: Option<Popup>,
    popup_area: Option<Rect>,
    pending_popup_fx: Option<PopupFx>,
    pending_page_fx: Option<Dir>,
    startup_played: bool,
    toasts: VecDeque<Toast>,
    conn: Conn,
    app_version: Option<String>,
    now_ms: i64,
    tick: u64,
    /// 正在进行的就地操作, 键是订阅 id。**按订阅 id 判重, 不按 `Mutation` 整体** —— 同一条订阅
    /// 同时只能有一个操作在跑, 但不同操作 (比如先 `t` 再 `e`) 仍然互斥, 不是各自独立排队。
    busy: HashMap<String, Mutation>,
    /// 每条订阅最近一次就地操作的结果, 与对应 toast 用的是**同一份文本** (I1 fix): toast 只能显示
    /// 一行 (≤72 列, 3 秒就消失), 详情面板的「上次操作」行拿这份存档展示完整文案。发起新操作时
    /// (`start_mutation` 真的派发出去那一刻) 移除对应条目, 不是等结果回来才清。
    last_outcome: HashMap<String, (ToastKind, String)>,
}

impl App {
    pub fn new(opts: AppOptions) -> Self {
        Self {
            s: opts.strings,
            theme: opts.theme,
            fx: Fx::new(opts.fx_enabled),
            tui_version: opts.tui_version,
            tab: Tab::Overview,
            store: Store::default(),
            overview: Overview::default(),
            subscriptions: Subscriptions::default(),
            placeholder: Placeholder,
            popup: None,
            popup_area: None,
            pending_popup_fx: None,
            pending_page_fx: None,
            startup_played: false,
            toasts: VecDeque::new(),
            conn: Conn::Connecting,
            app_version: None,
            now_ms: opts.now_ms,
            tick: 0,
            busy: HashMap::new(),
            last_outcome: HashMap::new(),
        }
    }

    /// 有动效在播: 主循环应该按约 60fps 继续画; 否则等下一个事件再画。
    pub fn wants_fast_frames(&self) -> bool {
        self.fx.is_running()
    }

    /// 按 `tab` 从几个不相交的字段里选一个页面。写成拿具体字段引用的关联函数 (而不是
    /// `&mut self` 的 helper 方法), 这样调用方在拿到 `&mut dyn Component` 的同时还能借用
    /// `self` 的其它字段 (比如 `self.store`) —— `&mut self` 的方法做不到这一点。
    fn select_page<'a>(
        tab: Tab,
        overview: &'a mut Overview,
        subscriptions: &'a mut Subscriptions,
        placeholder: &'a mut Placeholder,
    ) -> &'a mut dyn Component {
        match tab {
            Tab::Overview => overview,
            Tab::Subscriptions => subscriptions,
            Tab::VirtualModels | Tab::Live | Tab::Logs => placeholder,
        }
    }

    /// 同上, 只读版本 (帮助弹窗只需要 `help()`, 不需要 `&mut`)。
    fn select_page_ref<'a>(
        tab: Tab,
        overview: &'a Overview,
        subscriptions: &'a Subscriptions,
        placeholder: &'a Placeholder,
    ) -> &'a dyn Component {
        match tab {
            Tab::Overview => overview,
            Tab::Subscriptions => subscriptions,
            Tab::VirtualModels | Tab::Live | Tab::Logs => placeholder,
        }
    }

    fn version_mismatch(&self) -> Option<String> {
        let app = self.app_version.as_deref()?;
        let tui = self.tui_version;
        (app != tui).then(|| (self.s.version_mismatch)(tui, app))
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        // Windows 上按下和松开各报一次, 只认按下。
        if key.kind != KeyEventKind::Press {
            return None;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(Action::Quit);
        }
        if self.popup.is_some() {
            return match key.code {
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => Some(Action::ClosePopup),
                _ => None,
            };
        }
        match key.code {
            KeyCode::Char('q') => Some(Action::Quit),
            KeyCode::Char('?') => Some(Action::ToggleHelp),
            KeyCode::Char('r') => Some(Action::Refresh),
            KeyCode::Tab => Some(Action::NextTab),
            KeyCode::BackTab => Some(Action::PrevTab),
            KeyCode::Char(c @ '1'..='5') => Tab::from_index(c as usize - '1' as usize).map(Action::SwitchTab),
            _ => Self::select_page(self.tab, &mut self.overview, &mut self.subscriptions, &mut self.placeholder).handle_key(key, &self.store),
        }
    }

    fn switch_tab(&mut self, to: Tab, dir: Dir) -> Vec<Cmd> {
        if to == self.tab {
            return Vec::new();
        }
        self.tab = to;
        self.pending_page_fx = Some(dir);
        // 不可见的页面不轮询, 所以切回来的那一刻要补一次。
        if self.conn == Conn::Connected {
            Self::select_page(self.tab, &mut self.overview, &mut self.subscriptions, &mut self.placeholder).update(&Action::Refresh, &self.store)
        } else {
            Vec::new()
        }
    }

    fn close_popup(&mut self) {
        if self.popup.take().is_some() {
            self.pending_popup_fx = self.popup_area.take().map(PopupFx::Close);
        }
    }

    /// `Store` 刚接受了一份订阅列表 (`FetchDone` 两个分支共用): 广播给所有页面。**唯一**列出
    /// 全部页面字段的地方 —— 以后加新页面只改这一处, 不会出现"某个 FetchDone 分支忘了通知新
    /// 页面"这种只有部分刷新路径才触发、没有测试能咬住的漏更 (fix round 1, I1)。
    fn notify_subscriptions_changed(&mut self, changed: &[String]) {
        self.overview.on_subscriptions_changed(changed);
        self.subscriptions.on_subscriptions_changed(changed);
        self.placeholder.on_subscriptions_changed(changed);
    }

    /// 两处 toast 入口共用: 与最新排队的一条重复 (同 kind 同 text) 就丢弃, 否则挤掉最旧的排队项
    /// (下标 1, 下标 0 是正在屏幕上的那条, 不能被挤走)。
    fn push_toast(&mut self, toast: Toast) {
        let is_dup = self.toasts.back().is_some_and(|t| t.kind == toast.kind && t.text == toast.text);
        if is_dup {
            return;
        }
        self.toasts.push_back(toast);
        while self.toasts.len() > MAX_TOASTS {
            self.toasts.remove(1);
        }
    }

    /// 订阅备注名, 取不到 (结果回来时订阅已经不在 `Store` 里了) 就退回用 id。
    fn subscription_name(&self, id: &str) -> String {
        self.store.subscription(id).map(|s| s.display_name.clone()).unwrap_or_else(|| id.to_string())
    }

    /// `Action::Mutate` 落地成真正要发的 `Cmd`: 同一订阅已经有操作在跑 → 丢弃; 断线 → 弹 toast
    /// 拒绝; 余额刷新在不支持的 provider 上 → 就地回答, 不发请求。三条判定都不进忙碌表, 只有真的
    /// 要发的那条才 `insert`。
    fn start_mutation(&mut self, m: Mutation) -> Vec<Cmd> {
        let id = m.subscription_id();
        // 断线判定必须排在忙碌表前面: 断线期间按在一条正忙的订阅上 (比如上一次操作还没跑完就掉线了)
        // 也该看到「未连接」提示, 而不是被忙碌表悄悄吞掉、什么反馈都没有。
        if self.conn != Conn::Connected {
            self.push_toast(Toast::new(ToastKind::Error, self.s.toast_offline));
            return Vec::new();
        }
        if self.busy.contains_key(id) {
            return Vec::new();
        }
        if matches!(m, Mutation::RefreshBalance { .. }) {
            // 订阅不在 Store 里 (理论上不该发生, 因为发起方是页面自己选中的一条) 时保守放行,
            // 交给后端去报错, 不在这里就地拦。
            let supported = self.store.subscription(id).is_none_or(|s| s.balance_supported);
            if !supported {
                self.push_toast(Toast::new(ToastKind::Info, self.s.sub_balance_unsupported));
                return Vec::new();
            }
        }
        self.busy.insert(id.to_string(), m.clone());
        // I1 fix: 这条订阅上一次操作的结果 (如果还挂在详情面板上) 已经过时了, 新操作一发起就该
        // 隐去它, 不能让用户以为「上次操作」显示的是这次刚发出去的操作的结果。
        self.last_outcome.remove(id);
        vec![Cmd::Mutate(m)]
    }

    /// `Action::MutationDone`: 从忙碌表移除, 按结果弹一条 toast, **无论成败都追加一次订阅列表
    /// 刷新** —— 状态 / 缓存 / 错误信息可能都变了。toast 与 `last_outcome`（I1 fix）**共用同一份
    /// 文本**: toast 只能显示一行 (≤72 列, 3 秒就消失), 详情面板的「上次操作」行拿 `last_outcome`
    /// 展示完整文案, 不受 toast 单行截断的限制。
    fn finish_mutation(&mut self, mutation: Mutation, result: Result<MutationOutcome, String>) -> Vec<Cmd> {
        let id = mutation.subscription_id().to_string();
        self.busy.remove(&id);
        let name = self.subscription_name(&id);
        let (kind, text) = match &result {
            Ok(MutationOutcome::EnabledSet) => {
                let enabled = matches!(mutation, Mutation::SetEnabled { enabled: true, .. });
                // 乐观更新 Store, 不等下一次 Fetch::Subscriptions 落地: 否则「按 e、还没刷新完又按
                // 一次 e」会从 Store 读到没改过的旧 enabled, 算出同一个目标值发给后端 (no-op),
                // 且第二条 toast 文案与第一条相同, 被 push_toast 的去重规则吞掉, 用户毫无反馈。
                self.store.set_enabled(&id, enabled);
                let text = if enabled { (self.s.toast_enabled)(&name) } else { (self.s.toast_disabled)(&name) };
                (ToastKind::Success, text)
            }
            Ok(MutationOutcome::Tested(r)) if r.ok => (ToastKind::Success, (self.s.toast_test_ok)(&name, r.model_used.as_deref())),
            Ok(MutationOutcome::Tested(r)) => (ToastKind::Error, (self.s.toast_test_failed)(&name, &r.message)),
            Ok(MutationOutcome::Models(RefreshModelsResult::Auto { models, .. })) => {
                (ToastKind::Success, (self.s.toast_models_ok)(&name, models.len()))
            }
            Ok(MutationOutcome::Models(RefreshModelsResult::ManualFallback { reason })) => {
                (ToastKind::Info, (self.s.toast_models_manual)(&name, reason))
            }
            Ok(MutationOutcome::Balance(RefreshBalanceResult::Success { .. })) => (ToastKind::Success, (self.s.toast_balance_ok)(&name)),
            Ok(MutationOutcome::Balance(RefreshBalanceResult::Failed { reason })) => {
                (ToastKind::Error, (self.s.toast_balance_failed)(&name, reason))
            }
            Ok(MutationOutcome::Balance(RefreshBalanceResult::Unsupported)) => (ToastKind::Info, self.s.sub_balance_unsupported.to_string()),
            Err(message) => (ToastKind::Error, (self.s.toast_mutation_failed)(&name, message)),
        };
        self.push_toast(Toast::new(kind, text.clone()));
        self.last_outcome.insert(id, (kind, text));
        vec![Cmd::Fetch(Fetch::Subscriptions)]
    }

    pub fn update(&mut self, action: Action) -> Vec<Cmd> {
        match action {
            Action::Quit => vec![Cmd::Quit],
            Action::SwitchTab(to) => {
                let dir = if to.index() > self.tab.index() { Dir::Forward } else { Dir::Backward };
                self.switch_tab(to, dir)
            }
            Action::NextTab => self.switch_tab(self.tab.next(), Dir::Forward),
            Action::PrevTab => self.switch_tab(self.tab.prev(), Dir::Backward),
            Action::ToggleHelp => {
                if self.popup.is_some() {
                    self.close_popup();
                } else {
                    self.popup = Some(Popup::Help);
                    self.pending_popup_fx = Some(PopupFx::Open);
                }
                Vec::new()
            }
            Action::ClosePopup => {
                self.close_popup();
                Vec::new()
            }
            Action::Tick { now_ms } => {
                self.now_ms = now_ms;
                self.tick += 1;
                while self.toasts.front().is_some_and(|t| t.expired(now_ms)) {
                    self.toasts.pop_front();
                }
                if self.conn == Conn::Connected && self.tick.is_multiple_of(POLL_EVERY_TICKS) {
                    Self::select_page(self.tab, &mut self.overview, &mut self.subscriptions, &mut self.placeholder).update(&Action::Refresh, &self.store)
                } else {
                    Vec::new()
                }
            }
            Action::Connected { ref app_version } => {
                if self.conn == Conn::Reconnecting {
                    self.push_toast(Toast::new(ToastKind::Success, self.s.toast_reconnected));
                }
                self.conn = Conn::Connected;
                self.app_version = Some(app_version.clone());
                Self::select_page(self.tab, &mut self.overview, &mut self.subscriptions, &mut self.placeholder).update(&action, &self.store)
            }
            Action::ConnectionLost => {
                self.conn = Conn::Reconnecting;
                Vec::new()
            }
            Action::FetchDone { fetch, issued, result } => match result {
                Err(message) => {
                    // 断线期间每次轮询都会失败, 状态点已经在说「重连中」了, 不再刷屏。
                    if self.conn == Conn::Connected {
                        self.push_toast(Toast::new(ToastKind::Error, (self.s.toast_load_failed)(&message)));
                    }
                    Vec::new()
                }
                // 总览一次整页加载: 订阅列表 `mem::take` 挪给 Store (不克隆——克隆一次给 Store、
                // 整个 OverviewData 转给总览页时又整体克隆一次, 同一份列表会被复制两遍), 状态变了
                // 的 id 广播给所有页面; 不管 Store 是不是接受了这份订阅列表 (可能是晚到的旧结果),
                // 总览页自己的其它字段 (今日统计 / 每小时序列 / 代理状态) 都照常更新。
                Ok(FetchData::Overview(mut data)) => {
                    debug_assert_eq!(fetch, Fetch::Overview, "spawn_fetch 应该保证 FetchData::Overview 只配 Fetch::Overview");
                    let subs = std::mem::take(&mut data.subscriptions);
                    let changed = self.store.apply_subscriptions(issued, subs);
                    if let Some(changed) = &changed {
                        self.notify_subscriptions_changed(changed);
                    }
                    let action = Action::FetchDone { fetch, issued, result: Ok(FetchData::Overview(data)) };
                    // 加载结果永远交给发起它的页面, 哪怕用户已经切走了。
                    self.overview.update(&action, &self.store)
                }
                // 单独的订阅列表刷新 (SSE 触发): 只进 Store, 不需要转给任何页面的 update ——
                // 各页面画的时候直接读 ctx.store。
                Ok(FetchData::Subscriptions(subs)) => {
                    debug_assert_eq!(fetch, Fetch::Subscriptions, "spawn_fetch 应该保证 FetchData::Subscriptions 只配 Fetch::Subscriptions");
                    let changed = self.store.apply_subscriptions(issued, subs);
                    if let Some(changed) = &changed {
                        self.notify_subscriptions_changed(changed);
                    }
                    Vec::new()
                }
            },
            Action::Refresh | Action::Sse { .. } => {
                Self::select_page(self.tab, &mut self.overview, &mut self.subscriptions, &mut self.placeholder).update(&action, &self.store)
            }
            Action::Mutate(m) => self.start_mutation(m),
            Action::MutationDone { mutation, result } => self.finish_mutation(mutation, result),
        }
    }

    fn draw_tabs(&self, frame: &mut Frame, area: Rect) {
        let status = match self.conn {
            Conn::Connected => Line::from(vec![
                Span::styled(" ● ", Style::new().fg(self.theme.ok)),
                Span::raw(self.s.conn_connected),
                Span::styled(
                    format!(" · v{} ", self.app_version.as_deref().unwrap_or("?")),
                    self.theme.muted_style(),
                ),
            ]),
            Conn::Connecting | Conn::Reconnecting => {
                let state = spinner_state(self.tick);
                let spinner = Throbber::default().throbber_set(BRAILLE_SIX).to_symbol_span(&state);
                let text = if self.conn == Conn::Connecting { self.s.conn_connecting } else { self.s.conn_reconnecting };
                Line::from(vec![Span::raw(" "), spinner, Span::raw(format!("{text} "))]).style(Style::new().fg(self.theme.warn))
            }
        };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(self.theme.border_style())
            .title_top(Line::styled(" cc-router ", self.theme.accent_bold()))
            .title_top(status.right_aligned());
        let titles = self.s.tabs.iter().enumerate().map(|(i, name)| format!(" {} {name} ", i + 1));
        let tabs = Tabs::new(titles)
            .select(self.tab.index())
            .style(self.theme.muted_style())
            .highlight_style(self.theme.accent_bold().add_modifier(Modifier::REVERSED))
            .divider(" ")
            .padding("", "")
            .block(block);
        frame.render_widget(tabs, area);
    }

    fn draw_toast(&mut self, frame: &mut Frame, screen: Rect, top: u16) {
        let now = self.now_ms;
        let Some(toast) = self.toasts.front_mut() else { return };
        let area = toast::area(screen, top, &toast.text);
        match toast.shown_at {
            None => {
                toast.shown_at = Some(now);
                self.fx.toast_in(area, self.theme.border);
            }
            Some(at) if !toast.fading && now - at >= toast::LIFETIME_MS - i64::from(fx::ms::TOAST_OUT) => {
                toast.fading = true;
                self.fx.toast_out(area);
            }
            Some(_) => {}
        }
        // 消散效果播完之后不再画它: 它要等下一次 `Action::Tick` (可能晚 200ms 才到) 才会被真正
        // 弹出队列, 这段空隙里任何重画 (按键 / SSE 事件) 都不能让它以全亮度闪回 (H2)。
        if toast.fading && self.fx.toast_out_finished() {
            return;
        }
        toast::draw(frame, area, toast, &self.theme);
    }

    /// `elapsed`: 距上一帧的时间, 用来推进动效。
    pub fn draw(&mut self, frame: &mut Frame, elapsed: Duration) {
        let screen = frame.area();
        if screen.width < MIN_WIDTH || screen.height < MIN_HEIGHT {
            let line = Line::raw(self.s.too_small).centered();
            frame.render_widget(line, screen.centered_vertically(Constraint::Length(1)));
            // 太小画不出可以叠动效的内容; 不清掉的话 is_running() 会一直为真, 把主循环锁在 60fps。
            self.fx.clear();
            return;
        }
        // 空闲时两帧之间可能隔了几百毫秒; 动效是这一帧才加进来的话, 不能把这段空闲算成它已经播过的时间。
        let was_running = self.fx.is_running();

        let banner = self.version_mismatch();
        let [tabs, banner_area, content, footer] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(u16::from(banner.is_some())),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(screen);

        self.draw_tabs(frame, tabs);
        if let Some(text) = banner {
            frame.render_widget(Line::styled(format!(" ⚠ {text}"), Style::new().fg(self.theme.warn)), banner_area);
        }

        let s = self.s;
        let mut ctx = DrawCtx {
            theme: &self.theme,
            s,
            now_ms: self.now_ms,
            tick: self.tick,
            fx: &mut self.fx,
            store: &self.store,
            busy: &self.busy,
            last_outcome: &self.last_outcome,
        };
        let page = Self::select_page(self.tab, &mut self.overview, &mut self.subscriptions, &mut self.placeholder);
        page.draw(frame, content, &mut ctx);
        let mut left = page.hints(s);
        left.insert(0, ("1-5", s.key_switch_tab));
        keybar::draw(frame, footer, &left, &[("?", s.key_help), ("q", s.key_quit)], &self.theme);

        self.draw_toast(frame, screen, content.y);

        if self.popup.is_some() {
            // 压暗背景用静态的 DIM 修饰符而不是动效: 16 色 / 无色终端下同样成立。
            frame.buffer_mut().set_style(screen, Style::new().add_modifier(Modifier::DIM));
            let page_rows = Self::select_page_ref(self.tab, &self.overview, &self.subscriptions, &self.placeholder).help(s);
            let area = help::area(screen, s, page_rows);
            help::draw(frame, area, &self.theme, s, page_rows);
            self.popup_area = Some(area);
        }

        if !self.startup_played {
            self.startup_played = true;
            self.fx.startup(self.overview.logo_area().unwrap_or_default(), screen, self.theme.border);
        }
        if let Some(dir) = self.pending_page_fx.take() {
            self.fx.page_enter(dir, content, self.theme.border);
        }
        match self.pending_popup_fx.take() {
            Some(PopupFx::Open) => self.fx.popup_open(self.popup_area.unwrap_or_default(), self.theme.border),
            Some(PopupFx::Close(area)) => self.fx.popup_close(area),
            None => {}
        }
        let dt = if was_running { elapsed } else { Duration::ZERO };
        self.fx.process(dt, frame.buffer_mut(), screen);
    }
}
