//! 虚拟模型页: 左 5 个虚拟模型 / 右选中虚拟模型的有序订阅。支持重排序 (`J`/`K`)、加入 (`a`,
//! picker 选未绑定的订阅)、移除 (`x`)、切换调度模式 (`m`)、保存 (`s`)。
//!
//! 与订阅详情页 (Task 5, `subscriptions.rs`) 同一套草稿模式 (D3 起收进共用的 [`super::draft::Draft`]):
//! 页面自己的 [`VmDraft`], 首次编辑时从 `Store` 克隆, 与 `Store` 当前值完全相等则立刻丢弃, 只有
//! 成功保存才清空。**与订阅页不同的一点**: 布局不随终端宽度变化 (左 32 列固定, 右吃剩余), 因为
//! 虚拟模型固定只有 5 个, 不需要按宽度切一栏/两栏。

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, List, ListItem, ListState, Padding};
use ratatui::Frame;
use throbber_widgets_tui::{Throbber, BRAILLE_SIX};

use super::draft::Draft;
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
use crate::widgets::toast::ToastKind;
use crate::widgets::{pane_border_style, spinner_state};

/// 左栏固定宽度 (brief: 「所有宽度同一种：左 32 列，右吃剩余」)——虚拟模型固定只有 5 个,
/// 不需要像订阅页那样按终端宽度切一栏/两栏。
const LEFT_WIDTH: u16 = 32;
/// 最长的虚拟模型名是 "model-fallback" (14 列) + 草稿标记 " *" (2 列) = 16, 留 1 列余量 (V5,
/// fix round P3b: 草稿存在时左栏列表行也要显示 `*`, 因为 toast 会挡住右栏标题上的那颗)。
const MODEL_NAME_COL: usize = 17;
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

/// [`Draft::edit`]/[`Draft::sync`]/[`Draft::refresh_dirty`] 要求的 base: 这个虚拟模型在 `Store`
/// 里当前的值。
fn vm_draft_base(vm: &VirtualModel) -> VmDraft {
    VmDraft { name: vm.name.clone(), mode: vm.mode, subscription_ids: vm.subscription_ids.clone() }
}

pub struct VirtualModels {
    /// 左栏选中的虚拟模型下标 (0..5, 后端固定顺序 fable/opus/sonnet/haiku/fallback)。
    selected_index: usize,
    focus: VmFocus,
    /// 右栏 (成员列表) 的光标下标, 相对当前选中的虚拟模型; 换选中项时归零。
    members_cursor: usize,
    /// 当前正在编辑的虚拟模型草稿; `None` = 没有未保存的修改。D2/D3 (fix round P3b) 起
    /// `draft.is_some()` 与 `is_dirty()` 恒等价——零编辑 (空列表上的 `x`/`J`/`K`、到头的
    /// `J`/`K`) 或者改回原值都不会留下草稿, 见 [`Draft::edit`]。
    draft: Draft<VmDraft>,
    /// I1 (fix round final): 正在保存中的虚拟模型名 (`Mutation::UpdateVirtualModel` 从
    /// `on_mutation_started` 到对应 `on_mutation_done` 之间); `Some` 时拒绝任何会继续修改草稿的
    /// 按键 (含再按一次 `s`), 与订阅详情页 `Subscriptions::saving` 同一套道理。
    saving: Option<String>,
    /// 下一帧要闪一下的订阅 id; `draw` 取走。
    flash_rows: Vec<String>,
    /// 页面在 `update()` 内部想弹的一条 toast, `App::update_page` 在调用 `update()` 之后轮询取走
    /// (与订阅详情页 `Subscriptions::pending_notice` 同一套机制)。
    pending_notice: Option<(ToastKind, String)>,
}

impl Default for VirtualModels {
    fn default() -> Self {
        Self {
            selected_index: 0,
            focus: VmFocus::Models,
            members_cursor: 0,
            draft: Draft::default(),
            saving: None,
            flash_rows: Vec::new(),
            pending_notice: None,
        }
    }
}

