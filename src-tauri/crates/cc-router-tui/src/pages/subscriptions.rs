//! 订阅页: 列表 + 详情 (宽屏 ≥120 列双栏 / 窄屏进出详情), 加四个就地操作——启停 `e` / 测试连接
//! `t` / 刷新模型 `m` / 刷新余额 `b`, 列表态与详情态都生效, 作用于当前选中的订阅。
//!
//! Task 5 起详情态自己也是个小状态机 ([`Focus::Detail`] 带着当前槽位光标): `⏎` 改模型、`o` 改
//! 思考档位, 改动先落进页面自己的草稿 ([`SlotDraft`], 不进 `Store`), `s` 才真的发 `UpdateSlots`。

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Cell, Padding, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table, TableState, Wrap,
};
use ratatui::Frame;
use throbber_widgets_tui::{Throbber, BRAILLE_SIX};
use unicode_width::UnicodeWidthStr;

use super::draft::Draft;
use super::{Component, DrawCtx};
use crate::action::{Action, BusyKey, Cmd, Fetch, Mutation};
use crate::client::dto::{BalanceSeverity, ModelSlots, QuotaUsage, Slot, SlotEfforts, Subscription, EFFORT_CHOICES};
use crate::format::{compact, fit};
use crate::i18n::Strings;
use crate::store::Store;
use crate::theme::Theme;
use crate::widgets::badge::{badge, status_text};
use crate::widgets::gauge::quota_gauge;
use crate::widgets::keybar::Hint;
use crate::widgets::picker::{PickerChoice, PickerItem, PickerSpec, PickerTag};
use crate::widgets::toast::ToastKind;
use crate::widgets::{pane_border_style, spinner_state};

/// 达到才用左表右详情双栏; 以下只画一栏, 靠 [`Focus`] 在列表/详情之间切换 (两种宽度下 `⏎` 都能
/// 切换焦点, 区别只在窄屏一次只画一栏、宽屏两栏都画但边框颜色跟着焦点走)。
const WIDE_THRESHOLD: u16 = 120;
/// I3: 达到这个宽度, 左栏从 58 列放宽到 [`LIST_WIDTH_140`] 并显示状态列 (120–139 仍是 58 列
/// 无状态列, 与 [`WIDE_THRESHOLD`] 那档保持不变)。
const WIDE_140_THRESHOLD: u16 = 140;
const LIST_WIDTH: u16 = 58;
const LIST_WIDTH_140: u16 = 72;
/// 表的选中前缀 (`highlight_symbol`) 固定宽度, 用于手算列宽给 `format::fit`。
const HIGHLIGHT_COL: u16 = 2;
const SYMBOL_COL: u16 = 2;
const NAME_COL: usize = 20;
const PROVIDER_COL: usize = 12;
/// M6: 状态列 (badge 文案 + 冷却倒计时), 紧跟在备注名后面; 只在宽度够 (仍能留给 sonnet 列至少
/// 12 列) 才显示, 放不下就整列省略, 不挤压 name / provider / sonnet 的下限。
const STATUS_COL: usize = 14;
/// 列表左右各留一列空白, 不让内容贴着边框 (block 用 `Padding::horizontal`)。
const LIST_PADDING: u16 = 1;
const FIELD_LABEL_COL: usize = 10;
const SLOT_NAME_COL: usize = 8;
/// I2: 槽位 effort 那一列的定宽 (最长的档位文案是 "medium"/"xhigh", 5~6 列, 8 留了余量)。
/// 模型名列不再是常量, 改成按可用宽度算 (见 [`slot_model_col`])。
const EFFORT_COL: usize = 8;
/// 「最近错误」最多占的行数, 是上限不是固定分配 (`row_height` 按实际折行数留空间)。
const LAST_ERROR_ROWS: u16 = 4;
/// I1: 「上次操作」最多占的行数, 比「最近错误」少一行——它是补充信息, 不该比主字段还显眼。
const LAST_ACTION_ROWS: u16 = 3;
/// 两步向导没走完时槽位留下的占位模型名。
const PENDING_MODEL: &str = "(pending)";
/// `PageUp` / `PageDown` 在第一帧画出来之前没有真实的可视行数可用, 先给个不至于原地不动的默认值。
const DEFAULT_PAGE_ROWS: usize = 10;

const SSE_REFETCH: [&str; 2] = ["subscription_state_changed", "subscription_quota_reached"];

