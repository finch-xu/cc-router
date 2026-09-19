//! 虚拟模型页: 左 5 个虚拟模型 / 右选中虚拟模型的有序订阅。支持重排序 (`J`/`K`)、加入 (`a`,
//! picker 选未绑定的订阅)、移除 (`x`)、切换调度模式 (`m`)、保存 (`s`)。
//!
//! 与订阅详情页 (Task 5, `subscriptions.rs`) 同一套草稿模式: 页面自己的 [`VmDraft`], 首次编辑时
//! 从 `Store` 克隆, 与 `Store` 当前值完全相等则不脏, 只有成功保存才清空。**与订阅页不同的一点**:
//! 布局不随终端宽度变化 (左 32 列固定, 右吃剩余), 因为虚拟模型固定只有 5 个, 不需要按宽度切一栏/
//! 两栏。

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, List, ListItem, ListState, Padding};
use ratatui::Frame;
use throbber_widgets_tui::{Throbber, BRAILLE_SIX};

use super::{Component, DrawCtx};
use crate::action::{Action, BusyKey, Cmd, Fetch, Mutation};
use crate::client::dto::{RoutingMode, VirtualModel};
use crate::format::fit;
use crate::i18n::Strings;
use crate::store::Store;
use crate::theme::Theme;
use crate::widgets::badge::badge;
use crate::widgets::keybar::Hint;
use crate::widgets::picker::{PickerChoice, PickerItem, PickerSpec, PickerTag};
use crate::widgets::spinner_state;
use crate::widgets::toast::ToastKind;

/// 左栏固定宽度 (brief: 「所有宽度同一种：左 32 列，右吃剩余」)——虚拟模型固定只有 5 个,
/// 不需要像订阅页那样按终端宽度切一栏/两栏。
const LEFT_WIDTH: u16 = 32;
/// 最长的虚拟模型名是 "model-fallback" (14 列), 留 1 列余量。
const MODEL_NAME_COL: usize = 15;
/// 模式短名 (顺序/轮询/会话/未知) 都是 2 个 CJK 字符 (显示宽度 4), 留 1 列余量。
const MODE_COL: usize = 5;
const MEMBER_SYMBOL_COL: usize = 2;
const MEMBER_NAME_COL: usize = 18;
const MEMBER_PROVIDER_COL: usize = 10;

const SSE_REFETCH: [&str; 2] = ["subscription_state_changed", "subscription_quota_reached"];

/// 两栏的键盘焦点。左右两栏一直都画 (不像订阅页窄屏时只画一栏), 焦点只影响哪一栏的边框是
/// `theme.accent`, 以及方向键作用在哪个列表上。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VmFocus {
    Models,
    Members,
}

/// 一个虚拟模型的编辑草稿: 调度模式 + 有序订阅列表, 整块替换 (与后端 `UpdateVirtualModelInput`
/// 的语义一致)。`name` 钉住是哪个虚拟模型, 只放页面自己的状态, 不进 `Store`。
#[derive(Debug, Clone, PartialEq)]
struct VmDraft {
    name: String,
    mode: RoutingMode,
    subscription_ids: Vec<String>,
}

pub struct VirtualModels {
    /// 左栏选中的虚拟模型下标 (0..5, 后端固定顺序 fable/opus/sonnet/haiku/fallback)。
    selected_index: usize,
    focus: VmFocus,
    /// 右栏 (成员列表) 的光标下标, 相对当前选中的虚拟模型; 换选中项时归零。
    members_cursor: usize,
    draft: Option<VmDraft>,
    /// `is_dirty()` 的缓存, 与 `Store` 当前值比较得出; 由 `refresh_dirty_flag` 在 `draw()` /
    /// `update()` 里保持更新 (同订阅页的 `dirty` 字段同一套约定)。
    dirty: bool,
    /// 下一帧要闪一下的订阅 id; `draw` 取走。
    flash_rows: Vec<String>,
}

impl Default for VirtualModels {
    fn default() -> Self {
        Self { selected_index: 0, focus: VmFocus::Models, members_cursor: 0, draft: None, dirty: false, flash_rows: Vec::new() }
    }
}

impl VirtualModels {
    fn pane_border_style(&self, theme: &Theme, is_left: bool) -> Style {
        let left_focused = matches!(self.focus, VmFocus::Models);
        if is_left == left_focused {
            Style::new().fg(theme.accent)
        } else {
            theme.border_style()
        }
    }

