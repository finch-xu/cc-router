//! 订阅页 (只读): 列表 + 详情, 宽屏 (≥120 列) 双栏 / 窄屏进出详情。
//! 四个原地操作 (编辑槽位 `e` / 测试连接 `t` / 刷新模型 `m` / 刷新余额 `b`) 留给 Task 4。

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Cell, LineGauge, Padding, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table, TableState, Wrap,
};
use ratatui::Frame;
use throbber_widgets_tui::{Throbber, BRAILLE_SIX};
use unicode_width::UnicodeWidthStr;

use super::{Component, DrawCtx};
use crate::action::{Action, Cmd, Fetch};
use crate::client::dto::{BalanceSeverity, QuotaUsage, Subscription};
use crate::format::{compact, fit, mmss};
use crate::i18n::Strings;
use crate::store::Store;
use crate::theme::Theme;
use crate::widgets::badge::badge;
use crate::widgets::keybar::Hint;
use crate::widgets::spinner_state;

/// 达到才用左表右详情双栏; 以下只画一栏, 靠 `detail_open` 在列表/详情之间切换。
const WIDE_THRESHOLD: u16 = 120;
const LIST_WIDTH: u16 = 58;
/// 表的选中前缀 (`highlight_symbol`) 固定宽度, 用于手算列宽给 `format::fit`。
const HIGHLIGHT_COL: u16 = 2;
const SYMBOL_COL: u16 = 2;
const NAME_COL: usize = 20;
const PROVIDER_COL: usize = 12;
const FIELD_LABEL_COL: usize = 10;
const SLOT_NAME_COL: usize = 8;
const SLOT_MODEL_COL: usize = 24;
/// 「最近错误」占的固定行数, 超出的部分被 `Wrap` 裁掉。
const LAST_ERROR_ROWS: u16 = 4;
/// 两步向导没走完时槽位留下的占位模型名。
const PENDING_MODEL: &str = "(pending)";

const SSE_REFETCH: [&str; 2] = ["subscription_state_changed", "subscription_quota_reached"];

/// 详情面板的一行: 大多数是普通文本, 限额行要嵌一个真正的 `LineGauge` widget, 不是文本能表示的。
enum DetailRow {
    Line(Line<'static>),
    Quota { label: &'static str, quota: QuotaUsage },
}

#[derive(Default)]
pub struct Subscriptions {
    selected_id: Option<String>,
    /// `selected_id` 在新列表里找不到时, 用这个 (钳制到新列表长度后) 兜底, 而不是简单地弹回第一条。
    last_index: usize,
    /// 只有窄屏 (< [`WIDE_THRESHOLD`]) 才有意义; 宽屏画双栏时忽略它 (详情视图只是窄屏的一种呈现,
    /// 不是独立状态机)。
    detail_open: bool,
    /// 上一帧的宽度: `handle_key` 判断 `⏎` 该不该进详情要用得到, 但按键发生时还不知道这一帧的几何。
    last_width: u16,
    /// 上一帧表体的可视行数, `PageUp` / `PageDown` 按这个翻页。
    last_page_rows: usize,
    table_state: TableState,
    /// 下一帧要闪一下的订阅 id; `draw` 取走。
    flash_rows: Vec<String>,
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
        // 边框(2) + 选中前缀(HIGHLIGHT_COL) + [符号 + 备注名 + 厂商 + sonnet] (三个列间距各 1)。
        let inner_width = area.width.saturating_sub(2);
        let columns_width = inner_width.saturating_sub(HIGHLIGHT_COL);
        let fixed = SYMBOL_COL + 1 + NAME_COL as u16 + 1 + PROVIDER_COL as u16 + 1;
        let sonnet_col = columns_width.saturating_sub(fixed) as usize;

        let header = Row::new([
            Cell::from(""),
            Cell::from(fit(s.sub_col_name, NAME_COL)),
            Cell::from(fit(s.sub_col_provider, PROVIDER_COL)),
            Cell::from(fit(s.sub_col_sonnet, sonnet_col)),
        ])
        .style(ctx.theme.muted_style());

        let rows: Vec<Row> = subs
            .iter()
            .map(|sub| {
                let b = badge(sub, ctx.theme, s);
                Row::new([
                    Cell::from(Span::styled(fit(b.symbol, SYMBOL_COL as usize), Style::new().fg(b.color))),
                    Cell::from(fit(&sub.display_name, NAME_COL)),
                    Cell::from(fit(&sub.provider_display_name, PROVIDER_COL)),
                    Cell::from(fit(&sub.model_slots.sonnet, sonnet_col)),
                ])
            })
            .collect();

        let total = subs.len();
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(ctx.theme.border_style())
            .title_top(format!(" {} ", (s.sub_title)(total)))
            .title_bottom(Line::from(format!(" {}/{} ", idx + 1, total)).right_aligned().style(ctx.theme.muted_style()));

        let table = Table::new(
            rows,
            [
                Constraint::Length(SYMBOL_COL),
                Constraint::Length(NAME_COL as u16),
                Constraint::Length(PROVIDER_COL as u16),
                Constraint::Length(sonnet_col as u16),
            ],
        )
        .header(header)
        .highlight_symbol("▌ ")
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .block(block);

        frame.render_stateful_widget(&table, area, &mut self.table_state);

        // 表体可视行数 = 内高 - 边框(2) - 表头(1); 只有超出这个数才需要滚动条 / 用来翻页。
        let capacity = area.height.saturating_sub(3) as usize;
        self.last_page_rows = capacity.max(1);
        if total > capacity {
            let mut sb_state = ScrollbarState::new(total).position(self.table_state.offset());
            frame.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight),
                area.inner(Margin { vertical: 1, horizontal: 0 }),
                &mut sb_state,
            );
        }

