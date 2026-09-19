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

use crate::action::{Action, BusyKey, Cmd, Fetch, FetchData, Mutation, MutationOutcome, Tab};
use crate::client::dto::{RefreshBalanceResult, RefreshModelsResult};
use crate::fx::{self, Dir, Fx};
use crate::i18n::Strings;
use crate::pages::{Component, DrawCtx, Pages};
use crate::popup::{ConfirmState, Popup};
use crate::store::Store;
use crate::theme::Theme;
use crate::widgets::picker::{self, PickerState};
use crate::widgets::toast::{self, Toast, ToastKind};
use crate::widgets::{confirm, help, keybar, spinner_state};

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
    pages: Pages,
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
    /// 正在进行的就地操作, 键是 [`BusyKey`] (目前只会出现 `Subscription` 变体)。**按
    /// `Mutation::busy_key()` 判重, 不按 `Mutation` 整体** —— 同一条订阅同时只能有一个操作在跑,
    /// 但不同操作 (比如先 `t` 再 `e`) 仍然互斥, 不是各自独立排队。
    busy: HashMap<BusyKey, Mutation>,
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
            pages: Pages::default(),
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
        // Ctrl+C 永远立即退出, 不确认——弹窗打开、页面有未保存修改都不例外, 这是它与 `q`
        // (`Action::Quit`, 会先检查 dirty / 被弹窗吞掉) 唯一的区别。
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(Action::ForceQuit);
        }
        // 弹窗打开时按变体各自决定按键含义; 键盘完全归弹窗, 到不了下面的全局键 / 页面。
        // `&mut self.popup`: `Popup::Picker` 的按键 (打字 / 移动选中) 直接改 `PickerState` 自身,
        // 不像 Help / Confirm 那样只读。
        if let Some(popup) = &mut self.popup {
            return match popup {
                Popup::Help => match key.code {
                    KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => Some(Action::ClosePopup),
                    _ => None,
                },
                Popup::Confirm(state) => match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => Some(Action::Confirmed(state.on_yes.clone())),
                    // 默认 N: Esc / ⏎ 与显式的 n/N 一样只关弹窗, 不执行 on_yes。
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Enter => Some(Action::ClosePopup),
                    _ => None,
                },
                Popup::Picker(state) => state.handle_key(key),
            };
        }
        match key.code {
            KeyCode::Char('q') => Some(Action::Quit),
            KeyCode::Char('?') => Some(Action::ToggleHelp),
            KeyCode::Char('r') => Some(Action::Refresh),
            KeyCode::Tab => Some(Action::NextTab),
            KeyCode::BackTab => Some(Action::PrevTab),
            KeyCode::Char(c @ '1'..='5') => Tab::from_index(c as usize - '1' as usize).map(Action::SwitchTab),
            _ => self.pages.get_mut(self.tab).handle_key(key, &self.store),
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
            self.pages.get_mut(self.tab).update(&Action::Refresh, &self.store)
        } else {
            Vec::new()
        }
    }

    fn close_popup(&mut self) {
        if self.popup.take().is_some() {
            self.pending_popup_fx = self.popup_area.take().map(PopupFx::Close);
        }
    }

    fn open_confirm(&mut self, prompt: String, on_yes: Box<Action>) {
        // Fix round C: 先走正常的关闭路径——如果已经有另一个弹窗开着, 这样才会清掉它的
        // `popup_area` (不清的话, 如果新弹窗在第一次 draw() 之前就被关掉, 关闭动效会拿旧弹窗的
        // 几何去播), 而不是直接覆盖 `self.popup` 留下不一致的状态。
        self.close_popup();
        self.popup = Some(Popup::Confirm(ConfirmState { prompt, on_yes }));
        self.pending_popup_fx = Some(PopupFx::Open);
    }

    /// `Action::OpenPicker` 与 `open_confirm` 共用同一条「替换掉已经打开的弹窗」规则 (Fix round C)。
    fn open_picker(&mut self, spec: picker::PickerSpec) {
        self.close_popup();
        self.popup = Some(Popup::Picker(PickerState::new(spec)));
        self.pending_popup_fx = Some(PopupFx::Open);
    }

    /// `Quit` / `SwitchTab` / `NextTab` / `PrevTab` 四个来源共用: 当前页面 `is_dirty()` 时不直接
    /// 执行 `action`, 而是打开确认弹窗把它包进 `on_yes`; 否则调用 `run` 真正执行。`Action::Confirmed`
    /// 里 `discard_changes()` 之后再 `update(*inner)`, 会重新走到这里, 但那时 `is_dirty()` 已经是
    /// `false`——天然放行, 不需要另一条「不检查 dirty」的旁路。
    fn guard_dirty(&mut self, action: Action, run: impl FnOnce(&mut Self) -> Vec<Cmd>) -> Vec<Cmd> {
        if self.pages.get(self.tab).is_dirty() {
            let prompt = self.s.confirm_discard.to_string();
            self.open_confirm(prompt, Box::new(action));
            Vec::new()
        } else {
            run(self)
        }
    }

    /// `Store` 刚接受了一份订阅列表 (`FetchDone` 两个分支共用): 广播给所有页面。**唯一**列出
    /// 全部页面字段的地方 —— 以后加新页面只改这一处, 不会出现"某个 FetchDone 分支忘了通知新
    /// 页面"这种只有部分刷新路径才触发、没有测试能咬住的漏更 (fix round 1, I1)。
    fn notify_subscriptions_changed(&mut self, changed: &[String]) {
        self.pages.for_each_mut(|page| page.on_subscriptions_changed(changed));
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
        let key = m.busy_key();
        // 断线判定必须排在忙碌表前面: 断线期间按在一条正忙的订阅上 (比如上一次操作还没跑完就掉线了)
        // 也该看到「未连接」提示, 而不是被忙碌表悄悄吞掉、什么反馈都没有。
        if self.conn != Conn::Connected {
            self.push_toast(Toast::new(ToastKind::Error, self.s.toast_offline));
            return Vec::new();
        }
        if self.busy.contains_key(&key) {
            return Vec::new();
        }
        if matches!(m, Mutation::RefreshBalance { .. }) {
            // 订阅不在 Store 里 (理论上不该发生, 因为发起方是页面自己选中的一条) 时保守放行,
            // 交给后端去报错, 不在这里就地拦。
            if let BusyKey::Subscription(id) = &key {
                let supported = self.store.subscription(id).is_none_or(|s| s.balance_supported);
                if !supported {
                    self.push_toast(Toast::new(ToastKind::Info, self.s.sub_balance_unsupported));
                    return Vec::new();
                }
            }
        }
        self.busy.insert(key.clone(), m.clone());
        // I1 fix: 这条订阅上一次操作的结果 (如果还挂在详情面板上) 已经过时了, 新操作一发起就该
        // 隐去它, 不能让用户以为「上次操作」显示的是这次刚发出去的操作的结果。
        if let BusyKey::Subscription(id) = &key {
            self.last_outcome.remove(id);
        }
        vec![Cmd::Mutate(Box::new(m))]
    }

    /// `Action::MutationDone`: 从忙碌表移除, 先对 `mutation.refetch()` 里每个目标调对应的
    /// `set_*_barrier(barrier)` (挡住晚到的、内容还是变更前旧值的加载), 再按结果弹一条 toast,
    /// **无论成败都追加一次 `mutation.refetch()` 声明的重拉** —— 状态 / 缓存 / 错误信息可能都变了。
    /// toast 与 `last_outcome`（I1 fix）**共用同一份文本**: toast 只能显示一行 (≤72 列, 3 秒就
    /// 消失), 详情面板的「上次操作」行拿 `last_outcome` 展示完整文案, 不受 toast 单行截断的限制。
    /// `last_outcome` 只对 `BusyKey::Subscription` 的变更写 (虚拟模型页没有这块面板);
    /// `BusyKey::VirtualModel` 的变更 (Task 4 起) 仍然照常弹 toast、照常重拉, 只是不进这份存档。
    fn finish_mutation(&mut self, mutation: Mutation, barrier: u64, result: Result<MutationOutcome, String>) -> Vec<Cmd> {
        let key = mutation.busy_key();
        self.busy.remove(&key);
        for fetch in mutation.refetch() {
            match fetch {
                // 总览一次整页加载也带着订阅列表 (`Action::FetchDone` 的 `FetchData::Overview`
                // 分支同样喂给 `Store::apply_subscriptions`), 所以必须和 `Fetch::Subscriptions`
                // 一样推进订阅屏障——目前没有 `Mutation` 会把 `Fetch::Overview` 放进 `refetch()`,
                // 但空着这个分支是一个等真用上才会炸的洞, 先堵上。
                Fetch::Subscriptions | Fetch::Overview => self.store.set_subscriptions_barrier(barrier),
                Fetch::VirtualModels => self.store.set_virtual_models_barrier(barrier),
            }
        }
        let refetch_cmds: Vec<Cmd> = mutation.refetch().iter().copied().map(Cmd::Fetch).collect();

        let name = match &key {
            BusyKey::Subscription(id) => self.subscription_name(id),
            BusyKey::VirtualModel(vm_name) => vm_name.clone(),
        };
        let (kind, text) = match &result {
            Ok(MutationOutcome::EnabledSet) => {
                let enabled = matches!(mutation, Mutation::SetEnabled { enabled: true, .. });
                // 乐观更新 Store, 不等下一次 Fetch::Subscriptions 落地: 否则「按 e、还没刷新完又按
                // 一次 e」会从 Store 读到没改过的旧 enabled, 算出同一个目标值发给后端 (no-op),
                // 且第二条 toast 文案与第一条相同, 被 push_toast 的去重规则吞掉, 用户毫无反馈。
                if let BusyKey::Subscription(id) = &key {
                    self.store.set_enabled(id, enabled);
                }
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
            Ok(MutationOutcome::SlotsSaved) => (ToastKind::Success, (self.s.toast_slots_saved)(&name)),
            Ok(MutationOutcome::VirtualModelSaved) => (ToastKind::Success, (self.s.toast_vm_saved)(&name)),
            Err(message) => (ToastKind::Error, (self.s.toast_mutation_failed)(&name, message)),
        };
        self.push_toast(Toast::new(kind, text.clone()));
        if let BusyKey::Subscription(id) = key {
            self.last_outcome.insert(id, (kind, text));
        }
        refetch_cmds
    }

    pub fn update(&mut self, action: Action) -> Vec<Cmd> {
        match action {
            Action::Quit => self.guard_dirty(Action::Quit, |_| vec![Cmd::Quit]),
            Action::ForceQuit => vec![Cmd::Quit],
            Action::SwitchTab(to) => self.guard_dirty(Action::SwitchTab(to), move |app| {
                let dir = if to.index() > app.tab.index() { Dir::Forward } else { Dir::Backward };
                app.switch_tab(to, dir)
            }),
            Action::NextTab => self.guard_dirty(Action::NextTab, |app| {
                let to = app.tab.next();
                app.switch_tab(to, Dir::Forward)
            }),
            Action::PrevTab => self.guard_dirty(Action::PrevTab, |app| {
                let to = app.tab.prev();
                app.switch_tab(to, Dir::Backward)
            }),
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
            Action::OpenConfirm { prompt, on_yes } => {
                self.open_confirm(prompt, on_yes);
                Vec::new()
            }
            Action::Confirmed(inner) => {
                self.close_popup();
                self.pages.get_mut(self.tab).discard_changes();
                self.update(*inner)
            }
            Action::DiscardDraft => {
                self.pages.get_mut(self.tab).discard_changes();
                Vec::new()
            }
            Action::Tick { now_ms } => {
                self.now_ms = now_ms;
                self.tick += 1;
                while self.toasts.front().is_some_and(|t| t.expired(now_ms)) {
                    self.toasts.pop_front();
                }
                if self.conn == Conn::Connected && self.tick.is_multiple_of(POLL_EVERY_TICKS) {
                    self.pages.get_mut(self.tab).update(&Action::Refresh, &self.store)
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
                self.pages.get_mut(self.tab).update(&action, &self.store)
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
                    self.pages.overview.update(&action, &self.store)
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
                // 虚拟模型列表刷新: 只进 Store, 不需要转给任何页面的 update ——虚拟模型页 (Task 6 起)
                // 画的时候直接读 ctx.store, 与订阅列表同一套约定。
                Ok(FetchData::VirtualModels(vms)) => {
                    debug_assert_eq!(fetch, Fetch::VirtualModels, "spawn_fetch 应该保证 FetchData::VirtualModels 只配 Fetch::VirtualModels");
                    self.store.apply_virtual_models(issued, vms);
                    Vec::new()
                }
            },
            Action::Refresh | Action::Sse { .. } => self.pages.get_mut(self.tab).update(&action, &self.store),
            Action::Mutate(m) => self.start_mutation(m),
            Action::MutationDone { mutation, barrier, result } => self.finish_mutation(mutation, barrier, result),
            Action::OpenPicker(spec) => {
                self.open_picker(spec);
                Vec::new()
            }
            Action::PickerDone { tag, choice } => {
                self.close_popup();
                self.pages.get_mut(self.tab).update(&Action::PickerDone { tag, choice }, &self.store)
            }
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
        let page = self.pages.get_mut(self.tab);
        page.draw(frame, content, &mut ctx);
        let mut left = page.hints(s);
        left.insert(0, ("1-5", s.key_switch_tab));
        keybar::draw(frame, footer, &left, &[("?", s.key_help), ("q", s.key_quit)], &self.theme);

        self.draw_toast(frame, screen, content.y);

        if let Some(popup) = &mut self.popup {
            // 压暗背景用静态的 DIM 修饰符而不是动效: 16 色 / 无色终端下同样成立。
            frame.buffer_mut().set_style(screen, Style::new().add_modifier(Modifier::DIM));
            // 穷尽 match: 以后加新弹窗变体忘了在这里接住会编译失败。
            let area = match popup {
                Popup::Help => {
                    let page_rows = self.pages.get(self.tab).help(s);
                    let area = help::area(screen, s, page_rows);
                    help::draw(frame, area, &self.theme, s, page_rows);
                    area
                }
                Popup::Confirm(state) => {
                    let area = confirm::area(screen, &state.prompt);
                    confirm::draw(frame, area, state, &self.theme, s);
                    area
                }
                Popup::Picker(state) => {
                    let area = picker::area(screen);
                    picker::draw(frame, area, state, &self.theme, s);
                    area
                }
            };
            self.popup_area = Some(area);
        }

        if !self.startup_played {
            self.startup_played = true;
            self.fx.startup(self.pages.overview.logo_area().unwrap_or_default(), screen, self.theme.border);
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

// 「未保存修改 → 先确认」流程靠 `pages::placeholder::Placeholder` 的 `#[cfg(test)]` 专用钩子
// (`set_force_dirty`) 验证——那个钩子只在编译本 crate 的单测时存在 (见该文件顶部的注释),
// `tests/ui.rs` 这样的集成测试够不到它, 所以这批用例必须留在这个 `mod tests` 里, 不能挪到
// `tests/ui.rs`。不需要这个钩子的弹窗测试 (帮助弹窗按键、确认弹窗渲染、帮助高度守卫、快照) 仍然
// 放在 `tests/ui.rs`, 跟其它界面测试一起维护。
#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::*;
    use crate::action::Tab;
    use crate::i18n::ZH;

    const NOW: i64 = 1_700_000_000_000;
    const VERSION: &str = "9.9.9";

    fn app() -> App {
        App::new(AppOptions { strings: &ZH, theme: Theme::new(crate::theme::ColorMode::TrueColor), fx_enabled: false, now_ms: NOW, tui_version: VERSION })
    }

    /// 切到一个占位页 (`Tab::VirtualModels`) 并把它标记成 dirty——三个占位 tab 共用同一个
    /// `Placeholder` 实例, 选哪个不影响测试意图。
    fn dirty_app() -> App {
        let mut a = app();
        a.update(Action::SwitchTab(Tab::VirtualModels));
        a.pages.placeholder.set_force_dirty(true);
        a
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl_c() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
    }

    fn render(a: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| a.draw(f, Duration::ZERO)).unwrap();
        terminal.backend().to_string()
    }

    #[test]
    fn ctrl_c_quits_even_when_dirty_or_with_a_popup_open() {
        let mut a = dirty_app();
        assert_eq!(a.handle_key(ctrl_c()), Some(Action::ForceQuit), "dirty 页面上 Ctrl+C 也不该问");
        assert_eq!(a.update(Action::ForceQuit), vec![Cmd::Quit]);

        // 弹窗打开时 (比如已经在问「要不要放弃」) Ctrl+C 依然立即退出, 不再多问一次。
        let mut b = dirty_app();
        b.update(Action::Quit);
        assert!(b.popup.is_some(), "上一步应该已经弹出确认弹窗");
        assert_eq!(b.handle_key(ctrl_c()), Some(Action::ForceQuit));
        assert_eq!(b.update(Action::ForceQuit), vec![Cmd::Quit]);
    }

    #[test]
    fn q_on_a_dirty_page_asks_first() {
        let mut a = dirty_app();
        assert_eq!(a.handle_key(key(KeyCode::Char('q'))), Some(Action::Quit));
        assert!(a.update(Action::Quit).is_empty(), "dirty 页面上 q 不该立即退出");
        let out = render(&mut a, 80, 24);
        assert!(out.contains(ZH.confirm_discard), "应该弹出确认提示\n{out}");

        assert_eq!(a.handle_key(key(KeyCode::Char('y'))), Some(Action::Confirmed(Box::new(Action::Quit))));
        assert_eq!(a.update(Action::Confirmed(Box::new(Action::Quit))), vec![Cmd::Quit], "y 之后应该真的退出");

        // 另起一局: n 应该只关掉弹窗, 页面仍然 dirty (草稿没被丢弃, 也没有退出)。
        let mut b = dirty_app();
        b.update(Action::Quit);
        assert_eq!(b.handle_key(key(KeyCode::Char('n'))), Some(Action::ClosePopup));
        assert!(b.update(Action::ClosePopup).is_empty());
        assert!(b.popup.is_none(), "n 应该关掉弹窗");
        assert!(b.pages.get(b.tab).is_dirty(), "n 不该丢弃草稿");
    }

    #[test]
    fn switching_tabs_on_a_dirty_page_asks_first_and_yes_discards() {
        let mut a = dirty_app();
        assert!(a.update(Action::SwitchTab(Tab::Overview)).is_empty(), "dirty 时切页应该先确认");
        assert_eq!(a.tab, Tab::VirtualModels, "确认之前不该真的切走");
        assert!(a.pages.placeholder.is_dirty());

        a.update(Action::Confirmed(Box::new(Action::SwitchTab(Tab::Overview))));
        assert_eq!(a.tab, Tab::Overview, "y 之后应该真的切过去");
        assert!(!a.pages.placeholder.is_dirty(), "y 之后原页面的草稿应该被丢弃");
    }

    /// Fix round E: `guard_dirty` 之前只有 `Quit` / `SwitchTab` 两条路径被测过, `NextTab` /
    /// `PrevTab` 走的是同一个 `guard_dirty` helper, 但从没被单独断言过。
    #[test]
    fn next_tab_and_prev_tab_on_a_dirty_page_ask_first_and_yes_discards() {
        let mut a = dirty_app(); // tab = VirtualModels (index 2)
        assert!(a.update(Action::NextTab).is_empty(), "dirty 时 NextTab 应该先确认");
        assert_eq!(a.tab, Tab::VirtualModels, "确认之前不该真的切走");
        a.update(Action::Confirmed(Box::new(Action::NextTab)));
        assert_eq!(a.tab, Tab::Live, "y 之后应该真的切到下一页");

        let mut b = dirty_app();
        assert!(b.update(Action::PrevTab).is_empty(), "dirty 时 PrevTab 应该先确认");
        assert_eq!(b.tab, Tab::VirtualModels);
        b.update(Action::Confirmed(Box::new(Action::PrevTab)));
        assert_eq!(b.tab, Tab::Subscriptions, "y 之后应该真的切到上一页");
    }

    /// Fix round E: `Action::DiscardDraft` (页面自己按 Esc 放弃编辑时用) 应该直接调用当前页面的
    /// `discard_changes()`, 不产出任何 `Cmd`, 也不需要经过确认弹窗——这条路径以前没有专门测过。
    #[test]
    fn discard_draft_clears_the_current_pages_dirty_flag_without_a_cmd() {
        let mut a = dirty_app();
        assert!(a.pages.placeholder.is_dirty());
        assert!(a.update(Action::DiscardDraft).is_empty(), "不应该产出任何 Cmd");
        assert!(!a.pages.placeholder.is_dirty(), "草稿应该被丢弃");
    }

    /// Fix round C: 打开一个新弹窗 (确认 / picker) 时, 如果已经有另一个弹窗开着, 应该直接替换它,
    /// 并且清掉旧弹窗的 `popup_area`——不清的话, 如果新弹窗在第一次 `draw()` 之前就被关掉, 关闭
    /// 动效会拿旧弹窗的几何去播。
    #[test]
    fn opening_a_popup_while_another_is_open_replaces_it_and_resets_popup_area() {
        let mut a = app();
        a.update(Action::ToggleHelp);
        assert!(matches!(a.popup, Some(Popup::Help)));
        render(&mut a, 80, 24); // 让 App 记住 Help 弹窗的 popup_area。
        assert!(a.popup_area.is_some());

        a.update(Action::OpenConfirm { prompt: "测试".into(), on_yes: Box::new(Action::Refresh) });
        assert!(matches!(a.popup, Some(Popup::Confirm(_))), "应该直接替换成确认弹窗");
        assert!(a.popup_area.is_none(), "替换时应该清掉旧弹窗的 popup_area, 不留到下一次关闭时误用");

        render(&mut a, 80, 24); // 再记一次, 这次是 Confirm 弹窗的 popup_area。
        assert!(a.popup_area.is_some());
        a.update(Action::OpenPicker(picker::PickerSpec {
            tag: picker::PickerTag::VmAddSubscription,
            title: "选择".into(),
            items: vec![],
            allow_custom: false,
            initial: String::new(),
        }));
        assert!(matches!(a.popup, Some(Popup::Picker(_))), "应该直接替换成 picker 弹窗");
        assert!(a.popup_area.is_none(), "同样应该清掉 Confirm 弹窗的 popup_area");
    }
}