    /// 当前应该显示的调度模式: 有草稿 (且属于这个虚拟模型) 就用草稿, 否则用 `Store` 里的原始值。
    fn effective_mode(&self, vm: &VirtualModel) -> RoutingMode {
        match &self.draft {
            Some(d) if d.name == vm.name => d.mode,
            _ => vm.mode,
        }
    }

    fn effective_subscription_ids<'a>(&'a self, vm: &'a VirtualModel) -> &'a [String] {
        match &self.draft {
            Some(d) if d.name == vm.name => &d.subscription_ids,
            _ => &vm.subscription_ids,
        }
    }

    /// 草稿不存在, 或者存在但属于别的虚拟模型 (换了选中项之后按 `m`) 时新建一份 (从 `Store`
    /// 克隆); 已经存在且属于这个虚拟模型就直接复用。
    fn draft_mut(&mut self, vm: &VirtualModel) -> &mut VmDraft {
        let needs_new = self.draft.as_ref().is_none_or(|d| d.name != vm.name);
        if needs_new {
            self.draft = Some(VmDraft { name: vm.name.clone(), mode: vm.mode, subscription_ids: vm.subscription_ids.clone() });
        }
        match &mut self.draft {
            Some(d) => d,
            None => unreachable!("刚刚确保过 draft 是 Some"),
        }
    }

    /// 只重算 `dirty` 这个只读缓存, 不碰 `draft`/`focus`——`draw()`/`update()` 都调这个, 保证
    /// 「同一状态画两次得到同一帧」不受影响 (与订阅页 `refresh_dirty_flag` 同一条道理)。
    fn refresh_dirty_flag(&mut self, store: &Store) {
        self.dirty = match &self.draft {
            Some(d) => store
                .virtual_models()
                .iter()
                .find(|vm| vm.name == d.name)
                .is_some_and(|vm| vm.mode != d.mode || vm.subscription_ids != d.subscription_ids),
            None => false,
        };
    }

    /// `Models` 焦点下 `↑↓`/`jk`: 有草稿时先确认 (`on_yes: DiscardDraft`, 确认后停在原位,
    /// 用户再按一次移动); 没有草稿 (或草稿已经改回原值) 才真的移动选中项, 并清空 `members_cursor`
    /// (换了一个虚拟模型, 成员列表光标该从头开始)。
    fn move_model_selection(&mut self, vms: &[VirtualModel], idx: usize, delta: isize, s: &'static Strings) -> Option<Action> {
        if self.is_dirty() {
            return Some(Action::OpenConfirm { prompt: s.confirm_discard.to_string(), on_yes: Box::new(Action::DiscardDraft) });
        }
        self.draft = None;
        let next = (idx as isize + delta).clamp(0, vms.len() as isize - 1) as usize;
        self.selected_index = next;
        self.members_cursor = 0;
        None
    }

    /// `a`: 候选是 `Store` 里所有订阅中不在当前 (草稿) 成员列表里的那些; 没有候选就地回一条
    /// `Action::Notify`, 不开弹窗。
    fn open_add_picker(&self, vm: &VirtualModel, store: &Store, s: &'static Strings) -> Action {
        let current = self.effective_subscription_ids(vm);
        let items: Vec<PickerItem> = store
            .subscriptions()
            .iter()
            .filter(|sub| !current.contains(&sub.id))
            .map(|sub| PickerItem { id: sub.id.clone(), label: sub.display_name.clone(), hint: Some(sub.provider_display_name.clone()) })
            .collect();
        if items.is_empty() {
            return Action::Notify { kind: ToastKind::Info, text: s.vm_nothing_to_add.to_string() };
        }
        Action::OpenPicker(PickerSpec {
            tag: PickerTag::VmAddSubscription,
            title: (s.vm_pick_add_title)(&vm.name),
            items,
            allow_custom: false,
            initial: String::new(),
        })
    }