        // 只对这一帧实际画出来的行触发闪烁, 滚出视野的丢弃 (与总览页同一套「每帧开头取走」写法)。
        let offset = self.table_state.offset();
        for (i, sub) in subs.iter().enumerate().skip(offset).take(capacity) {
            if flash_rows.contains(&sub.id) {
                let row_y = area.y + 2 + (i - offset) as u16;
                let row_rect = Rect::new(area.x + 1, row_y, area.width.saturating_sub(2), 1);
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

        let rows = detail_rows(sub, ctx);

        let error_text = match &sub.last_error_message {
            Some(msg) => msg.clone(),
            None => "—".into(),
        };
        let error_line = format!("{}{error_text}", fit(s.sub_f_last_error, FIELD_LABEL_COL));
        // 「最多占 4 行」是上限, 不是固定分配: 没有错误 (占位符「—」) 只需要 1 行, 把剩下的让给
        // 上面的字段, 免得在矮终端上把「被引用」这类靠后的行硬生生挤没了。
        let error_rows = wrapped_line_count(&error_line, inner.width).clamp(1, LAST_ERROR_ROWS);
        let [rows_area, error_area] = Layout::vertical([Constraint::Min(0), Constraint::Length(error_rows)]).areas(inner);
        draw_detail_rows(frame, rows_area, ctx, &rows);
        frame.render_widget(Paragraph::new(error_line).wrap(Wrap { trim: true }), error_area);
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
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(subs, idx, -1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(subs, idx, 1),
            KeyCode::Char('g') | KeyCode::Home => self.select_index(subs, 0),
            KeyCode::Char('G') | KeyCode::End => {
                if !subs.is_empty() {
                    self.select_index(subs, subs.len() - 1);
                }
            }
            KeyCode::PageUp => self.move_selection(subs, idx, -(self.last_page_rows.max(1) as isize)),
            KeyCode::PageDown => self.move_selection(subs, idx, self.last_page_rows.max(1) as isize),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') if !self.is_wide() && idx.is_some() => {
                self.detail_open = true;
            }
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => {
                self.detail_open = false;
            }
            _ => {}
        }
        None
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
        let idx = self.resolve_selection(subs).expect("subs 非空时 resolve_selection 总返回 Some");

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
        if !self.is_wide() {
            if self.detail_open {
                hints.push(("Esc", s.key_back));
            } else {
                hints.push(("⏎", s.key_detail));
            }
        }
        hints
    }