impl VirtualModels {
    fn pane_border_style(&self, theme: &Theme, is_left: bool) -> Style {
        let left_focused = matches!(self.focus, VmFocus::Models);
        pane_border_style(theme, is_left == left_focused)
    }

    /// I1: 当前选中的虚拟模型是否正有一次 `UpdateVirtualModel` 保存在飞行中。
    fn is_saving(&self) -> bool {
        self.saving.is_some()
    }

    fn saving_notice(s: &'static Strings) -> Action {
        Action::Notify { kind: ToastKind::Info, text: s.saving_in_progress.to_string() }
    }

    /// 当前应该显示的调度模式: 有草稿 (且属于这个虚拟模型) 就用草稿, 否则用 `Store` 里的原始值。
    fn effective_mode(&self, vm: &VirtualModel) -> RoutingMode {
        match self.draft.get() {
            Some(d) if d.name == vm.name => d.mode,
            _ => vm.mode,
        }
    }

    fn effective_subscription_ids<'a>(&'a self, vm: &'a VirtualModel) -> &'a [String] {
        match self.draft.get() {
            Some(d) if d.name == vm.name => &d.subscription_ids,
            _ => &vm.subscription_ids,
        }
    }

    /// `draw()` 专用: 只重算 `is_dirty()` 的缓存, 不碰 `draft`/`focus`——保证「同一状态画两次
    /// 得到同一帧」不受影响 (与订阅页 `refresh_dirty_flag` 同一条道理, 见 [`Draft::refresh_dirty`]
    /// 与 [`Draft::sync`] 的分工说明)。
    fn refresh_dirty_flag(&mut self, store: &Store) {
        let base = self.draft.get().and_then(|d| store.virtual_models().iter().find(|vm| vm.name == d.name)).map(vm_draft_base);
        self.draft.refresh_dirty(base.as_ref());
    }

    /// `update()` 专用: 真正核对一遍草稿是否已经与 `Store` 当前值相等 (D2, 改回原值 / 别的客户端
    /// 把 `Store` 改成了跟草稿一样都算) 就丢弃——虚拟模型固定只有 5 个, 理论上不会真的「消失」,
    /// 但仍然按 `Draft::sync` 的通用语义处理 (base 找不到就丢弃), 不额外弹通知/挪焦点。
    fn sync_draft_with_store(&mut self, store: &Store) {
        let Some(name) = self.draft.get().map(|d| d.name.clone()) else { return };
        let base = store.virtual_models().iter().find(|vm| vm.name == name).map(vm_draft_base);
        self.draft.sync(base.as_ref());
    }