/// 详情面板的一行: 大多数是普通文本, 限额行要嵌一个真正的 `LineGauge` widget (不是文本能表示
/// 的), 「上次操作」/「最近错误」这类自由文本可能超宽折成好几行 (`Wrapped`)。`height` 在构造
/// 时就算好 (见 [`wrapped_row`]) 而不是画的时候现算——这样"占几行"的估算与真正截给
/// `Paragraph` 的文本严格来自同一份计算, 不会出现分配的空间和实际内容对不上的情况。
enum DetailRow {
    Line(Line<'static>),
    Quota { label: &'static str, quota: QuotaUsage },
    Wrapped { label: &'static str, text: String, height: u16, style: Style },
}

/// 列表 / 详情的键盘焦点 (Task 5)。两种宽度都有效: 宽屏两栏一直都画, 焦点只影响哪一栏的边框是
/// `theme.accent`; 窄屏一次只画一栏, 焦点直接决定画哪栏 (与旧的 `detail_open: bool` 同一件事,
/// 只是现在宽屏下也有意义)。`Detail` 带着当前槽位光标, 因为「进详情」与「选中第一个槽位」是
/// 同一个动作 (`⏎`/`→`/`l` 从 `List` 过来恒落在 [`Slot::Fable`])。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    List,
    Detail { slot: Slot },
}

/// 一条订阅槽位的编辑草稿。`sub_id` 钉住是哪一条订阅 (焦点在 `Detail` 时列表选中项不会变, 但
/// 防御性地存一份, 不隐式依赖「当前选中项没变过」这件事)。只放页面自己的状态, 不进 `Store`
/// (spec 全局约束)。
#[derive(Debug, Clone, PartialEq)]
struct SlotDraft {
    sub_id: String,
    model_slots: ModelSlots,
    slot_efforts: SlotEfforts,
}

/// [`Draft::edit`]/[`Draft::sync`]/[`Draft::refresh_dirty`] 要求的 base: 这条订阅在 `Store` 里
/// 当前的值, 包成跟草稿同一个形状才能直接比较相等。
fn slot_draft_base(sub: &Subscription) -> SlotDraft {
    SlotDraft { sub_id: sub.id.clone(), model_slots: sub.model_slots.clone(), slot_efforts: sub.slot_efforts.clone() }
}

/// 五个槽位的固定顺序, 给槽位光标的 上/下/首/尾 移动用。
const ALL_SLOTS: [Slot; 5] = [Slot::Fable, Slot::Opus, Slot::Sonnet, Slot::Haiku, Slot::Fallback];
/// 四个主槽 (不含兜底), 给详情面板画槽位行用——兜底槽单独一行 (没有 effort 列)。
const MAIN_SLOTS: [Slot; 4] = [Slot::Fable, Slot::Opus, Slot::Sonnet, Slot::Haiku];

pub struct Subscriptions {
    selected_id: Option<String>,
    /// `selected_id` 在新列表里找不到时, 用这个 (钳制到新列表长度后) 兜底, 而不是简单地弹回第一条。
    last_index: usize,
    focus: Focus,
    /// 上一帧的宽度: `draw_list`/`draw_detail` 布局判断要用得到, 但按键发生时还不知道这一帧的几何。
    last_width: u16,
    /// 上一帧表体的可视行数, `PageUp` / `PageDown` 按这个翻页; 首帧之前用 [`DEFAULT_PAGE_ROWS`]
    /// 兜底, 不然第一次按键 (还没画过) 只会移动 0 格 (F8 教训: 步长绝不能默认成 0)。
    last_page_rows: usize,
    table_state: TableState,
    /// 下一帧要闪一下的订阅 id; `draw` 取走。
    flash_rows: Vec<String>,
    /// 当前正在编辑的槽位草稿; `None` = 没有未保存的修改。首次编辑时从 `Store` 里对应订阅克隆,
    /// 与 `Store` 当前值相等 (改回原值 / 从没真的改过) 就立刻丢弃——D2/D3 (fix round P3b) 起这条
    /// 规则收进 [`Draft`] 内部, 不再是页面自己要记得维护的约定 (`draft.is_some()` ⇔ `is_dirty()`
    /// 恒成立)。
    draft: Draft<SlotDraft>,
    /// I1 (fix round final): 正在保存中的订阅 id (`Mutation::UpdateSlots` 从 `on_mutation_started`
    /// 到对应 `on_mutation_done` 之间); `Some` 时拒绝任何会继续修改草稿的按键 (含再按一次 `s`),
    /// 避免飞行中的编辑被落地的保存结果悄悄冲掉 (D1 只保证了草稿本身不丢, 但没有在编辑发生的那
    /// 一刻提示用户"现在编辑不安全")。
    saving: Option<String>,
    /// 页面在 `update()` 内部想弹的一条 toast, `App::update_page` 在调用 `update()` 之后轮询取走
    /// (`update()` 签名只能返回 `Vec<Cmd>`, 塞不进一个 `Action::Notify`)。
    pending_notice: Option<(ToastKind, String)>,
}

impl Default for Subscriptions {
    fn default() -> Self {
        Self {
            selected_id: None,
            last_index: 0,
            focus: Focus::List,
            last_width: 0,
            last_page_rows: DEFAULT_PAGE_ROWS,
            table_state: TableState::default(),
            flash_rows: Vec::new(),
            draft: Draft::default(),
            saving: None,
            pending_notice: None,
        }
    }
}

impl Subscriptions {
    fn is_wide(&self) -> bool {
        self.last_width >= WIDE_THRESHOLD
    }

    /// I3: 宽屏左栏的列宽——140 列起放宽到 72 (放得下状态列), 120–139 仍是 58 (与之前一致)。
    fn list_width(&self) -> u16 {
        if self.last_width >= WIDE_140_THRESHOLD {
            LIST_WIDTH_140
        } else {
            LIST_WIDTH
        }
    }

    /// 宽屏下有焦点的那一栏边框用 `theme.accent`, 另一栏用 `theme.border`; 窄屏一次只画一栏,
    /// 边框颜色的区分没有意义, 恒用 `theme.border` (与改动前一致)。「focused → accent, 否则
    /// border」这条颜色规则本身挪进了 `widgets::pane_border_style` (D3, 与虚拟模型页共用)。
    fn pane_border_style(&self, theme: &Theme, is_list_pane: bool) -> Style {
        if !self.is_wide() {
            return theme.border_style();
        }
        let list_focused = matches!(self.focus, Focus::List);
        pane_border_style(theme, is_list_pane == list_focused)
    }

    /// `draw()` 专用: 只重算 `is_dirty()` 的缓存 (不碰 `draft`/`focus`, 不产出通知)——「同一状态
    /// 画两次得到同一帧」不受影响, 覆盖「草稿仍指向一条存在的订阅, 但它在 `Store` 里的值变了」
    /// 这种只有靠重新画才会经过的路径。真正的丢弃 (D2) 只在 `sync_draft_with_store` (`update()`
    /// 时机) 里发生, 见 [`Draft::refresh_dirty`] 与 [`Draft::sync`] 的分工说明。
    fn refresh_dirty_flag(&mut self, store: &Store) {
        let base = self.draft.get().and_then(|d| store.subscription(&d.sub_id)).map(slot_draft_base);
        self.draft.refresh_dirty(base.as_ref());
    }

    /// `update()` 专用: 核对一遍草稿是否已经与 `Store` 当前值相等 (D2, 改回原值 / 别的客户端把
    /// `Store` 改成了跟草稿一样都算) 就真的丢弃; 草稿对应的订阅从 `Store` 消失 (被别处删除) 时
    /// 也丢弃, 并顺带处理「消失」这件业务逻辑本身 (清草稿、焦点退回 `List`、排一条 `s.sub_gone`
    /// 通知)——这两件事都是业务状态变更, 只能在 `update()`/`handle_key()` 里做, 不能在 `draw()`
    /// 里 (`draw()` 不改业务状态)。
    fn sync_draft_with_store(&mut self, store: &Store, s: &'static Strings) {
        let Some(sub_id) = self.draft.get().map(|d| d.sub_id.clone()) else { return };
        let base = store.subscription(&sub_id).map(slot_draft_base);
        let vanished = self.draft.sync(base.as_ref());
        if vanished {
            self.focus = Focus::List;
            self.pending_notice = Some((ToastKind::Info, s.sub_gone.to_string()));
        }
    }

    /// 当前应该显示的槽位值: 有草稿 (且草稿属于这条订阅) 就用草稿, 否则用 `Store` 里的原始值。
    fn effective_model_slots<'a>(&'a self, sub: &'a Subscription) -> &'a ModelSlots {
        match self.draft.get() {
            Some(d) if d.sub_id == sub.id => &d.model_slots,
            _ => &sub.model_slots,
        }
    }

    fn effective_slot_efforts<'a>(&'a self, sub: &'a Subscription) -> &'a SlotEfforts {
        match self.draft.get() {
            Some(d) if d.sub_id == sub.id => &d.slot_efforts,
            _ => &sub.slot_efforts,
        }
    }

    fn move_slot_cursor(&mut self, current: Slot, delta: isize) {
        let idx = ALL_SLOTS.iter().position(|&sl| sl == current).unwrap_or(0);
        let next = (idx as isize + delta).clamp(0, ALL_SLOTS.len() as isize - 1) as usize;
        self.focus = Focus::Detail { slot: ALL_SLOTS[next] };
    }

    /// I1: 当前选中的订阅是否正有一次 `UpdateSlots` 保存在飞行中。
    fn is_saving(&self) -> bool {
        self.saving.is_some()
    }

    fn saving_notice(s: &'static Strings) -> Action {
        Action::Notify { kind: ToastKind::Info, text: s.saving_in_progress.to_string() }
    }

    /// `⏎` (在 `Detail` 焦点下): 打开当前槽位的模型 picker, `initial` 是草稿 (没有就是 `Store`)
    /// 里该槽当前值; 兜底槽额外在最前面放一项「清空」。
    fn open_model_picker(&self, sub: &Subscription, slot: Slot, s: &'static Strings) -> Action {
        let initial = self.effective_model_slots(sub).get(slot).to_string();
        let mut items = Vec::new();
        if slot == Slot::Fallback {
            items.push(PickerItem { id: String::new(), label: s.pick_clear_fallback.to_string(), hint: None });
        }
        if let Some(cache) = &sub.model_cache {
            items.extend(cache.models.iter().map(|m| PickerItem { id: m.id.clone(), label: m.id.clone(), hint: m.display_name.clone() }));
        }
        Action::OpenPicker(PickerSpec {
            // I5: 带上这次弹窗是为哪条订阅开的, `PickerDone` 落地时据此核对是否还该应用。
            tag: PickerTag::SlotModel { sub_id: sub.id.clone(), slot },
            title: (s.pick_model_title)(slot_label(slot, s)),
            items,
            allow_custom: true,
            initial,
        })
    }

    /// `o` (在 `Detail` 焦点下): 打开当前槽位的思考档位 picker, 兜底槽 / Kiro 订阅直接拒绝
    /// (就地回一条 `Action::Notify`, 不开弹窗)。
    fn open_effort_picker_or_refuse(&self, sub: &Subscription, slot: Slot, s: &'static Strings) -> Option<Action> {
        if slot == Slot::Fallback {
            return Some(Action::Notify { kind: ToastKind::Info, text: s.sub_effort_na_fallback.to_string() });
        }
        if sub.auth_type == "kiro_oauth" {
            return Some(Action::Notify { kind: ToastKind::Info, text: s.sub_effort_na_kiro.to_string() });
        }
        let initial = self.effective_slot_efforts(sub).get(slot).unwrap_or("").to_string();
        let mut items = vec![PickerItem { id: String::new(), label: s.sub_effort_auto.to_string(), hint: None }];
        items.extend(EFFORT_CHOICES.iter().map(|e| PickerItem { id: (*e).to_string(), label: (*e).to_string(), hint: None }));
        Some(Action::OpenPicker(PickerSpec {
            tag: PickerTag::SlotEffort { sub_id: sub.id.clone(), slot },
            title: (s.pick_effort_title)(slot_label(slot, s)),
            items,
            allow_custom: false,
            initial,
        }))
    }

    /// I5: 这个 `sub_id` 是否还该被当前页面接住——焦点必须在 `Detail`, 且等于**当前选中项**
    /// (不是"草稿属于哪条订阅", 草稿本来就该跟着选中项走)。弹窗打开之后订阅可能已经被删除、
    /// 焦点已经退回列表、或者 (理论上不该发生, 但防御性地) 选中项变成了另一条——都应该让调用方
    /// 静默忽略这次 picker 结果, 不弹通知 (弹窗本身已经在这种情况下被 `App` 关掉了, 见
    /// `App::notify_subscriptions_changed`)。
    fn applies_to(&self, sub_id: &str) -> bool {
        matches!(self.focus, Focus::Detail { .. }) && self.selected_id.as_deref() == Some(sub_id)
    }

    /// `PickerDone` 落地: 按 `tag` 通过 [`Draft::edit`] 写进草稿 (首次编辑时惰性克隆, 结果等于
    /// `Store` 当前值就立刻丢弃, D2/D3 起这条规则收在 `Draft` 内部, 这里不用再手动核对一遍);
    /// 主槽的空白自定义值被拒绝 (拒绝时不碰草稿), 兜底槽的空白等于清空。跟自己无关的 tag
    /// (虚拟模型页的 `VmAddSubscription`) 直接忽略; I5: `sub_id` 对不上当前选中项 (或者焦点已经
    /// 不在 `Detail`) 也直接忽略, 不弹通知; I1: 这条订阅正有保存在飞行中时拒绝, 弹
    /// `saving_in_progress`。
    fn apply_picker_choice(&mut self, tag: &PickerTag, choice: &PickerChoice, store: &Store, s: &'static Strings) {
        match tag {
            PickerTag::SlotModel { sub_id, slot } => {
                if !self.applies_to(sub_id) {
                    return;
                }
                let Some(sub) = store.subscription(sub_id) else { return };
                if self.is_saving() {
                    self.pending_notice = Some((ToastKind::Info, s.saving_in_progress.to_string()));
                    return;
                }
                // 防御性: 草稿如果属于别的订阅 (不该发生, `applies_to` 已经确认 `sub_id` 等于当前
                // 选中项, 草稿理应跟着选中项走) 先丢弃, 不把别的订阅的编辑内容当成这条订阅的基线。
                if self.draft.get().is_some_and(|d| d.sub_id != *sub_id) {
                    self.draft.clear();
                }
                let base = slot_draft_base(sub);
                let value = match choice {
                    PickerChoice::Item(item_id) => item_id.clone(),
                    PickerChoice::Custom(text) => text.trim().to_string(),
                };
                if value.is_empty() && *slot != Slot::Fallback {
                    self.pending_notice = Some((ToastKind::Info, s.sub_model_required.to_string()));
                    return;
                }
                self.draft.edit(&base, |d| d.model_slots.set(*slot, value));
            }
            PickerTag::SlotEffort { sub_id, slot } => {
                if !self.applies_to(sub_id) {
                    return;
                }
                let Some(sub) = store.subscription(sub_id) else { return };
                if self.is_saving() {
                    self.pending_notice = Some((ToastKind::Info, s.saving_in_progress.to_string()));
                    return;
                }
                if self.draft.get().is_some_and(|d| d.sub_id != *sub_id) {
                    self.draft.clear();
                }
                let base = slot_draft_base(sub);
                let value = match choice {
                    PickerChoice::Item(item_id) if item_id.is_empty() => None,
                    PickerChoice::Item(item_id) => Some(item_id.clone()),
                    // `allow_custom: false`: picker 理论上不会产出 Custom, 防御性地忽略。
                    PickerChoice::Custom(_) => return,
                };
                self.draft.edit(&base, |d| d.slot_efforts.set(*slot, value));
            }
            PickerTag::VmAddSubscription { .. } => (),
        }
    }

    /// `s`: 不脏时无动作; 脏时产出 `Action::Mutate(UpdateSlots)`——断线由 `App::start_mutation`
    /// 统一处理 (弹 `toast_offline`, 不真的发), 这里不用重复判断连接状态。I5: 额外要求草稿的
    /// `sub_id` 等于当前选中项, 否则拒绝、绝不发送——草稿理论上只可能属于当前选中项 (`focus ==
    /// Detail` 期间选中项不会变), 但这是发往后端的最后一道关卡, 宁可防御性地多判一次。
    fn save_action(&self) -> Option<Action> {
        if !self.is_dirty() {
            return None;
        }
        let draft = self.draft.get()?;
        if self.selected_id.as_deref() != Some(draft.sub_id.as_str()) {
            return None;
        }
        Some(Action::Mutate(Mutation::UpdateSlots {
            id: draft.sub_id.clone(),
            model_slots: draft.model_slots.clone(),
            slot_efforts: draft.slot_efforts.clone(),
        }))
    }

    /// 每次 `draw` / `handle_key` 都要调用: 把 `selected_id` 解析成当前列表里的下标。
    /// 首次有数据 (`selected_id` 还是 `None`) 选第一条; id 还在列表里就跟着它走 (哪怕挪了位置);
    /// id 不在了就落到 `last_index` 钳制到新列表长度后的位置。列表为空返回 `None`。
    fn resolve_selection(&mut self, subs: &[Subscription]) -> Option<usize> {
        if subs.is_empty() {
            self.selected_id = None;
            self.last_index = 0;
            return None;
        }
        let idx = match &self.selected_id {
            Some(id) => subs.iter().position(|s| &s.id == id).unwrap_or_else(|| self.last_index.min(subs.len() - 1)),
            None => 0,
        };
        self.selected_id = Some(subs[idx].id.clone());
        self.last_index = idx;
        Some(idx)
    }

    fn select_index(&mut self, subs: &[Subscription], idx: usize) {
        if subs.is_empty() {
            self.selected_id = None;
            self.last_index = 0;
            return;
        }
        let idx = idx.min(subs.len() - 1);
        self.selected_id = Some(subs[idx].id.clone());
        self.last_index = idx;
    }

    /// 相对当前下标移动 `delta` 步, 钳制在 `[0, len-1]`, 不绕回。
    fn move_selection(&mut self, subs: &[Subscription], cur: Option<usize>, delta: isize) {
        let Some(cur) = cur else { return };
        if subs.is_empty() {
            return;
        }
        let next = (cur as isize + delta).clamp(0, subs.len() as isize - 1) as usize;
        self.select_index(subs, next);
    }

    #[allow(clippy::too_many_arguments)] // 与主 crate 的 dispatch 函数同一条先例 (见 proxy/*.rs)
    fn draw_list(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        ctx: &mut DrawCtx,
        subs: &[Subscription],
        idx: usize,
        flash_rows: &[String],
        border_style: Style,
    ) {
        let s = ctx.s;
        self.table_state.select(Some(idx));

        // 手算每列的显示宽度, 好让 `format::fit` 与 Table 实际分配的列宽严格一致:
        // 边框(2) + 左右留白(2×LIST_PADDING) + 选中前缀(HIGHLIGHT_COL) +
        // [符号 + 备注名 + (状态?) + 厂商 + sonnet] (每个列间距各 1)。
        // M6: 状态列只在还能给 sonnet 留够 ≥12 列时才加进来 (name 20 / provider 12 / sonnet ≥12
        // 是硬下限, 放不下就整列省略, 不挤压这三个)。
        let inner_width = area.width.saturating_sub(2 + 2 * LIST_PADDING);
        let columns_width = inner_width.saturating_sub(HIGHLIGHT_COL);
        const MIN_SONNET_COL: u16 = 12;
        let base_fixed = SYMBOL_COL + 1 + NAME_COL as u16 + 1 + PROVIDER_COL as u16 + 1;
        let with_status_fixed = base_fixed + STATUS_COL as u16 + 1;
        let show_status = columns_width >= with_status_fixed + MIN_SONNET_COL;
        let fixed = if show_status { with_status_fixed } else { base_fixed };
        let sonnet_col = columns_width.saturating_sub(fixed) as usize;

        let mut header_cells = vec![Cell::from(""), Cell::from(fit(s.sub_col_name, NAME_COL))];
        if show_status {
            header_cells.push(Cell::from(fit(s.sub_col_state, STATUS_COL)));
        }
        header_cells.push(Cell::from(fit(s.sub_col_provider, PROVIDER_COL)));
        header_cells.push(Cell::from(fit(s.sub_col_sonnet, sonnet_col)));
        let header = Row::new(header_cells).style(ctx.theme.muted_style());

        let rows: Vec<Row> = subs
            .iter()
            .map(|sub| {
                // 手动停用的订阅: 整行都用 muted 样式, 不再各自套 badge 的语义色——用户自己关掉的
                // 订阅不需要用颜色去强调它「健康」还是「限流」。
                let muted = !sub.enabled;
                let muted_style = ctx.theme.muted_style();
                // 忙碌的订阅: 第一列的状态符号换成 spinner (与总览页的加载态同一套 throbber_set),
                // 不再显示 badge 的颜色/符号——正在跑的操作可能就是要把这个状态改掉。
                let symbol = if ctx.busy.contains_key(&BusyKey::Subscription(sub.id.clone())) {
                    let glyph = Throbber::default().throbber_set(BRAILLE_SIX).to_symbol_span(&spinner_state(ctx.tick));
                    Span::styled(fit(glyph.content.as_ref(), SYMBOL_COL as usize), muted_style)
                } else {
                    let b = badge(sub, ctx.theme, s);
                    let style = if muted { muted_style } else { Style::new().fg(b.color) };
                    Span::styled(fit(b.symbol, SYMBOL_COL as usize), style)
                };
                let mut cells = vec![
                    Cell::from(symbol),
                    Cell::from(Span::styled(fit(&sub.display_name, NAME_COL), if muted { muted_style } else { Style::default() })),
                ];
                if show_status {
                    let b = badge(sub, ctx.theme, s);
                    let text = status_text(sub, &b, ctx.now_ms);
                    let style = if muted { muted_style } else { Style::new().fg(b.color) };
                    cells.push(Cell::from(Span::styled(fit(&text, STATUS_COL), style)));
                }
                cells.push(Cell::from(Span::styled(
                    fit(&sub.provider_display_name, PROVIDER_COL),
                    if muted { muted_style } else { Style::default() },
                )));
                cells.push(Cell::from(Span::styled(
                    fit(&sub.model_slots.sonnet, sonnet_col),
                    if muted { muted_style } else { Style::default() },
                )));
                Row::new(cells)
            })
            .collect();

        let total = subs.len();
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(border_style)
            .title_top(format!(" {} ", (s.sub_title)(total)))
            .title_bottom(Line::from(format!(" {}/{} ", idx + 1, total)).right_aligned().style(ctx.theme.muted_style()))
            .padding(Padding::horizontal(LIST_PADDING));

        let mut widths = vec![Constraint::Length(SYMBOL_COL), Constraint::Length(NAME_COL as u16)];
        if show_status {
            widths.push(Constraint::Length(STATUS_COL as u16));
        }
        widths.push(Constraint::Length(PROVIDER_COL as u16));
        widths.push(Constraint::Length(sonnet_col as u16));

        let table = Table::new(rows, widths)
            .header(header)
            .highlight_symbol("▌ ")
            .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .block(block);

        frame.render_stateful_widget(&table, area, &mut self.table_state);

        // 表体可视行数 = 内高 - 边框(2) - 表头(1); 左右留白不占高度。只有超出这个数才需要滚动条 /
        // 用来翻页。
        let capacity = area.height.saturating_sub(3) as usize;
        self.last_page_rows = capacity.max(1);
        if total > capacity {
            let mut sb_state = ScrollbarState::new(total.saturating_sub(capacity)).position(self.table_state.offset());
            frame.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight),
                area.inner(Margin { vertical: 1, horizontal: 0 }),
                &mut sb_state,
            );
        }

        // 只对这一帧实际画出来的行触发闪烁, 滚出视野的丢弃 (与总览页同一套「每帧开头取走」写法)。
        let offset = self.table_state.offset();
        let content_x = area.x + 1 + LIST_PADDING;
        let content_width = inner_width;
        for (i, sub) in subs.iter().enumerate().skip(offset).take(capacity) {
            if flash_rows.contains(&sub.id) {
                let row_y = area.y + 2 + (i - offset) as u16;
                let row_rect = Rect::new(content_x, row_y, content_width, 1);
                let b = badge(sub, ctx.theme, s);
                ctx.fx.row_changed(&sub.id, row_rect, b.color);
            }
        }
    }