    fn help(&self, s: &'static Strings) -> &'static [(&'static str, &'static str)] {
        s.sub_help_rows
    }

    fn on_subscriptions_changed(&mut self, changed: &[String]) {
        self.flash_rows.extend(changed.iter().cloned());
    }
}

fn field_line(label: &'static str, mut value: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::raw(fit(label, FIELD_LABEL_COL))];
    spans.append(&mut value);
    Line::from(spans)
}

fn model_style(model: &str, theme: &Theme) -> Style {
    if model == PENDING_MODEL {
        Style::new().fg(theme.warn)
    } else {
        Style::default()
    }
}

fn slot_line(name: &'static str, model: &str, effort: Option<&str>, theme: &Theme, s: &'static Strings) -> Line<'static> {
    let effort_text = match effort {
        Some(e) if !e.is_empty() => e.to_string(),
        _ => s.sub_effort_auto.to_string(),
    };
    let effort_style = if effort.is_some_and(|e| !e.is_empty()) { Style::default() } else { theme.muted_style() };
    Line::from(vec![
        Span::raw(format!("  {}", fit(name, SLOT_NAME_COL))),
        Span::styled(fit(model, SLOT_MODEL_COL), model_style(model, theme)),
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

fn balance_rows(sub: &Subscription, theme: &Theme, s: &'static Strings) -> Vec<DetailRow> {
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
        let mut spans = vec![Span::raw(format!("{} {} {}", entry.label, entry.value_text, entry.unit))];
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

fn detail_rows(sub: &Subscription, ctx: &DrawCtx) -> Vec<DetailRow> {
    let s = ctx.s;
    let theme = ctx.theme;
    let mut rows = Vec::new();

    // 状态: `badge()` 的符号 + 文案, 与总览页同一条冷却规则 (`enabled && cooldown_until > now`
    // 才追加倒计时)。
    let b = badge(sub, theme, s);
    let status_text = match sub.cooldown_until.filter(|until| *until > ctx.now_ms && sub.enabled) {
        Some(until) => format!("{} {} · {}", b.symbol, b.label, mmss(until - ctx.now_ms)),
        None => format!("{} {}", b.symbol, b.label),
    };
    rows.push(DetailRow::Line(field_line(s.sub_f_state, vec![Span::styled(status_text, Style::new().fg(b.color))])));

    // 厂商
    rows.push(DetailRow::Line(field_line(
        s.sub_f_provider,
        vec![Span::raw(format!("{} · {}", sub.provider_display_name, sub.auth_type))],
    )));

    // 端点
    rows.push(DetailRow::Line(field_line(s.sub_f_endpoint, vec![Span::raw(sub.base_url.clone())])));

    // 槽位: 标签独占一行, 四个槽 + 兜底各自缩进一行 (兜底没有 effort 列)。
    rows.push(DetailRow::Line(Line::from(Span::raw(fit(s.sub_f_slots, FIELD_LABEL_COL)))));
    let slot_efforts = &sub.slot_efforts;
    for (name, model, effort) in [
        ("fable", &sub.model_slots.fable, slot_efforts.fable.as_deref()),
        ("opus", &sub.model_slots.opus, slot_efforts.opus.as_deref()),
        ("sonnet", &sub.model_slots.sonnet, slot_efforts.sonnet.as_deref()),
        ("haiku", &sub.model_slots.haiku, slot_efforts.haiku.as_deref()),
    ] {
        rows.push(DetailRow::Line(slot_line(name, model, effort, theme, s)));
    }
    rows.push(DetailRow::Line(fallback_slot_line(&sub.model_slots.fallback, theme, s)));

    // 限额: 每个设了上限的周期一行, 不只显示最紧的那个。
    let limited: Vec<&QuotaUsage> = sub.quota_usage.iter().filter(|q| q.limit.is_some()).collect();
    if limited.is_empty() {
        rows.push(DetailRow::Line(field_line(s.sub_f_quota, vec![Span::styled("—", theme.muted_style())])));
    } else {
        for (i, q) in limited.iter().enumerate() {
            let label = if i == 0 { s.sub_f_quota } else { "" };
            rows.push(DetailRow::Quota { label, quota: (*q).clone() });
        }
    }

    // 余额
    rows.extend(balance_rows(sub, theme, s));

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
        Span::raw(sub.referenced_by.join(", "))
    };
    rows.push(DetailRow::Line(field_line(s.sub_f_referenced, vec![referenced])));

    rows
}

fn draw_detail_rows(frame: &mut Frame, area: Rect, ctx: &DrawCtx, rows: &[DetailRow]) {
    for (i, row) in rows.iter().enumerate() {
        if i as u16 >= area.height {
            break;
        }
        let rect = Rect::new(area.x, area.y + i as u16, area.width, 1);
        match row {
            DetailRow::Line(line) => frame.render_widget(line.clone(), rect),
            DetailRow::Quota { label, quota } => draw_quota_row(frame, rect, ctx, label, quota),
        }
    }
}

/// 粗略估算 `Wrap { trim: true }` 会把这段文本折成几行: 按显示宽度整除, 不模拟真正的按词换行
/// (中文本来就没有词边界), 够用来给「最近错误」留够行数、不需要逐字符复刻 ratatui 的折行算法。
fn wrapped_line_count(text: &str, width: u16) -> u16 {
    let width = width.max(1);
    let text_width = text.width() as u16;
    text_width.div_ceil(width).max(1)
}

fn draw_quota_row(frame: &mut Frame, area: Rect, ctx: &DrawCtx, label: &str, q: &QuotaUsage) {
    let s = ctx.s;
    let theme = ctx.theme;
    let ratio = q.ratio().unwrap_or(0.0);
    let color = theme.quota_color(ratio);
    let [label_area, period_area, gauge_area, pct_area, used_area] = Layout::horizontal([
        Constraint::Length(FIELD_LABEL_COL as u16),
        Constraint::Length(8),
        Constraint::Min(6),
        Constraint::Length(5),
        Constraint::Length(16),
    ])
    .spacing(1)
    .areas(area);
    frame.render_widget(Line::raw(fit(label, FIELD_LABEL_COL)), label_area);
    frame.render_widget(Line::styled(s.quota_period(q.period), theme.muted_style()), period_area);
    frame.render_widget(
        LineGauge::default()
            .ratio(ratio)
            .label("")
            .filled_symbol("━")
            .unfilled_symbol("─")
            .filled_style(Style::new().fg(color))
            .unfilled_style(theme.border_style()),
        gauge_area,
    );
    frame.render_widget(Line::raw(format!("{:.0}%", ratio * 100.0)).right_aligned(), pct_area);
    frame.render_widget(
        Line::raw(format!("{} / {}", compact(q.used() as i64), compact(q.limit.unwrap_or(0) as i64))),
        used_area,
    );
}
