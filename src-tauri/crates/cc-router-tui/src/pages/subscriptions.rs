//! 订阅页: 列表 + 详情 (宽屏 ≥120 列双栏 / 窄屏进出详情), 加四个就地操作——启停 `e` / 测试连接
//! `t` / 刷新模型 `m` / 刷新余额 `b`, 列表态与详情态都生效, 作用于当前选中的订阅。

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

use super::{Component, DrawCtx};
use crate::action::{Action, BusyKey, Cmd, Fetch, Mutation};
use crate::client::dto::{BalanceSeverity, QuotaUsage, Subscription};
use crate::format::{compact, fit};
use crate::i18n::Strings;
use crate::store::Store;
use crate::theme::Theme;
use crate::widgets::badge::{badge, status_text};
use crate::widgets::gauge::quota_gauge;
use crate::widgets::keybar::Hint;
use crate::widgets::spinner_state;
use crate::widgets::toast::ToastKind;

/// 达到才用左表右详情双栏; 以下只画一栏, 靠 `detail_open` 在列表/详情之间切换。
const WIDE_THRESHOLD: u16 = 120;
const LIST_WIDTH: u16 = 58;
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

pub struct Subscriptions {
    selected_id: Option<String>,
    /// `selected_id` 在新列表里找不到时, 用这个 (钳制到新列表长度后) 兜底, 而不是简单地弹回第一条。
    last_index: usize,
    /// 只有窄屏 (< [`WIDE_THRESHOLD`]) 才有意义; 宽屏画双栏时忽略它 (详情视图只是窄屏的一种呈现,
    /// 不是独立状态机)。
    detail_open: bool,
    /// 上一帧的宽度: `handle_key` 判断 `⏎` 该不该进详情要用得到, 但按键发生时还不知道这一帧的几何。
    last_width: u16,
    /// 上一帧表体的可视行数, `PageUp` / `PageDown` 按这个翻页; 首帧之前用 [`DEFAULT_PAGE_ROWS`]
    /// 兜底, 不然第一次按键 (还没画过) 只会移动 0 格 (F8 教训: 步长绝不能默认成 0)。
    last_page_rows: usize,
    table_state: TableState,
    /// 下一帧要闪一下的订阅 id; `draw` 取走。
    flash_rows: Vec<String>,
}

impl Default for Subscriptions {
    fn default() -> Self {
        Self {
            selected_id: None,
            last_index: 0,
            detail_open: false,
            last_width: 0,
            last_page_rows: DEFAULT_PAGE_ROWS,
            table_state: TableState::default(),
            flash_rows: Vec::new(),
        }
    }
}