    /// `PickerDone { tag: VmAddSubscription, .. }` 落地: 追加到草稿末尾, 光标跟到它。
    /// `PickerChoice::Custom` 理论上不会发生 (`allow_custom: false`), 防御性地忽略。
    fn apply_add_choice(&mut self, choice: &PickerChoice, store: &Store) {
        let PickerChoice::Item(id) = choice else { return };
        let vms = store.virtual_models();
        if vms.is_empty() {
            return;
        }
        let idx = self.selected_index.min(vms.len() - 1);
        let vm = &vms[idx];
        let draft = self.draft_mut(vm);
        draft.subscription_ids.push(id.clone());
        self.members_cursor = draft.subscription_ids.len() - 1;
        self.refresh_dirty_flag(store);
    }

    /// `s`: 不脏时无动作; 草稿的调度模式是 `Unknown` (后端某天加的新模式, `as_wire()` 会静默降级
    /// 成 `"sequential"`) 时拒绝保存, 不能让用户在不知情的情况下把它发回后端。断线 / 忙碌由
    /// `App::start_mutation` 统一处理, 这里不用重复判断。
    fn save_action(&self, s: &'static Strings) -> Option<Action> {
        if !self.is_dirty() {
            return None;
        }
        let draft = self.draft.as_ref()?;
        if draft.mode == RoutingMode::Unknown {
            return Some(Action::Notify { kind: ToastKind::Info, text: s.vm_unknown_mode.to_string() });
        }
        Some(Action::Mutate(Mutation::UpdateVirtualModel {
            name: draft.name.clone(),
            mode: draft.mode,
            subscription_ids: draft.subscription_ids.clone(),
        }))
    }

    fn draw_loading(&self, frame: &mut Frame, area: Rect, ctx: &mut DrawCtx) {
        let s = ctx.s;
        let block =
            Block::bordered().border_type(BorderType::Rounded).border_style(ctx.theme.border_style()).title_top(format!(" {} ", s.vm_title));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let mut state = spinner_state(ctx.tick);
        let throbber = Throbber::default().label(s.loading).throbber_set(BRAILLE_SIX).style(ctx.theme.muted_style());
        frame.render_stateful_widget(throbber, inner, &mut state);
    }

    fn draw_models(&self, frame: &mut Frame, area: Rect, ctx: &DrawCtx, vms: &[VirtualModel], idx: usize, border_style: Style) {
        let s = ctx.s;
        let items: Vec<ListItem> = vms
            .iter()
            .map(|vm| {
                let mode = self.effective_mode(vm);
                let count = self.effective_subscription_ids(vm).len();
                let line = Line::from(vec![
                    Span::raw(fit(&vm.name, MODEL_NAME_COL)),
                    Span::raw(" "),
                    Span::raw(fit(s.vm_mode_short(mode), MODE_COL)),
                    Span::raw(format!("{count:>3}")),
                ]);
                ListItem::new(line)
            })
            .collect();
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(border_style)
            .title_top(format!(" {} ", s.vm_title))
            .padding(Padding::horizontal(1));
        let list = List::new(items).highlight_symbol("▌ ").highlight_style(Style::new().add_modifier(Modifier::REVERSED)).block(block);
        let mut state = ListState::default();
        state.select(Some(idx));
        frame.render_stateful_widget(list, area, &mut state);
    }