    fn draw_detail(&self, frame: &mut Frame, area: Rect, ctx: &DrawCtx, sub: &Subscription, narrow: bool, border_style: Style) {
        let s = ctx.s;
        let model_slots = self.effective_model_slots(sub);
        let slot_efforts = self.effective_slot_efforts(sub);
        let focus_slot = match self.focus {
            Focus::Detail { slot } => Some(slot),
            Focus::List => None,
        };
        // 有草稿时标题加 " *"——走 `is_dirty()` (`Component` trait 方法, 与 `App::guard_dirty` 问
        // 的是同一个问题) 而不是直接读 `self.dirty` 字段, 保证「脏」只有这一个判定入口, 改回原值
        // (草稿仍在但与 `Store` 相等) 不该继续显示这个星号。
        let title_suffix = if self.is_dirty() { " *" } else { "" };
        let mut block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(border_style)
            .title_top(format!(" {}{} ", sub.display_name, title_suffix))
            .padding(Padding::horizontal(1));
        if narrow {
            block = block.title_bottom(Line::from(format!(" Esc {} ", s.key_back)).right_aligned().style(ctx.theme.muted_style()));
        }
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let rows = detail_rows(sub, model_slots, slot_efforts, focus_slot, ctx, inner.width);
        draw_detail_rows(frame, inner, ctx, &rows);
    }