impl Subscriptions {
    fn is_wide(&self) -> bool {
        self.last_width >= WIDE_THRESHOLD
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

    fn draw_list(&mut self, frame: &mut Frame, area: Rect, ctx: &mut DrawCtx, subs: &[Subscription], idx: usize, flash_rows: &[String]) {
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
            .border_style(ctx.theme.border_style())
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

    fn draw_detail(&self, frame: &mut Frame, area: Rect, ctx: &DrawCtx, sub: &Subscription, narrow: bool) {
        let s = ctx.s;
        let mut block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(ctx.theme.border_style())
            .title_top(format!(" {} ", sub.display_name))
            .padding(Padding::horizontal(1));
        if narrow {
            block = block.title_bottom(Line::from(format!(" Esc {} ", s.key_back)).right_aligned().style(ctx.theme.muted_style()));
        }
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let rows = detail_rows(sub, ctx, inner.width);
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
    fn handle_key(&mut self, key: KeyEvent, store: &Store) -> Option<Action> {
        let subs = store.subscriptions();
        let idx = self.resolve_selection(subs);
        match key.code {
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
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') if !self.is_wide() && idx.is_some() => {
                self.detail_open = true;
                None
            }
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => {
                self.detail_open = false;
                None
            }
            // 四个就地操作: 列表态 / 详情态都生效, 作用于当前选中的订阅; 没有选中项 (空列表) 不动作。
            KeyCode::Char('e') => idx.map(|i| Action::Mutate(Mutation::SetEnabled { id: subs[i].id.clone(), enabled: !subs[i].enabled })),
            KeyCode::Char('t') => idx.map(|i| Action::Mutate(Mutation::TestConnection { id: subs[i].id.clone() })),
            KeyCode::Char('m') => idx.map(|i| Action::Mutate(Mutation::RefreshModels { id: subs[i].id.clone() })),
            KeyCode::Char('b') => idx.map(|i| Action::Mutate(Mutation::RefreshBalance { id: subs[i].id.clone() })),
            _ => None,
        }
    }

    fn update(&mut self, action: &Action, _store: &Store) -> Vec<Cmd> {
        match action {
            Action::Refresh | Action::Connected { .. } => vec![Cmd::Fetch(Fetch::Subscriptions)],
            Action::Sse { name, .. } if SSE_REFETCH.contains(&name.as_str()) => vec![Cmd::Fetch(Fetch::Subscriptions)],
            _ => Vec::new(),
        }
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, ctx: &mut DrawCtx) {
        let flash_rows = std::mem::take(&mut self.flash_rows);
        self.last_width = area.width;

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
            let [left, right] = Layout::horizontal([Constraint::Length(LIST_WIDTH), Constraint::Min(0)]).areas(area);
            self.draw_list(frame, left, ctx, subs, idx, &flash_rows);
            self.draw_detail(frame, right, ctx, &subs[idx], false);
        } else if self.detail_open {
            self.draw_detail(frame, area, ctx, &subs[idx], true);
        } else {
            self.draw_list(frame, area, ctx, subs, idx, &flash_rows);
        }
    }

    fn hints(&self, s: &'static Strings) -> Vec<Hint<'static>> {
        let mut hints = vec![("↑↓", s.key_select)];
        // M5: 详情态不再重复一份 Esc 返回——面板右下角的 `title_bottom` 已经在说这件事了 (见
        // `draw_detail`), 键位栏这份纯粹是挤占空间, 挤掉了本该放得下的 `b 余额`。
        if !self.is_wide() && !self.detail_open {
            hints.push(("⏎", s.key_detail));
        }
        hints.push(("e", s.key_toggle));
        hints.push(("t", s.key_test));
        hints.push(("m", s.key_models));
        hints.push(("b", s.key_balance));
        hints
    }

    fn help(&self, s: &'static Strings) -> &'static [(&'static str, &'static str)] {
        s.sub_help_rows
    }

    fn on_subscriptions_changed(&mut self, changed: &[String]) {
        // 整体替换而不是往后追加: 页面不可见时攒了好几拨变化, 回来只该闪最新一拨——旧的早就过时了,
        // 而且不去重的 `extend` 会让积压的重复 id 在 `flash_rows.contains` 里白跑好几遍 (I10)。
        self.flash_rows = changed.to_vec();
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

/// I2: 槽位那两个字符缩进 + 名字列 + effort 列吃掉的宽度之后, 剩下的都给模型名, 下限 24 列
/// (旧的固定值) ——真实的模型 id (`claude-sonnet-4-5-20250929` / `qwen3-coder-480b-a35b-instruct`)
/// 经常超过 24, 详情面板实际有 58~76 列可用, 不该白白浪费。
fn slot_model_col(width: u16) -> usize {
    width.saturating_sub(2 + SLOT_NAME_COL as u16 + EFFORT_COL as u16).max(24) as usize
}

fn slot_line(name: &'static str, model: &str, effort: Option<&str>, model_col: usize, theme: &Theme, s: &'static Strings) -> Line<'static> {
    let effort_text = match effort {
        Some(e) if !e.is_empty() => e.to_string(),
        _ => s.sub_effort_auto.to_string(),
    };
    let effort_style = if effort.is_some_and(|e| !e.is_empty()) { Style::default() } else { theme.muted_style() };
    Line::from(vec![
        Span::raw(format!("  {}", fit(name, SLOT_NAME_COL))),
        Span::styled(fit(model, model_col), model_style(model, theme)),
        Span::styled(effort_text, effort_style),
    ])
}

fn fallback_slot_line(model: &str, theme: &Theme, s: &'static Strings) -> Line<'static> {
    let name = format!("  {}", fit(s.sub_slot_fallback, SLOT_NAME_COL));
    if model.is_empty() {
        Line::from(vec![Span::raw(name), Span::styled(s.sub_slot_unset, theme.muted_style())])
    } else {
        Line::from(vec![Span::raw(name), Span::styled(model.to_string(), model_style(model, theme))])
    }
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
fn detail_rows(sub: &Subscription, ctx: &DrawCtx, width: u16) -> Vec<DetailRow> {
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

    // 槽位: 标签独占一行, 四个槽 + 兜底各自缩进一行 (兜底没有 effort 列)。
    rows.push(DetailRow::Line(Line::from(Span::raw(fit(s.sub_f_slots, FIELD_LABEL_COL)))));
    let model_col = slot_model_col(width);
    let slot_efforts = &sub.slot_efforts;
    for (name, model, effort) in [
        ("fable", &sub.model_slots.fable, slot_efforts.fable.as_deref()),
        ("opus", &sub.model_slots.opus, slot_efforts.opus.as_deref()),
        ("sonnet", &sub.model_slots.sonnet, slot_efforts.sonnet.as_deref()),
        ("haiku", &sub.model_slots.haiku, slot_efforts.haiku.as_deref()),
    ] {
        rows.push(DetailRow::Line(slot_line(name, model, effort, model_col, theme, s)));
    }
    rows.push(DetailRow::Line(fallback_slot_line(&sub.model_slots.fallback, theme, s)));

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

    /// Fix round 1, #10: 页面不可见时攒了好几拨订阅变化, 回来只该闪最新一拨——`extend` 会把
    /// 旧的也留着, 之后每帧都要多扫一遍这些早就过时的 id。
    #[test]
    fn on_subscriptions_changed_replaces_the_queue_not_appends() {
        let mut page = Subscriptions::default();
        page.on_subscriptions_changed(&["a".to_string(), "b".to_string()]);
        page.on_subscriptions_changed(&["c".to_string()]);
        assert_eq!(page.flash_rows, vec!["c".to_string()], "第三次通知应该整体替换队列, 不是往后追加");
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

    /// I2: 模型名列按可用宽度动态算, 下限 24 (旧的固定值)。
    #[test]
    fn slot_model_col_uses_available_width_with_a_floor() {
        // 80 列窄屏详情面板: inner=76, 76-2-8(SLOT_NAME_COL)-8(EFFORT_COL)=58。
        assert_eq!(slot_model_col(76), 58);
        // 宽度小到连 24 都算不出来时, 钳制在 24, 不会因为窄而给出更小 (甚至溢出成 0) 的值。
        assert_eq!(slot_model_col(20), 24);
    }
}