    fn draw_members(&self, frame: &mut Frame, area: Rect, ctx: &mut DrawCtx, vm: &VirtualModel, border_style: Style, flash_rows: &[String]) {
        let s = ctx.s;
        let theme = ctx.theme;
        let ids: Vec<String> = self.effective_subscription_ids(vm).to_vec();
        let mode = self.effective_mode(vm);
        let is_fallback = vm.name == "model-fallback";
        let is_dirty_here = self.is_dirty() && self.draft.as_ref().is_some_and(|d| d.name == vm.name);

        let title_suffix = if is_dirty_here { " *" } else { "" };
        let mut title_spans: Vec<Span<'static>> = vec![Span::raw(format!(" {}{} ", vm.name, title_suffix))];
        if ctx.busy.contains_key(&BusyKey::VirtualModel(vm.name.clone())) {
            let glyph = Throbber::default().throbber_set(BRAILLE_SIX).to_symbol_span(&spinner_state(ctx.tick));
            title_spans.push(Span::raw(glyph.content.to_string()));
            title_spans.push(Span::raw(" "));
        }

        let mode_full = s.vm_mode_full(mode);
        let summary = (s.vm_members_summary)(mode_full, ids.len());
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(border_style)
            .title_top(Line::from(title_spans))
            .title_bottom(Line::from(format!(" {summary} ")).right_aligned().style(theme.muted_style()))
            .padding(Padding::horizontal(1));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if ids.is_empty() {
            frame.render_widget(Line::styled(s.vm_empty, theme.muted_style()), inner);
            return;
        }

        let store = ctx.store;
        let mut items: Vec<ListItem> = Vec::with_capacity(ids.len());
        for (i, id) in ids.iter().enumerate() {
            let mut spans = vec![Span::styled(format!("{:>2} ", i + 1), theme.muted_style())];
            match store.subscription(id) {
                Some(sub) => {
                    let b = badge(sub, theme, s);
                    spans.push(Span::styled(fit(b.symbol, MEMBER_SYMBOL_COL), Style::new().fg(b.color)));
                    spans.push(Span::raw(fit(&sub.display_name, MEMBER_NAME_COL)));
                    spans.push(Span::raw(fit(&sub.provider_display_name, MEMBER_PROVIDER_COL)));
                    if is_fallback && sub.auth_type != "api_key" && sub.model_slots.fallback.is_empty() {
                        spans.push(Span::styled(s.vm_will_skip, Style::new().fg(theme.warn)));
                    }
                }
                None => {
                    // 订阅在 Store 里找不到 (被别处删除): 名字退化成 id 前 8 位 + `vm_missing`。
                    let prefix: String = id.chars().take(8).collect();
                    spans.push(Span::styled(fit("?", MEMBER_SYMBOL_COL), theme.muted_style()));
                    spans.push(Span::styled(format!("{prefix}{}", s.vm_missing), theme.muted_style()));
                }
            }
            items.push(ListItem::new(Line::from(spans)));
        }

        // 只对这一帧实际画出来的行触发闪烁 (成员列表不分页, 全部都在视野里)。
        for (i, id) in ids.iter().enumerate() {
            if flash_rows.contains(id) {
                let color = store.subscription(id).map(|sub| badge(sub, theme, s).color).unwrap_or(theme.muted);
                let rect = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
                ctx.fx.row_changed(id, rect, color);
            }
        }

        let list = List::new(items).highlight_symbol("▌ ").highlight_style(Style::new().add_modifier(Modifier::REVERSED));
        let mut state = ListState::default();
        let cursor = self.members_cursor.min(ids.len() - 1);
        state.select(Some(cursor));
        frame.render_stateful_widget(list, inner, &mut state);
    }
}