    fn draw_placeholder(&self, frame: &mut Frame, area: Rect, ctx: &mut DrawCtx, loading: bool) {
        let s = ctx.s;
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(ctx.theme.border_style())
            .title_top(format!(" {} ", (s.sub_title)(0)));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if loading {
            let mut state = spinner_state(ctx.tick);
            let throbber = Throbber::default().label(s.loading).throbber_set(BRAILLE_SIX).style(ctx.theme.muted_style());
            frame.render_stateful_widget(throbber, inner, &mut state);
        } else {
            frame.render_widget(Line::styled(s.ov_no_subs, ctx.theme.muted_style()), inner);
        }
    }
}

impl Component for Subscriptions {
    fn handle_key(&mut self, key: KeyEvent, store: &Store, s: &'static Strings) -> Option<Action> {
        let subs = store.subscriptions();
        let idx = self.resolve_selection(subs);

        // 有草稿时 e/t/m/b 一律拒绝 (不管当前 focus——草稿只可能在 `Detail` 焦点下存在, 但这条
        // 判断不依赖那个不变式), 避免重拉覆盖编辑基线的困惑。D2/D3 (fix round P3b) 起
        // `draft.get().is_some()` 与 `is_dirty()` 恒等价 (零编辑/改回原值都不留草稿, 由
        // `Draft::edit`/`Draft::sync` 保证), 这里直接查草稿是否存在——与虚拟模型页 V5 的左栏
        // `*` 标记同一种判定方式, 两个页面对这条不变式的依赖保持一致。
        if self.draft.get().is_some() && matches!(key.code, KeyCode::Char('e' | 't' | 'm' | 'b')) {
            return Some(Action::Notify { kind: ToastKind::Info, text: s.sub_save_first.to_string() });
        }

        match self.focus {
            Focus::List => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.move_selection(subs, idx, -1);
                    None
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.move_selection(subs, idx, 1);
                    None
                }
                KeyCode::Char('g') | KeyCode::Home => {
                    self.select_index(subs, 0);
                    None
                }
                KeyCode::Char('G') | KeyCode::End => {
                    if !subs.is_empty() {
                        self.select_index(subs, subs.len() - 1);
                    }
                    None
                }
                KeyCode::PageUp => {
                    self.move_selection(subs, idx, -(self.last_page_rows.max(1) as isize));
                    None
                }
                KeyCode::PageDown => {
                    self.move_selection(subs, idx, self.last_page_rows.max(1) as isize);
                    None
                }
                // 两种宽度都有效 (Task 5): 宽屏下这只是把焦点从列表挪到详情 (边框跟着变), 窄屏下
                // 才是「切一整屏」——同一个按键, `draw()` 按宽度决定怎么呈现。
                KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') if idx.is_some() => {
                    self.focus = Focus::Detail { slot: Slot::Fable };
                    None
                }
                KeyCode::Char('e') => idx.map(|i| Action::Mutate(Mutation::SetEnabled { id: subs[i].id.clone(), enabled: !subs[i].enabled })),
                KeyCode::Char('t') => idx.map(|i| Action::Mutate(Mutation::TestConnection { id: subs[i].id.clone() })),
                KeyCode::Char('m') => idx.map(|i| Action::Mutate(Mutation::RefreshModels { id: subs[i].id.clone() })),
                KeyCode::Char('b') => idx.map(|i| Action::Mutate(Mutation::RefreshBalance { id: subs[i].id.clone() })),
                _ => None,
            },
            Focus::Detail { slot } => match key.code {
                // 五个槽位间移动, 不绕回; 列表选中项不动。
                KeyCode::Up | KeyCode::Char('k') => {
                    self.move_slot_cursor(slot, -1);
                    None
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.move_slot_cursor(slot, 1);
                    None
                }
                KeyCode::Char('g') | KeyCode::Home => {
                    self.focus = Focus::Detail { slot: ALL_SLOTS[0] };
                    None
                }
                KeyCode::Char('G') | KeyCode::End => {
                    self.focus = Focus::Detail { slot: ALL_SLOTS[ALL_SLOTS.len() - 1] };
                    None
                }
                KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => {
                    if self.is_dirty() {
                        Some(Action::OpenConfirm { prompt: s.confirm_discard.to_string(), on_yes: Box::new(Action::DiscardDraft) })
                    } else {
                        // 草稿不脏 (可能压根没有, 也可能改回了原值) 时直接放行, 顺带清掉它——
                        // `focus == List` 时 `draft` 恒为 `None` 是页面维持的不变式。D2 起改回
                        // 原值时草稿其实已经被 `Draft::edit` 自动丢弃了, 这里的 `clear()` 只是
                        // 兜底 (真正没有草稿的普通情况下是个 no-op)。
                        self.draft.clear();
                        self.focus = Focus::List;
                        None
                    }
                }
                KeyCode::Enter => {
                    // I1/M5: 保存在飞行中时拒绝打开 picker——避免用户对着一份马上要被覆盖的草稿
                    // 继续编辑, `apply_picker_choice` 里的同款守卫是给"picker 已经开着、保存才
                    // 开始"这种更罕见的时序兜底, 这里挡的是更常见的"想再开一次 picker"。
                    if self.is_saving() {
                        return Some(Self::saving_notice(s));
                    }
                    let i = idx?;
                    Some(self.open_model_picker(&subs[i], slot, s))
                }
                KeyCode::Char('o') => {
                    if self.is_saving() {
                        return Some(Self::saving_notice(s));
                    }
                    let i = idx?;
                    self.open_effort_picker_or_refuse(&subs[i], slot, s)
                }
                KeyCode::Char('s') => {
                    // M5: 再按一次 s (保存已经在飞行中) 不再被 `App::start_mutation` 的忙碌表悄悄
                    // 吞掉——就地给个提示, 而不是让用户以为按键没生效。
                    if self.is_saving() {
                        return Some(Self::saving_notice(s));
                    }
                    self.save_action()
                }
                KeyCode::Char('e') => idx.map(|i| Action::Mutate(Mutation::SetEnabled { id: subs[i].id.clone(), enabled: !subs[i].enabled })),
                KeyCode::Char('t') => idx.map(|i| Action::Mutate(Mutation::TestConnection { id: subs[i].id.clone() })),
                KeyCode::Char('m') => idx.map(|i| Action::Mutate(Mutation::RefreshModels { id: subs[i].id.clone() })),
                KeyCode::Char('b') => idx.map(|i| Action::Mutate(Mutation::RefreshBalance { id: subs[i].id.clone() })),
                _ => None,
            },
        }
    }

    fn update(&mut self, action: &Action, store: &Store, s: &'static Strings) -> Vec<Cmd> {
        // 每次 `update()` 都先核对一遍草稿: 草稿对应的订阅可能已经在别处被删掉 (见
        // `sync_draft_with_store`)。放在 match 之前, 不管这次具体是哪个 action。
        self.sync_draft_with_store(store, s);
        match action {
            Action::Refresh | Action::Connected { .. } => vec![Cmd::Fetch(Fetch::Subscriptions)],
            Action::Sse { name, .. } if SSE_REFETCH.contains(&name.as_str()) => vec![Cmd::Fetch(Fetch::Subscriptions)],
            Action::PickerDone { tag, choice } => {
                self.apply_picker_choice(tag, choice, store, s);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, ctx: &mut DrawCtx) {
        let flash_rows = std::mem::take(&mut self.flash_rows);
        self.last_width = area.width;
        // 只读重算 `dirty` 缓存 (不碰 `draft`/`focus`): 覆盖「草稿仍指向一条存在的订阅, 但它在
        // `Store` 里的值这一帧才刚变」的路径, 不依赖下一次 `update()` 才被发现。
        self.refresh_dirty_flag(ctx.store);

        if !ctx.store.subscriptions_loaded() {
            self.draw_placeholder(frame, area, ctx, true);
            return;
        }
        let subs = ctx.store.subscriptions();
        if subs.is_empty() {
            self.draw_placeholder(frame, area, ctx, false);
            return;
        }
        let Some(idx) = self.resolve_selection(subs) else { return };

        if self.is_wide() {
            let [left, right] = Layout::horizontal([Constraint::Length(self.list_width()), Constraint::Min(0)]).areas(area);
            let list_border = self.pane_border_style(ctx.theme, true);
            let detail_border = self.pane_border_style(ctx.theme, false);
            self.draw_list(frame, left, ctx, subs, idx, &flash_rows, list_border);
            self.draw_detail(frame, right, ctx, &subs[idx], false, detail_border);
        } else if matches!(self.focus, Focus::List) {
            let border = self.pane_border_style(ctx.theme, true);
            self.draw_list(frame, area, ctx, subs, idx, &flash_rows, border);
        } else {
            let border = self.pane_border_style(ctx.theme, false);
            self.draw_detail(frame, area, ctx, &subs[idx], true, border);
        }
    }

    fn hints(&self, s: &'static Strings) -> Vec<Hint<'static>> {
        match self.focus {
            Focus::List => {
                // S1(a) (fix round P3b): `⏎` 现在两种宽度下都会真的切焦点进详情 (Task 5), 不该
                // 只在窄屏才提示——宽屏用户一样需要知道这个键。
                vec![("↑↓", s.key_select), ("⏎", s.key_detail), ("e", s.key_toggle), ("t", s.key_test), ("m", s.key_models), ("b", s.key_balance)]
            }
            Focus::Detail { .. } => {
                // 放不下时 keybar 从右往左丢——e/t/m/b 排在最后, 会先被裁掉, 符合简报的预期。
                // V1(b) (fix round P3b): 脏页面上 `s 保存` 排到 `↑↓ 选择` 右边第一个, 保证它是
                // 最后才会被裁掉的那批——丢掉保存提示是所有裁剪结果里最糟的一种; 不脏时留在原位
                // (跟在改模型/改档位后面, 视觉上更贴近它们描述的操作)。
                // M6 (fix round final): 脏时精简成 `↑↓ 选择 / s 保存 / Esc 放弃 / ⏎ 改模型 / o
                // 改档位` 这五个——e/t/m/b 此时全部被拒绝 (见 `handle_key` 顶部的守卫), 继续
                // 提示它们只会让用户白按; `Esc 放弃` 是新增的, 紧跟在 `s` 后面 (同样是"保存/放弃
                // 这次编辑"这组操作里最该保留的一批, 优先级仅次于 `s` 本身)。
                let mut hints = vec![("↑↓", s.key_select)];
                if self.is_dirty() {
                    hints.push(("s", s.key_save));
                    hints.push(("Esc", s.key_discard));
                    hints.push(("⏎", s.key_edit_model));
                    hints.push(("o", s.key_edit_effort));
                } else {
                    hints.push(("⏎", s.key_edit_model));
                    hints.push(("o", s.key_edit_effort));
                    hints.push(("s", s.key_save));
                    hints.push(("e", s.key_toggle));
                    hints.push(("t", s.key_test));
                    hints.push(("m", s.key_models));
                    hints.push(("b", s.key_balance));
                }
                hints
            }
        }
    }

    fn help(&self, s: &'static Strings) -> &'static [(&'static str, &'static str)] {
        s.sub_help_rows
    }

    fn on_subscriptions_changed(&mut self, changed: &[String], store: &Store, s: &'static Strings) {
        // 整体替换而不是往后追加: 页面不可见时攒了好几拨变化, 回来只该闪最新一拨——旧的早就过时了,
        // 而且不去重的 `extend` 会让积压的重复 id 在 `flash_rows.contains` 里白跑好几遍 (I10)。
        self.flash_rows = changed.to_vec();
        // S1(c) (fix round P3b): `Store` 这一刻刚接受了新列表, 立刻核对一遍草稿对应的订阅还在不在,
        // 不用等下一次真正的 `update()` (`Refresh`/`Sse`/`PickerDone`/…) 才发现——早一帧总比晚一帧
        // 好, 尤其是「订阅被删了但用户还盯着详情面板」这种场景。
        self.sync_draft_with_store(store, s);
    }

    fn is_dirty(&self) -> bool {
        self.draft.is_dirty()
    }

    fn discard_changes(&mut self) {
        self.draft.clear();
        self.focus = Focus::List;
    }

    fn on_mutation_started(&mut self, mutation: &Mutation) {
        // I1: 记下这条订阅正有保存在飞行中——不看 `ok`/`err`, 那是 `on_mutation_done` 才知道的事;
        // 这里只关心"发出去了", 从这一刻起到结果落地之间拒绝继续编辑这份草稿。
        if let Mutation::UpdateSlots { id, .. } = mutation {
            self.saving = Some(id.clone());
        }
    }

    fn on_mutation_done(&mut self, mutation: &Mutation, ok: bool) {
        // I1: 不管成败, 先把"正在保存"标记摘掉——`ok=false` 时草稿要继续可编辑 (原有行为不变),
        // `ok=true` 时下面才决定草稿本身要不要清空。
        if let Mutation::UpdateSlots { id, .. } = mutation {
            if self.saving.as_deref() == Some(id.as_str()) {
                self.saving = None;
            }
        }
        if !ok {
            return;
        }
        // D1 (fix round P3b): 只有「保存时发出去的那份负载」与「结果落地这一刻的当前草稿」完全
        // 相等才清空——用户在保存在途期间可能已经又编辑了一次 (比如先选 m3、按 s、还没等结果回来
        // 又选了 m9), 这时不能凭 `id` 匹配就无条件清掉, 会把 m9 这次编辑悄悄冲掉且没有任何提示。
        if let Mutation::UpdateSlots { id, model_slots, slot_efforts } = mutation {
            if self.draft.get().is_some_and(|d| &d.sub_id == id) {
                let saved = SlotDraft { sub_id: id.clone(), model_slots: model_slots.clone(), slot_efforts: slot_efforts.clone() };
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

fn field_line(label: &'static str, mut value: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::raw(fit(label, FIELD_LABEL_COL))];
    spans.append(&mut value);
    Line::from(spans)
}

/// 长文本超宽时截断成省略号收尾, 但不像 `format::fit` 那样把短文本右补空格到定宽——这几处
/// (URL / 被引用列表 / 余额条目) 是自由文本行, 不是要跟表格对齐的列; 补出来的空格会把跟在
/// 后面的别的 span (比如余额条目的 hint) 顶到可视宽度以外, 平白消失 (Fix round 1, #5 的教训)。
fn clip(text: &str, width: usize) -> String {
    if text.width() <= width {
        text.to_string()
    } else {
        fit(text, width)
    }
}

fn model_style(model: &str, theme: &Theme) -> Style {
    // 后端字段, 保险起见按 trim 后的值比较 (与 `ping.rs::pick_test_model` 同规则), 不因为多一个
    // 空格就把该有的 warn 色漏掉。
    if model.trim() == PENDING_MODEL {
        Style::new().fg(theme.warn)
    } else {
        Style::default()
    }
}

/// 四个主槽 + 兜底槽里最长的那个模型名的显示宽度。
fn longest_model_width(model_slots: &ModelSlots) -> usize {
    [&model_slots.fable, &model_slots.opus, &model_slots.sonnet, &model_slots.haiku, &model_slots.fallback]
        .into_iter()
        .map(|m| m.width())
        .max()
        .unwrap_or(0)
}

/// I2 (Task 5 修正): 模型名列宽 = `min(可用宽度, 五个槽里最长模型名的显示宽度 + 2)`, 下限 24
/// (旧的固定值)——之前的版本把整段可用宽度都给了模型名, 短模型名 (常见情况) 后面拖着一大段
/// 空白, effort 列被推到贴着右边框的地方; 现在按实际内容定宽, 让 effort 列贴着模型名。
/// 宽度小到连 24 都算不出来时仍然钳制在 24 (`.max(24)` 排在 `.min(available)` 之后), 不会因为
/// 窄而给出更小 (甚至溢出成 0) 的值——80 列的最小终端保证了这种极端情况不会真的溢出面板。
fn slot_model_col(width: u16, model_slots: &ModelSlots) -> usize {
    let available = width.saturating_sub(2 + SLOT_NAME_COL as u16 + EFFORT_COL as u16) as usize;
    (longest_model_width(model_slots) + 2).min(available).max(24)
}

/// `Slot` 的显示名: 四个主槽用英文原名 (与后端 `ModelSlots` 的字段名一致), `Fallback` 用现有的
/// `s.sub_slot_fallback` (中文「兜底」)。
fn slot_label(slot: Slot, s: &'static Strings) -> &'static str {
    match slot {
        Slot::Fable => "fable",
        Slot::Opus => "opus",
        Slot::Sonnet => "sonnet",
        Slot::Haiku => "haiku",
        Slot::Fallback => s.sub_slot_fallback,
    }
}

/// `modified`: 这个槽位的显示值 (草稿) 跟 `Store` 里的原始值不同, 行末追加一条 muted 的
/// `s.sub_slot_modified`。`focused`: 当前详情焦点落在这个槽位, 整行 `REVERSED` (与列表选中行、
/// picker 选中行同一套视觉语言)。
#[allow(clippy::too_many_arguments)] // 与主 crate 的 dispatch 函数同一条先例 (见 proxy/*.rs)
fn slot_line(
    name: &'static str,
    model: &str,
    effort: Option<&str>,
    model_col: usize,
    theme: &Theme,
    s: &'static Strings,
    modified: bool,
    focused: bool,
) -> Line<'static> {
    let effort_text = match effort {
        Some(e) if !e.is_empty() => e.to_string(),
        _ => s.sub_effort_auto.to_string(),
    };
    let effort_style = if effort.is_some_and(|e| !e.is_empty()) { Style::default() } else { theme.muted_style() };
    let mut spans = vec![
        Span::raw(format!("  {}", fit(name, SLOT_NAME_COL))),
        Span::styled(fit(model, model_col), model_style(model, theme)),
        Span::styled(effort_text, effort_style),
    ];
    if modified {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(s.sub_slot_modified, theme.muted_style()));
    }
    let mut line = Line::from(spans);
    if focused {
        line = line.style(Style::new().add_modifier(Modifier::REVERSED));
    }
    line
}

fn fallback_slot_line(model: &str, theme: &Theme, s: &'static Strings, modified: bool, focused: bool) -> Line<'static> {
    let name = format!("  {}", fit(s.sub_slot_fallback, SLOT_NAME_COL));
    let mut spans = if model.is_empty() {
        vec![Span::raw(name), Span::styled(s.sub_slot_unset, theme.muted_style())]
    } else {
        vec![Span::raw(name), Span::styled(model.to_string(), model_style(model, theme))]
    };
    if modified {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(s.sub_slot_modified, theme.muted_style()));
    }
    let mut line = Line::from(spans);
    if focused {
        line = line.style(Style::new().add_modifier(Modifier::REVERSED));
    }
    line
}

/// `value_width`: 字段值那一列还剩多少显示宽度 (详情内宽 - `FIELD_LABEL_COL`), 长文本 (URL /
/// 被引用列表 / 余额条目) 超出时截断成省略号收尾, 不能硬裁到贴着边框。
fn balance_rows(sub: &Subscription, theme: &Theme, s: &'static Strings, value_width: usize) -> Vec<DetailRow> {
    let mut out = Vec::new();
    if !sub.balance_supported {
        out.push(DetailRow::Line(field_line(s.sub_f_balance, vec![Span::styled(s.sub_balance_unsupported, theme.muted_style())])));
        return out;
    }
    let Some(cache) = &sub.balance_cache else {
        out.push(DetailRow::Line(field_line(s.sub_f_balance, vec![Span::styled(s.sub_balance_never, theme.muted_style())])));
        return out;
    };
    let snapshot = &cache.snapshot;
    let mut first = true;
    if snapshot.is_available == Some(false) {
        let label = if first { s.sub_f_balance } else { "" };
        first = false;
        out.push(DetailRow::Line(field_line(label, vec![Span::styled(s.sub_balance_unavailable, Style::new().fg(theme.err))])));
    }
    if snapshot.entries.is_empty() {
        if first {
            out.push(DetailRow::Line(field_line(s.sub_f_balance, vec![Span::styled(s.sub_balance_never, theme.muted_style())])));
        }
        return out;
    }
    for entry in &snapshot.entries {
        let label = if first { s.sub_f_balance } else { "" };
        first = false;
        let color = match entry.severity {
            BalanceSeverity::Low => Some(theme.warn),
            BalanceSeverity::Critical => Some(theme.err),
            BalanceSeverity::Normal | BalanceSeverity::Unknown => None,
        };
        let text = clip(&format!("{} {} {}", entry.label, entry.value_text, entry.unit), value_width);
        let mut spans = vec![Span::raw(text)];
        if let Some(color) = color {
            spans[0].style = Style::new().fg(color);
        }
        if let Some(hint) = &entry.hint {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(hint.clone(), theme.muted_style()));
        }
        out.push(DetailRow::Line(field_line(label, spans)));
    }
    out
}

/// `width`: 详情面板内宽 (block 边框 + 内边距之后), 用来给长字段截断、给「最近错误」估折行数。
/// `model_slots`/`slot_efforts`: 当前应该显示的槽位值 (有草稿就是草稿, 否则是 `sub` 自己的原始值,
/// 由调用方 `Subscriptions::effective_model_slots`/`effective_slot_efforts` 算好传进来);
/// `focus_slot`: 详情焦点当前落在哪个槽位 (`None` = 焦点不在详情, 比如宽屏下焦点还在列表)。
fn detail_rows(
    sub: &Subscription,
    model_slots: &ModelSlots,
    slot_efforts: &SlotEfforts,
    focus_slot: Option<Slot>,
    ctx: &DrawCtx,
    width: u16,
) -> Vec<DetailRow> {
    let s = ctx.s;
    let theme = ctx.theme;
    let value_width = width.saturating_sub(FIELD_LABEL_COL as u16) as usize;
    let mut rows = Vec::new();

    // 状态: `badge()` 的符号 + 文案, 冷却倒计时规则与总览页共用 (`widgets::badge::status_text`)。
    // 这条订阅正有就地操作在跑时, 后面追加一条 muted 的进行中文案 ——哪个操作就显示哪句。
    let b = badge(sub, theme, s);
    let status = format!("{} {}", b.symbol, status_text(sub, &b, ctx.now_ms));
    let mut status_spans = vec![Span::styled(status, Style::new().fg(b.color))];
    if let Some(m) = ctx.busy.get(&BusyKey::Subscription(sub.id.clone())) {
        let busy_text = match m {
            Mutation::SetEnabled { .. } => s.sub_busy_toggling,
            Mutation::TestConnection { .. } => s.sub_busy_testing,
            Mutation::RefreshModels { .. } => s.sub_busy_models,
            Mutation::RefreshBalance { .. } => s.sub_busy_balance,
            Mutation::UpdateSlots { .. } => s.sub_busy_saving,
            // 这个订阅页的 busy 查询按 `BusyKey::Subscription` 取, `UpdateVirtualModel` 只会出现在
            // `BusyKey::VirtualModel` 下, 永远不会真的落到这一分支——但 `match m` 穷尽
            // `Mutation` 的全部变体 (编译器不知道调用方已经按 key 过滤过), 补一个不会触发的分支。
            Mutation::UpdateVirtualModel { .. } => s.sub_busy_saving,
        };
        status_spans.push(Span::raw(" · "));
        status_spans.push(Span::styled(busy_text, theme.muted_style()));
    }
    rows.push(DetailRow::Line(field_line(s.sub_f_state, status_spans)));

    // I1: 上次操作的结果 (与对应 toast 同一份文本), 紧跟在状态后面; 没有条目就不画这一行。
    // 发起新操作那一刻 `App::start_mutation` 就会把这里清掉, 所以正忙的订阅不会同时既显示
    // 「正在测试连接…」又显示上一次早已过时的结果。
    if let Some((kind, text)) = ctx.last_outcome.get(&sub.id) {
        let style = match kind {
            ToastKind::Success => Style::new().fg(theme.ok),
            ToastKind::Error => Style::new().fg(theme.err),
            ToastKind::Info => Style::default(),
        };
        rows.push(wrapped_row(s.sub_f_last_action, text, value_width as u16, LAST_ACTION_ROWS, style));
    }

    // 厂商
    rows.push(DetailRow::Line(field_line(
        s.sub_f_provider,
        vec![Span::raw(format!("{} · {}", sub.provider_display_name, sub.auth_type))],
    )));

    // 端点
    rows.push(DetailRow::Line(field_line(s.sub_f_endpoint, vec![Span::raw(clip(&sub.base_url, value_width))])));

    // 槽位: 标签独占一行, 四个槽 + 兜底各自缩进一行 (兜底没有 effort 列)。有草稿时显示草稿值;
    // 与 `Store` (`sub` 自己的字段) 不同的槽位行末尾加 muted 的「已修改」, 焦点落在的槽位整行
    // REVERSED。
    rows.push(DetailRow::Line(Line::from(Span::raw(fit(s.sub_f_slots, FIELD_LABEL_COL)))));
    let model_col = slot_model_col(width, model_slots);
    for slot in MAIN_SLOTS {
        let name = slot_label(slot, s);
        let model = model_slots.get(slot);
        let effort = slot_efforts.get(slot);
        let modified = model != sub.model_slots.get(slot) || effort != sub.slot_efforts.get(slot);
        let focused = focus_slot == Some(slot);
        rows.push(DetailRow::Line(slot_line(name, model, effort, model_col, theme, s, modified, focused)));
    }
    let fallback_model = model_slots.get(Slot::Fallback);
    let fallback_modified = fallback_model != sub.model_slots.get(Slot::Fallback);
    let fallback_focused = focus_slot == Some(Slot::Fallback);
    rows.push(DetailRow::Line(fallback_slot_line(fallback_model, theme, s, fallback_modified, fallback_focused)));

    // 限额: 每个设了上限的周期一行, 不只显示最紧的那个。`limit == Some(0)` 与「没设上限」同义
    // (`QuotaUsage::ratio()` 把它当无限额处理, 见 `tightest_quota` 同一条规则), 不能只看
    // `limit.is_some()`——否则会显示一条 "0%  n / 0" 的假限额行 (M4)。
    let limited: Vec<&QuotaUsage> = sub.quota_usage.iter().filter(|q| q.ratio().is_some()).collect();
    if limited.is_empty() {
        rows.push(DetailRow::Line(field_line(s.sub_f_quota, vec![Span::styled("—", theme.muted_style())])));
    } else {
        for (i, q) in limited.iter().enumerate() {
            let label = if i == 0 { s.sub_f_quota } else { "" };
            rows.push(DetailRow::Quota { label, quota: (*q).clone() });
        }
    }

    // 余额
    rows.extend(balance_rows(sub, theme, s, value_width));

    // 模型
    let models_text = match &sub.model_cache {
        Some(cache) => (s.sub_models_cached)(cache.models.len()),
        None => s.sub_models_never.to_string(),
    };
    rows.push(DetailRow::Line(field_line(s.sub_f_models, vec![Span::raw(models_text)])));

    // 被引用
    let referenced = if sub.referenced_by.is_empty() {
        Span::styled(s.sub_unreferenced, theme.muted_style())
    } else {
        Span::raw(clip(&sub.referenced_by.join(", "), value_width))
    };
    rows.push(DetailRow::Line(field_line(s.sub_f_referenced, vec![referenced])));

    // 最近错误: 永远是最后一条, 按实际折行数占 1..=4 行 (见 [`wrapped_row`]), 不再单独占死 4 行。
    let error_text = match &sub.last_error_message {
        Some(msg) => msg.clone(),
        None => "—".into(),
    };
    rows.push(wrapped_row(s.sub_f_last_error, &error_text, value_width as u16, LAST_ERROR_ROWS, Style::default()));

    rows
}

/// 这一行需要几个显示行。除 `Wrapped` 外都是定高 1 行, `Wrapped` 的行数已经在构造时
/// (见 [`wrapped_row`]) 算好存进 `height` 字段——不在画的时候重算, 保证「占几行」与真正截给
/// `Paragraph` 的那份文本 (`text`) 永远是同一次计算的结果, 不会对不上。
fn row_height(row: &DetailRow) -> u16 {
    match row {
        DetailRow::Line(_) | DetailRow::Quota { .. } => 1,
        DetailRow::Wrapped { height, .. } => *height,
    }
}

/// 从上到下依次画每一行; 高度不够全部画完时, 最后一行改画「(s.ov_more_rows)(hidden)」(与总览页
/// 健康度面板超出可视高度时同一个词条, `hidden` 是没画出来的字段条数, 不是行数)。
fn draw_detail_rows(frame: &mut Frame, area: Rect, ctx: &DrawCtx, rows: &[DetailRow]) {
    let s = ctx.s;
    let heights: Vec<u16> = rows.iter().map(row_height).collect();
    let total: u16 = heights.iter().sum();
    let overflow = total > area.height;
    // 溢出时给最后一行的提示让位; 没溢出就用满整个 area。
    let capacity = if overflow { area.height.saturating_sub(1) } else { area.height };

    let mut y = area.y;
    let mut shown = 0usize;
    for (row, h) in rows.iter().zip(&heights) {
        if y + h > area.y + capacity {
            break;
        }
        draw_detail_row(frame, Rect::new(area.x, y, area.width, *h), ctx, row);
        y += h;
        shown += 1;
    }
    if overflow {
        let hidden = rows.len() - shown;
        let rect = Rect::new(area.x, area.y + area.height.saturating_sub(1), area.width, 1);
        frame.render_widget(Line::styled((s.ov_more_rows)(hidden), ctx.theme.muted_style()), rect);
    }
}

fn draw_detail_row(frame: &mut Frame, rect: Rect, ctx: &DrawCtx, row: &DetailRow) {
    match row {
        DetailRow::Line(line) => frame.render_widget(line.clone(), Rect::new(rect.x, rect.y, rect.width, 1)),
        DetailRow::Quota { label, quota } => draw_quota_row(frame, Rect::new(rect.x, rect.y, rect.width, 1), ctx, label, quota),
        // M2: 标签只画在第一行 (label_area), 正文整段交给 `Paragraph` 在 value_area 里自己折行——
        // 这样续行天然从 value_area.x (与其它字段的值列完全相同的一列) 开始, 不会像"标签+正文拼成
        // 一整条字符串再整体 Wrap"那样, 续行找不到标签占的那几列, 缩回列 0。
        DetailRow::Wrapped { label, text, style, .. } => {
            let [label_area, value_area] = Layout::horizontal([Constraint::Length(FIELD_LABEL_COL as u16), Constraint::Min(0)]).areas(rect);
            frame.render_widget(Line::raw(fit(label, FIELD_LABEL_COL)), Rect::new(label_area.x, label_area.y, label_area.width, 1));
            frame.render_widget(Paragraph::new(text.as_str()).style(*style).wrap(Wrap { trim: true }), value_area);
        }
    }
}

/// 「最近错误」/「上次操作」这类自由文本字段的通用构造: 先用 [`clip_to_rows`] 截到 `max_rows`
/// 行装得下的字符数为止 (超出的部分补省略号收尾), 再用这份已经定长的文本算它占几行——
/// 保证存进 [`DetailRow::Wrapped`] 的 `height` 与真正交给 `Paragraph` 渲染的 `text` 是同一次
/// 计算的结果, 不会出现"分配的行数比实际截断后的内容还少, 省略号被吞掉看不见"这种偏差。
fn wrapped_row(label: &'static str, text: &str, value_width: u16, max_rows: u16, style: Style) -> DetailRow {
    let clipped = clip_to_rows(text, value_width, max_rows);
    let height = wrapped_line_count(&clipped, value_width, max_rows);
    DetailRow::Wrapped { label, text: clipped, height, style }
}

/// 超过 `width` 列 `max_rows` 行装得下的字符数就截断收尾补省略号——`Paragraph` 的 `Wrap` 只会把
/// 画不出来的内容悄悄丢掉, 不会自己加省略号, 所以这一步必须在喂给它之前做完 (M2)。
fn clip_to_rows(text: &str, width: u16, max_rows: u16) -> String {
    let width = width.max(1);
    // M3: 上游的错误信息没有长度上限, `text.width()` 是 usize, 直接 `as u16` 在超长文本上会
    // 静默环绕算出错误的容量; 用 `try_from` 饱和到 `u16::MAX`, 不 panic 也不会算错。
    let text_width = u16::try_from(text.width()).unwrap_or(u16::MAX);
    let capacity = width.saturating_mul(max_rows);
    if text_width <= capacity {
        text.to_string()
    } else {
        clip(text, capacity.saturating_sub(1) as usize)
    }
}

/// 粗略估算 `Wrap { trim: true }` 会把这段文本折成几行: 按显示宽度整除是「贴着最后一列才换行」
/// 的下界, 真实的按词 / 标点换行几乎总是提前收尾, 常见比整除结果多用一行——所以在整除结果上
/// +1 兜底, 宁可多留一行空白也不要把最后一行文字挤没 (Fix round 1, #6)。`saturating_add` /
/// `clamp` 到 `max_rows`: 超长文本 (M3) 不能让这两步在极端输入上 panic。
fn wrapped_line_count(text: &str, width: u16, max_rows: u16) -> u16 {
    let width = width.max(1);
    let text_width = u16::try_from(text.width()).unwrap_or(u16::MAX);
    text_width.div_ceil(width).saturating_add(1).clamp(1, max_rows.max(1))
}

fn draw_quota_row(frame: &mut Frame, area: Rect, ctx: &DrawCtx, label: &str, q: &QuotaUsage) {
    let s = ctx.s;
    let theme = ctx.theme;
    // 先把标签切出来 (不带 spacing, 与 `field_line` 的值列起点严格一致), 再在剩下的宽度里给
    // 周期名/进度条/百分比/用量四段各自留一点呼吸间距 (Fix round 1, #1: 之前把标签也算进
    // `.spacing(1)` 里, 所有字段行的值都会因此错位一列)。
    let [label_area, value_area] = Layout::horizontal([Constraint::Length(FIELD_LABEL_COL as u16), Constraint::Min(0)]).areas(area);
    frame.render_widget(Line::raw(fit(label, FIELD_LABEL_COL)), label_area);

    let ratio = q.ratio().unwrap_or(0.0);
    let [period_area, gauge_area, pct_area, used_area] =
        Layout::horizontal([Constraint::Length(8), Constraint::Min(6), Constraint::Length(5), Constraint::Length(16)])
            .spacing(1)
            .areas(value_area);
    frame.render_widget(Line::styled(s.quota_period(q.period), theme.muted_style()), period_area);
    frame.render_widget(quota_gauge(ratio, theme), gauge_area);
    frame.render_widget(Line::raw(format!("{:.0}%", ratio * 100.0)).right_aligned(), pct_area);
    frame.render_widget(Line::raw(format!("{} / {}", compact(q.used() as i64), compact(q.limit.unwrap_or(0) as i64))), used_area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::ZH;

    /// Fix round 1, #10: 页面不可见时攒了好几拨订阅变化, 回来只该闪最新一拨——`extend` 会把
    /// 旧的也留着, 之后每帧都要多扫一遍这些早就过时的 id。
    #[test]
    fn on_subscriptions_changed_replaces_the_queue_not_appends() {
        let mut page = Subscriptions::default();
        let store = Store::default();
        page.on_subscriptions_changed(&["a".to_string(), "b".to_string()], &store, &ZH);
        page.on_subscriptions_changed(&["c".to_string()], &store, &ZH);
        assert_eq!(page.flash_rows, vec!["c".to_string()], "第三次通知应该整体替换队列, 不是往后追加");
    }

    /// D3: `Draft<T>` 的 `edit` 帮页面自动做「零编辑不留草稿」, D2 的槽位编辑单测因此可以直接从
    /// `Draft` 的单测里覆盖——这里只补一条页面层面的集成检查: 通过 `apply_picker_choice` 选回原值
    /// 之后, `draft` 真的被清空了 (`get()` 返回 `None`), 不是仅仅 `dirty` 缓存变假。
    #[test]
    fn a_reverted_pick_actually_clears_the_draft_not_just_the_dirty_cache() {
        let mut page = Subscriptions::default();
        let mut store = Store::default();
        let sub = crate::client::dto::Subscription {
            id: "1".into(),
            display_name: "s".into(),
            provider_display_name: "p".into(),
            enabled: true,
            state: crate::client::dto::SubscriptionState::Healthy,
            cooldown_until: None,
            last_error_message: None,
            is_dispatchable: true,
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
        };
        store.apply_subscriptions(1, vec![sub]);
        page.selected_id = Some("1".into());
        // I5: `apply_picker_choice` 现在要求焦点在 `Detail` 且 `sub_id` 等于当前选中项才应用。
        page.focus = Focus::Detail { slot: Slot::Fable };

        page.apply_picker_choice(
            &PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable },
            &PickerChoice::Item("m3".into()),
            &store,
            &ZH,
        );
        assert!(page.draft.get().is_some());
        page.apply_picker_choice(
            &PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable },
            &PickerChoice::Custom("d".into()),
            &store,
            &ZH,
        );
        assert!(page.draft.get().is_none(), "改回原值应该真的清空草稿, 不是只改 dirty 缓存");
    }

    fn minimal_sub(id: &str) -> crate::client::dto::Subscription {
        crate::client::dto::Subscription {
            id: id.into(),
            display_name: id.into(),
            provider_display_name: "p".into(),
            enabled: true,
            state: crate::client::dto::SubscriptionState::Healthy,
            cooldown_until: None,
            last_error_message: None,
            is_dispatchable: true,
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

    /// I5: `save_action` 额外要求草稿的 `sub_id` 等于当前选中项, 绝不把它发给屏幕上并没有显示的
    /// 那一条订阅。这个不一致在正常的 `App` 驱动流程里走不到 (`focus == Detail` 期间选中项不会
    /// 变, 草稿存在时 `resolve_selection` 也不会把它换成一个不同的、仍然存在的订阅)——这里直接
    /// 摆一个理论上不该出现的内部状态, 覆盖这最后一道防线本身。
    #[test]
    fn save_never_sends_a_draft_for_a_subscription_that_is_not_on_screen() {
        let mut page = Subscriptions::default();
        let mut store = Store::default();
        store.apply_subscriptions(1, vec![minimal_sub("1"), minimal_sub("2")]);
        page.selected_id = Some("1".into());
        page.focus = Focus::Detail { slot: Slot::Fable };
        page.apply_picker_choice(
            &PickerTag::SlotModel { sub_id: "1".into(), slot: Slot::Fable },
            &PickerChoice::Item("m3".into()),
            &store,
            &ZH,
        );
        assert!(page.draft.get().is_some_and(|d| d.sub_id == "1"), "草稿应该属于 \"1\"");

        // 理论上不该发生: 选中项被换成了另一条仍然存在的订阅, 草稿还留着 "1" 的编辑。
        page.selected_id = Some("2".into());
        assert_eq!(page.save_action(), None, "草稿的 sub_id 跟当前选中项不一致时不该发送");
    }

    /// M3: 上游错误信息没有长度上限, 旧版 `text.width() as u16` 会在超长字符串上静默环绕
    /// (70,000 % 65536 = 4,464), 算出一个错误但不 panic 的行数。修好之后应该稳稳落在 `max_rows`
    /// 这个上限, 而不是那个环绕出来的错误值。
    #[test]
    fn wrapped_line_count_saturates_instead_of_panicking() {
        let text = "x".repeat(70_000);
        assert_eq!(wrapped_line_count(&text, 40, LAST_ERROR_ROWS), LAST_ERROR_ROWS);
        assert_eq!(wrapped_line_count(&text, 40, LAST_ACTION_ROWS), LAST_ACTION_ROWS);
    }

    /// M3 的姊妹函数: `clip_to_rows` 也要在同一个输入上不 panic, 并且真的把文本截到了 `max_rows`
    /// 行的容量以内 (含省略号)。
    #[test]
    fn clip_to_rows_saturates_instead_of_panicking() {
        let text = "x".repeat(70_000);
        let clipped = clip_to_rows(&text, 40, LAST_ERROR_ROWS);
        assert!(clipped.width() <= 40 * LAST_ERROR_ROWS as usize, "截断后应该落在容量以内: {}", clipped.width());
        assert!(clipped.trim_end().ends_with('…'), "超长文本截断后应该以省略号收尾");
    }

    fn slots_with_sonnet(sonnet: &str) -> ModelSlots {
        ModelSlots { fable: "d".into(), opus: "a".into(), sonnet: sonnet.into(), haiku: "c".into(), fallback: String::new() }
    }

    /// I2 (Task 5 修正): 模型名列宽是 `min(可用宽度, 最长模型名+2)`, 下限 24——不再是「把整段
    /// 可用宽度都给模型名」那版, 短模型名不该拖出一大段空白让 effort 列远在天边。
    #[test]
    fn slot_model_col_uses_the_longest_model_name_capped_by_available_width_and_a_floor() {
        // 80 列窄屏详情面板: inner=76, 可用=76-2-8(SLOT_NAME_COL)-8(EFFORT_COL)=58。
        // 短模型名 (mock 惯用的 "d"/"a"/"c" 单字符): 1+2=3, 远小于下限 24, 钳到 24。
        assert_eq!(slot_model_col(76, &slots_with_sonnet("b")), 24);
        // 30 字符的真实模型 id: 30+2=32, 小于可用宽度 58, 直接用 32——effort 列贴着模型名,
        // 不再吃掉整段 58 列的可用宽度。
        let real = slots_with_sonnet("qwen3-coder-480b-a35b-instruct");
        assert_eq!(slot_model_col(76, &real), 32);
        // 宽度小到连 24 都算不出来时, 仍然钳制在 24, 不会因为窄而给出更小 (甚至溢出成 0) 的值。
        assert_eq!(slot_model_col(20, &slots_with_sonnet("b")), 24);
    }
}