    /// `Models` 焦点下 `↑↓`/`jk`: 有草稿时先确认 (`on_yes: DiscardDraft`, 确认后停在原位,
    /// 用户再按一次移动); 没有草稿 (或草稿已经改回原值) 才真的移动选中项, 并清空 `members_cursor`
    /// (换了一个虚拟模型, 成员列表光标该从头开始)。
    fn move_model_selection(&mut self, vms: &[VirtualModel], idx: usize, delta: isize, s: &'static Strings) -> Option<Action> {
        if self.is_dirty() {
            return Some(Action::OpenConfirm { prompt: s.confirm_discard.to_string(), on_yes: Box::new(Action::DiscardDraft) });
        }
        self.draft.clear();
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
            // I5: 带上这次弹窗是为哪个虚拟模型开的, `PickerDone` 落地时据此核对是否还该应用。
            tag: PickerTag::VmAddSubscription { vm: vm.name.clone() },
            title: (s.vm_pick_add_title)(&vm.name),
            items,
            allow_custom: false,
            initial: String::new(),
        })
    }

    /// `PickerDone { tag: VmAddSubscription { vm }, .. }` 落地: 追加到草稿末尾, 光标跟到它。
    /// `PickerChoice::Custom` 理论上不会发生 (`allow_custom: false`), 防御性地忽略。I5: `vm` 对不
    /// 上当前选中的虚拟模型就静默忽略 (理论上不该发生, 虚拟模型固定只有 5 个且不能在有草稿时切换
    /// 选中项, 但防御性地核对一次); I1: 这个虚拟模型正有保存在飞行中时拒绝, 弹 `saving_in_progress`
    /// (存进 `pending_notice`, 与订阅页 `apply_picker_choice` 同一套「`update()` 内部想弹通知」机制)。
    fn apply_add_choice(&mut self, vm_name: &str, choice: &PickerChoice, store: &Store, s: &'static Strings) {
        let PickerChoice::Item(id) = choice else { return };
        let vms = store.virtual_models();
        if vms.is_empty() {
            return;
        }
        let idx = self.selected_index.min(vms.len() - 1);
        let vm = &vms[idx];
        if vm.name != vm_name {
            return;
        }
        if self.is_saving() {
            self.pending_notice = Some((ToastKind::Info, s.saving_in_progress.to_string()));
            return;
        }
        let base = vm_draft_base(vm);
        self.draft.edit(&base, |d| d.subscription_ids.push(id.clone()));
        self.members_cursor = self.effective_subscription_ids(vm).len() - 1;
    }

    /// `s`: 不脏时无动作; 草稿的调度模式是 `Unknown` (后端某天加的新模式, `as_wire()` 会静默降级
    /// 成 `"sequential"`) 时拒绝保存, 不能让用户在不知情的情况下把它发回后端。V2 (fix round P3b):
    /// 草稿里如果还有 `Store` 找不到的 id (「已删除」的订阅) 也拒绝——发给后端只会换回一句裸 uuid
    /// 的英文报错, 不如就地引导用户先按 `x` 移除。断线 / 忙碌由 `App::start_mutation` 统一处理,
    /// 这里不用重复判断。
    fn save_action(&self, s: &'static Strings, store: &Store) -> Option<Action> {
        if !self.is_dirty() {
            return None;
        }
        let draft = self.draft.get()?;
        // I5: 防御性地要求草稿的 `name` 等于当前选中项 (与订阅详情页 `save_action` 同一条道理)——
        // 理论上不该发生 (选中项在有草稿时不能被换掉, 见 `move_model_selection`), 但绝不能把一个
        // 不在屏幕上的虚拟模型的草稿发出去。
        let vms = store.virtual_models();
        let selected_name = vms.get(self.selected_index.min(vms.len().saturating_sub(1))).map(|vm| vm.name.as_str());
        if selected_name != Some(draft.name.as_str()) {
            return None;
        }
        if draft.mode == RoutingMode::Unknown {
            return Some(Action::Notify { kind: ToastKind::Info, text: s.vm_unknown_mode.to_string() });
        }
        if draft.subscription_ids.iter().any(|id| store.subscription(id).is_none()) {
            return Some(Action::Notify { kind: ToastKind::Info, text: s.vm_remove_ghosts_first.to_string() });
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
                // V5 (fix round P3b): 草稿存在时左栏这一行也带 `*`——右栏标题上的那颗会被 80 列
                // 下贴右边缘的 toast 挡住, 左栏这颗不会 (toast 只贴右边)。
                let is_dirty_here = self.draft.get().is_some_and(|d| d.name == vm.name);
                let name = if is_dirty_here { format!("{} *", vm.name) } else { vm.name.clone() };
                let line = Line::from(vec![
                    Span::raw(fit(&name, MODEL_NAME_COL)),
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
        let is_dirty_here = self.draft.get().is_some_and(|d| d.name == vm.name);

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
        // I4: 订阅列表还没加载完 (`list_subscriptions` 还没回来, 或者一直失败) 时, `store.subscription`
        // 对任何 id 都会返回 `None`——这不代表这些订阅"已删除", 只是这一刻还不知道。区分这两种情况,
        // 免得每个成员在页面刚打开、还没等到第一次订阅列表加载完成的那几百毫秒里全部被误标成
        // `vm_missing`, 顺带把 `s`/`a`/`x`/`J`/`K` 都指向"请先移除已删除的订阅"这种具有误导性的提示。
        let subs_loaded = store.subscriptions_loaded();
        let mut items: Vec<ListItem> = Vec::with_capacity(ids.len());
        for (i, id) in ids.iter().enumerate() {
            let mut spans = vec![Span::styled(format!("{:>2} ", i + 1), theme.muted_style())];
            match store.subscription(id) {
                Some(sub) => {
                    let b = badge(sub, theme, s);
                    spans.push(Span::styled(fit(b.symbol, MEMBER_SYMBOL_COL), Style::new().fg(b.color)));
                    spans.push(Span::raw(fit(&sub.display_name, MEMBER_NAME_COL)));
                    spans.push(Span::raw(fit(&sub.provider_display_name, MEMBER_PROVIDER_COL)));
                    // V3 (fix round P3b): 与后端 `ModelSlots::fallback_model()` 的 `trim()` 规则
                    // 对齐——纯空白的兜底槽视为未配置, 否则这里会漏标一条实际会被 pipeline 跳过的
                    // 订阅。
                    if is_fallback && sub.auth_type != "api_key" && sub.model_slots.fallback.trim().is_empty() {
                        spans.push(Span::styled(s.vm_will_skip, Style::new().fg(theme.warn)));
                    }
                }
                None if !subs_loaded => {
                    // I4: 还不知道这条订阅是否存在——只显示 id 前 8 位, 不带 `vm_missing` (不能
                    // 断言"已删除")。
                    let prefix: String = id.chars().take(8).collect();
                    spans.push(Span::styled(fit("?", MEMBER_SYMBOL_COL), theme.muted_style()));
                    spans.push(Span::styled(fit(&prefix, MEMBER_NAME_COL), theme.muted_style()));
                }
                None => {
                    // V2 (fix round P3b): 订阅在 Store 里找不到 (被别处删除) 这一行——之前是手写
                    // 拼接 (无分隔符、不走 `fit`), 跟正常行的列对不齐; 现在走同一个 `fit` 列, 只是
                    // 内容换成「id 前 8 位 + vm_missing」, 整行 muted。
                    let prefix: String = id.chars().take(8).collect();
                    let label = format!("{prefix} {}", s.vm_missing);
                    spans.push(Span::styled(fit("?", MEMBER_SYMBOL_COL), theme.muted_style()));
                    spans.push(Span::styled(fit(&label, MEMBER_NAME_COL), theme.muted_style()));
                }
            }
            items.push(ListItem::new(Line::from(spans)));
        }

        let list = List::new(items).highlight_symbol("▌ ").highlight_style(Style::new().add_modifier(Modifier::REVERSED));
        let mut state = ListState::default();
        let cursor = self.members_cursor.min(ids.len() - 1);
        state.select(Some(cursor));
        frame.render_stateful_widget(list, inner, &mut state);

        // V4 (fix round P3b): 成员超过面板高度时 `List` 会自动滚动——只有渲染完之后 `state.offset()`
        // 才知道真实的可视窗口在哪; 用没经过 offset 换算的下标算 rect 会闪错行 / 闪到面板外
        // (`inner.y + i` 在滚动之后完全对不上屏幕上的实际行), 与订阅页 `draw_list` 渲染完
        // `table_state` 之后再读 `offset()` 是同一个道理。
        let offset = state.offset();
        let capacity = inner.height as usize;
        for (i, id) in ids.iter().enumerate() {
            if flash_rows.contains(id) {
                if let Some(rect) = member_flash_rect(inner, offset, capacity, i) {
                    let color = store.subscription(id).map(|sub| badge(sub, theme, s).color).unwrap_or(theme.muted);
                    ctx.fx.row_changed(id, rect, color);
                }
            }
        }
    }
}

/// V4: 第 `i` 个成员这一帧是否落在可视窗口 (`offset`..`offset+capacity`) 内, 是则返回它相对
/// `inner` 的行 `Rect` (`y` 已经按 `offset` 换算过, 保证落在 `inner` 高度以内), 不在窗口内返回
/// `None`——纯函数, 拆出来单独测试几何计算本身, 不需要真的渲染一帧。
fn member_flash_rect(inner: Rect, offset: usize, capacity: usize, i: usize) -> Option<Rect> {
    if i < offset || i >= offset + capacity {
        return None;
    }
    Some(Rect::new(inner.x, inner.y + (i - offset) as u16, inner.width, 1))
}

impl Component for VirtualModels {
    fn handle_key(&mut self, key: KeyEvent, store: &Store, s: &'static Strings) -> Option<Action> {
        let vms = store.virtual_models();
        if vms.is_empty() {
            return None;
        }
        let idx = self.selected_index.min(vms.len() - 1);
        let vm = &vms[idx];
        // I4: 订阅列表还没加载完 (或一直加载失败) 时, `a`/`x`/`J`/`K`/`s` 都依赖它才能判断"这个 id
        // 是不是真的已删除" / "能不能加入", 统一拒绝——不能在这段时间把找不到的 id 误判成"已删除"。
        let subs_loaded = store.subscriptions_loaded();

        match self.focus {
            VmFocus::Models => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.move_model_selection(vms, idx, -1, s),
                KeyCode::Down | KeyCode::Char('j') => self.move_model_selection(vms, idx, 1, s),
                // M6: 现在读作「成员」(见 `Strings::key_members`), 更准确地描述这个键的作用。
                KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                    self.focus = VmFocus::Members;
                    None
                }
                // 「m 在 Models 焦点下也可用（作用于选中的虚拟模型，同样产生草稿）」。
                KeyCode::Char('m') => {
                    if self.is_saving() {
                        return Some(Self::saving_notice(s));
                    }
                    let base = vm_draft_base(vm);
                    self.draft.edit(&base, |d| d.mode = d.mode.next());
                    None
                }
                // I3: `s`/`Esc` 现在 Models 焦点下也可用, 不用先进 Members 才能保存/放弃——同一份
                // 草稿两个焦点都能碰到 (`m` 早就是这样), 保存/放弃理应对称。
                KeyCode::Char('s') => {
                    if self.is_saving() {
                        return Some(Self::saving_notice(s));
                    }
                    if !subs_loaded {
                        return Some(Action::Notify { kind: ToastKind::Info, text: s.vm_subs_not_loaded.to_string() });
                    }
                    self.save_action(s, store)
                }
                KeyCode::Esc if self.is_dirty() => {
                    Some(Action::OpenConfirm { prompt: s.confirm_discard.to_string(), on_yes: Box::new(Action::DiscardDraft) })
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
                    // D2(b) (fix round P3b): 到头 (或空列表) 不动——`cursor+1 < len`/`cursor > 0`
                    // 这两条边界判断现在只决定"要不要真的动 `members_cursor`", 草稿那边完全交给
                    // `Draft::edit` 自己核对结果是否等于 base (空列表 / 单项列表上的 no-op swap
                    // 结果必然与 base 相等, 会被自动丢弃, 不需要在这里重复判断一遍)。
                    KeyCode::Char('J') => {
                        if self.is_saving() {
                            return Some(Self::saving_notice(s));
                        }
                        if !subs_loaded {
                            return Some(Action::Notify { kind: ToastKind::Info, text: s.vm_subs_not_loaded.to_string() });
                        }
                        if cursor + 1 < len {
                            let base = vm_draft_base(vm);
                            self.draft.edit(&base, |d| d.subscription_ids.swap(cursor, cursor + 1));
                            self.members_cursor = cursor + 1;
                        }
                        None
                    }
                    KeyCode::Char('K') => {
                        if self.is_saving() {
                            return Some(Self::saving_notice(s));
                        }
                        if !subs_loaded {
                            return Some(Action::Notify { kind: ToastKind::Info, text: s.vm_subs_not_loaded.to_string() });
                        }
                        if cursor > 0 {
                            let base = vm_draft_base(vm);
                            self.draft.edit(&base, |d| d.subscription_ids.swap(cursor - 1, cursor));
                            self.members_cursor = cursor - 1;
                        }
                        None
                    }
                    KeyCode::Char('a') => {
                        if self.is_saving() {
                            return Some(Self::saving_notice(s));
                        }
                        if !subs_loaded {
                            return Some(Action::Notify { kind: ToastKind::Info, text: s.vm_subs_not_loaded.to_string() });
                        }
                        Some(self.open_add_picker(vm, store, s))
                    }
                    KeyCode::Char('x') => {
                        if self.is_saving() {
                            return Some(Self::saving_notice(s));
                        }
                        if !subs_loaded {
                            return Some(Action::Notify { kind: ToastKind::Info, text: s.vm_subs_not_loaded.to_string() });
                        }
                        let base = vm_draft_base(vm);
                        self.draft.edit(&base, |d| {
                            if cursor < d.subscription_ids.len() {
                                d.subscription_ids.remove(cursor);
                            }
                        });
                        // 空列表上按 `x`: `edit` 内部的比较让草稿保持 `None` (D2b), 这里从
                        // `effective_subscription_ids` (自动回落到 `vm` 的原值) 读新长度, 不用
                        // 关心到底有没有真的创建过草稿。
                        let new_len = self.effective_subscription_ids(vm).len();
                        self.members_cursor = if new_len == 0 { 0 } else { cursor.min(new_len - 1) };
                        None
                    }
                    KeyCode::Char('m') => {
                        if self.is_saving() {
                            return Some(Self::saving_notice(s));
                        }
                        let base = vm_draft_base(vm);
                        self.draft.edit(&base, |d| d.mode = d.mode.next());
                        None
                    }
                    KeyCode::Char('s') => {
                        if self.is_saving() {
                            return Some(Self::saving_notice(s));
                        }
                        if !subs_loaded {
                            return Some(Action::Notify { kind: ToastKind::Info, text: s.vm_subs_not_loaded.to_string() });
                        }
                        self.save_action(s, store)
                    }
                    KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => {
                        if self.is_dirty() {
                            Some(Action::OpenConfirm { prompt: s.confirm_discard.to_string(), on_yes: Box::new(Action::DiscardDraft) })
                        } else {
                            self.draft.clear();
                            self.focus = VmFocus::Models;
                            None
                        }
                    }
                    _ => None,
                }
            }
        }
    }

    fn update(&mut self, action: &Action, store: &Store, s: &'static Strings) -> Vec<Cmd> {
        self.sync_draft_with_store(store);
        match action {
            // 右栏要订阅名 / 厂商 / badge, 所以两个 Fetch 都要——被 `Fetches` 去重, 每 5 秒都发也
            // 没关系。
            Action::Refresh | Action::Connected { .. } => vec![Cmd::Fetch(Fetch::VirtualModels), Cmd::Fetch(Fetch::Subscriptions)],
            Action::Sse { name, .. } if SSE_REFETCH.contains(&name.as_str()) => vec![Cmd::Fetch(Fetch::Subscriptions)],
            Action::PickerDone { tag: PickerTag::VmAddSubscription { vm }, choice } => {
                self.apply_add_choice(vm, choice, store, s);
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
            // M6/I3 (fix round final): `⏎` 现在读作「成员」而不是「详情」(更准确); 脏时 `s 保存`
            // 挪到 `↑↓ 选择` 右边第一个, 紧跟着加一条 `Esc 放弃`——与 Members 焦点同一套优先级
            // (`s` 最不该被 80 列裁掉, `Esc` 次之), `m` 在 Models 焦点下也能造草稿 (I3 之前就是
            // 这样), 保存/放弃这两个键理应跟着它一起在这个焦点下也看得见。
            VmFocus::Models => {
                let mut hints = vec![("↑↓", s.key_select)];
                if self.is_dirty() {
                    hints.push(("s", s.key_save));
                    hints.push(("Esc", s.key_discard));
                }
                hints.push(("⏎", s.key_members));
                hints.push(("m", s.key_mode));
                if !self.is_dirty() {
                    hints.push(("s", s.key_save));
                }
                hints
            }
            VmFocus::Members => {
                // V1(b) (fix round P3b): 80 列放不下时 keybar 从右往左丢, `s` 原来排最后, `m` 反而
                // 先它一步留下——丢掉保存提示是最糟的裁剪结果。脏时把 `s` 挪到 `↑↓ 选择` 右边第一个
                // (保证它是最后才会被裁掉的那批), 不脏时留在原位 (跟着 `m` 之后, 视觉上更贴近
                // "调整完之后保存" 的顺序)。M6 (fix round final): 脏时紧跟着 `s` 再加一条
                // `Esc 放弃`, 与订阅详情页同一套规则。
                let mut hints = vec![("↑↓", s.key_select)];
                if self.is_dirty() {
                    hints.push(("s", s.key_save));
                    hints.push(("Esc", s.key_discard));
                }
                hints.push(("J K", s.key_move));
                hints.push(("a", s.key_add));
                hints.push(("x", s.key_remove));
                hints.push(("m", s.key_mode));
                if !self.is_dirty() {
                    hints.push(("s", s.key_save));
                }
                hints
            }
        }
    }

    fn help(&self, s: &'static Strings) -> &'static [(&'static str, &'static str)] {
        s.vm_help_rows
    }

    fn on_subscriptions_changed(&mut self, changed: &[String], _store: &Store, _s: &'static Strings) {
        self.flash_rows = changed.to_vec();
    }

    /// M1 (fix round final): `Store` 刚接受了一份 (可能与草稿恰好相等的) 新虚拟模型列表——这条
    /// action 只经过 `App::update` 里 `Ok(FetchData::VirtualModels(..))` 分支, 不会触发这个页面的
    /// `Component::update` (那条路径只更新 `Store`, 不转给任何页面), 所以旧版 `is_dirty()` 缓存要
    /// 等下一次真正的 `update()` (最多 5 秒的轮询) 才会被核对; 这里立刻核对一遍。
    fn on_store_changed(&mut self, store: &Store) {
        self.sync_draft_with_store(store);
    }

    fn is_dirty(&self) -> bool {
        self.draft.is_dirty()
    }

    fn discard_changes(&mut self) {
        self.draft.clear();
        self.focus = VmFocus::Models;
    }

    fn on_mutation_started(&mut self, mutation: &Mutation) {
        if let Mutation::UpdateVirtualModel { name, .. } = mutation {
            self.saving = Some(name.clone());
        }
    }

    fn on_mutation_done(&mut self, mutation: &Mutation, ok: bool) {
        if let Mutation::UpdateVirtualModel { name, .. } = mutation {
            if self.saving.as_deref() == Some(name.as_str()) {
                self.saving = None;
            }
        }
        if !ok {
            return;
        }
        // D1 (fix round P3b): 同订阅页——只有「保存时发出去的负载」与「结果落地这一刻的当前草稿」
        // 完全相等才清空, 用户在保存在途期间可能已经又重排/加减了一次成员。
        if let Mutation::UpdateVirtualModel { name, mode, subscription_ids } = mutation {
            if self.draft.get().is_some_and(|d| &d.name == name) {
                let saved = VmDraft { name: name.clone(), mode: *mode, subscription_ids: subscription_ids.clone() };
                if self.draft.matches(&saved) {
                    self.draft.clear();
                }
            }
        }
    }

    fn take_notice(&mut self) -> Option<(ToastKind, String)> {
        self.pending_notice.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// V4: 只覆盖窗口内的下标, 窗口外的 (滚出视野之前 / 还没滚到) 都该是 `None`; 窗口边界 (第一行 /
    /// 最后一行) 的 `y` 应该正确换算成相对 `inner` 的坐标, 不是原始下标。
    #[test]
    fn member_flash_rect_only_covers_the_visible_window() {
        let inner = Rect::new(3, 5, 20, 4); // 可视窗口 offset..offset+4
        assert_eq!(member_flash_rect(inner, 2, 4, 1), None, "offset=2: 下标 1 已经滚出视野之前");
        assert_eq!(member_flash_rect(inner, 2, 4, 2), Some(Rect::new(3, 5, 20, 1)), "窗口第一行, y 应该换算成 inner.y");
        assert_eq!(member_flash_rect(inner, 2, 4, 5), Some(Rect::new(3, 8, 20, 1)), "窗口最后一行");
        assert_eq!(member_flash_rect(inner, 2, 4, 6), None, "还没滚到的下标");
    }
}