impl Component for VirtualModels {
    fn handle_key(&mut self, key: KeyEvent, store: &Store, s: &'static Strings) -> Option<Action> {
        let vms = store.virtual_models();
        if vms.is_empty() {
            return None;
        }
        let idx = self.selected_index.min(vms.len() - 1);
        let vm = &vms[idx];

        match self.focus {
            VmFocus::Models => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.move_model_selection(vms, idx, -1, s),
                KeyCode::Down | KeyCode::Char('j') => self.move_model_selection(vms, idx, 1, s),
                KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                    self.focus = VmFocus::Members;
                    None
                }
                // 「m 在 Models 焦点下也可用（作用于选中的虚拟模型，同样产生草稿）」。
                KeyCode::Char('m') => {
                    let mode = self.effective_mode(vm).next();
                    self.draft_mut(vm).mode = mode;
                    self.refresh_dirty_flag(store);
                    None
                }
                _ => None,
            },
            VmFocus::Members => {
                let len = self.effective_subscription_ids(vm).len();
                let cursor = self.members_cursor.min(len.saturating_sub(1));
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.members_cursor = cursor.saturating_sub(1);
                        None
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.members_cursor = if len == 0 { 0 } else { (cursor + 1).min(len - 1) };
                        None
                    }
                    // 到头不动 (不绕回)——去掉这条越界保护, `J` 在最后一项上会尝试
                    // `swap(cursor, cursor+1)` 越界 panic (Step 4 咬合检查 a)。
                    // 到头不动 (不绕回)——去掉这条越界保护, `J` 在最后一项上会尝试
                    // `swap(cursor, cursor+1)` 越界 panic (Step 4 咬合检查 a)。
                    KeyCode::Char('J') => {
                        let d = self.draft_mut(vm);
                        if cursor + 1 < d.subscription_ids.len() {
                            d.subscription_ids.swap(cursor, cursor + 1);
                            self.members_cursor = cursor + 1;
                        }
                        self.refresh_dirty_flag(store);
                        None
                    }
                    KeyCode::Char('K') => {
                        let d = self.draft_mut(vm);
                        if cursor > 0 {
                            d.subscription_ids.swap(cursor - 1, cursor);
                            self.members_cursor = cursor - 1;
                        }
                        self.refresh_dirty_flag(store);
                        None
                    }
                    KeyCode::Char('a') => Some(self.open_add_picker(vm, store, s)),
                    KeyCode::Char('x') => {
                        let d = self.draft_mut(vm);
                        if cursor < d.subscription_ids.len() {
                            d.subscription_ids.remove(cursor);
                        }
                        let new_len = d.subscription_ids.len();
                        self.members_cursor = if new_len == 0 { 0 } else { cursor.min(new_len - 1) };
                        self.refresh_dirty_flag(store);
                        None
                    }
                    KeyCode::Char('m') => {
                        let mode = self.effective_mode(vm).next();
                        self.draft_mut(vm).mode = mode;
                        self.refresh_dirty_flag(store);
                        None
                    }
                    KeyCode::Char('s') => self.save_action(s),
                    KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => {
                        if self.is_dirty() {
                            Some(Action::OpenConfirm { prompt: s.confirm_discard.to_string(), on_yes: Box::new(Action::DiscardDraft) })
                        } else {
                            self.draft = None;
                            self.focus = VmFocus::Models;
                            None
                        }
                    }
                    _ => None,
                }
            }
        }
    }

    fn update(&mut self, action: &Action, store: &Store, _s: &'static Strings) -> Vec<Cmd> {
        self.refresh_dirty_flag(store);
        match action {
            // 右栏要订阅名 / 厂商 / badge, 所以两个 Fetch 都要——被 `Fetches` 去重, 每 5 秒都发也
            // 没关系。
            Action::Refresh | Action::Connected { .. } => vec![Cmd::Fetch(Fetch::VirtualModels), Cmd::Fetch(Fetch::Subscriptions)],
            Action::Sse { name, .. } if SSE_REFETCH.contains(&name.as_str()) => vec![Cmd::Fetch(Fetch::Subscriptions)],
            Action::PickerDone { tag: PickerTag::VmAddSubscription, choice } => {
                self.apply_add_choice(choice, store);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, ctx: &mut DrawCtx) {
        let flash_rows = std::mem::take(&mut self.flash_rows);
        self.refresh_dirty_flag(ctx.store);

        if !ctx.store.virtual_models_loaded() {
            self.draw_loading(frame, area, ctx);
            return;
        }
        let vms = ctx.store.virtual_models();
        if vms.is_empty() {
            self.draw_loading(frame, area, ctx);
            return;
        }
        let idx = self.selected_index.min(vms.len() - 1);

        let [left, right] = Layout::horizontal([Constraint::Length(LEFT_WIDTH), Constraint::Min(0)]).areas(area);
        let left_border = self.pane_border_style(ctx.theme, true);
        let right_border = self.pane_border_style(ctx.theme, false);
        self.draw_models(frame, left, ctx, vms, idx, left_border);
        let vm = &vms[idx];
        self.draw_members(frame, right, ctx, vm, right_border, &flash_rows);
    }

    fn hints(&self, s: &'static Strings) -> Vec<Hint<'static>> {
        match self.focus {
            VmFocus::Models => vec![("↑↓", s.key_select), ("⏎", s.key_detail), ("m", s.key_mode)],
            VmFocus::Members => {
                vec![("↑↓", s.key_select), ("J K", s.key_move), ("a", s.key_add), ("x", s.key_remove), ("m", s.key_mode), ("s", s.key_save)]
            }
        }
    }

    fn help(&self, s: &'static Strings) -> &'static [(&'static str, &'static str)] {
        s.vm_help_rows
    }

    fn on_subscriptions_changed(&mut self, changed: &[String]) {
        self.flash_rows = changed.to_vec();
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn discard_changes(&mut self) {
        self.draft = None;
        self.dirty = false;
        self.focus = VmFocus::Models;
    }

    fn on_mutation_done(&mut self, mutation: &Mutation, ok: bool) {
        if !ok {
            return;
        }
        if let Mutation::UpdateVirtualModel { name, .. } = mutation {
            if self.draft.as_ref().is_some_and(|d| &d.name == name) {
                self.draft = None;
                self.dirty = false;
            }
        }
    }
}
