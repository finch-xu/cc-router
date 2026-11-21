//! 请求日志页: 分页表格 (`list_requests`) + 按订阅 / 虚拟模型 / 状态过滤的单一选择弹窗 + `⏎`
//! 打开的只读详情弹窗。只有第 1 页跟着 `Action::Poll` 自动刷新, 翻到第 2 页以后只认用户按的 `r`。
//!
//! 与其它「表格 + 详情」页面 (订阅页 / 实时路由页) 不同的是: 这里没有 `Store` 里的一份共享数据,
//! `query`/`data` 完全是页面自己的状态——分页与过滤条件本身就是「用户此刻想看什么」, 不该被别的
//! 页面共享或缓存。

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Cell, HighlightSpacing, Padding, Row, Table, TableState};
use ratatui::Frame;
use throbber_widgets_tui::{Throbber, BRAILLE_SIX};

use super::{Component, DrawCtx};
use crate::action::{Action, Cmd, Fetch, FetchData};
use crate::client::dto::{RequestFilters, RequestLog, RequestPage, RequestQuery, RequestStatus, REQUEST_PAGE_SIZE};
use crate::format::{compact, duration, fit, short_stamp, thousands, widest};
use crate::i18n::Strings;
use crate::store::Store;
use crate::theme::Theme;
use crate::widgets::detail::{DetailRow, DetailSpec, Tone};
use crate::widgets::keybar::Hint;
use crate::widgets::picker::{PickerChoice, PickerItem, PickerSpec, PickerTag};
use crate::widgets::spinner_state;

/// 各定宽列按内容量出来的下限; 表头 (或状态列里的「✕ 超时」) 更宽的语言按 [`Cols::new`] 放宽。
const TIME_COL: usize = 11;
const STATUS_COL: usize = 6;
const VM_COL: usize = 14;
const SUB_COL: usize = 12;
const SUB_COL_WIDE: usize = 16;
/// 订阅列借给模型列之后的下限 (见 `draw_table`)。
const SUB_COL_MIN: usize = 8;
const CLIENT_COL: usize = 10;
const LATENCY_COL: usize = 6;
/// Token 列要放的「12.3K/3.4K」这一级数值; 表头更宽时按表头 (中文「Token 入/出」是 11)。
const TOKENS_VALUE_COL: usize = 10;
const WIDE_THRESHOLD: u16 = 120;
/// 表格定宽列的实际宽度: 每列取「下限」与「这一列在当前语言下要放的最宽文字」中的较大者, 表头与
/// 内容不会被截成无意义的片段; 中文下每一列都与按中文量出来的旧列宽相同。
struct Cols {
    time: usize,
    status: usize,
    vm: usize,
    sub: usize,
    client: usize,
    latency: usize,
    tokens: usize,
}

impl Cols {
    fn new(s: &Strings, wide: bool) -> Self {
        let at_least = |min: usize, texts: &[&str]| widest(texts.iter().copied()).max(min);
        let timeout_cell = timeout_cell(s);
        Self {
            time: at_least(TIME_COL, &[s.lg_col_time]),
            status: at_least(STATUS_COL, &[s.lg_col_status, &timeout_cell]),
            vm: at_least(VM_COL, &[s.lg_col_vm]),
            sub: at_least(if wide { SUB_COL_WIDE } else { SUB_COL }, &[s.lg_col_sub]),
            client: at_least(CLIENT_COL, &[s.lg_col_client]),
            latency: at_least(LATENCY_COL, &[s.lg_col_latency]),
            tokens: at_least(TOKENS_VALUE_COL, &[s.lg_col_tokens]),
        }
    }
}

/// 表的选中前缀 (`highlight_symbol("▌ ")`, `HighlightSpacing::Always`) 固定宽度, 用于手算
/// `Constraint::Fill(1)` 那一列 (模型名) 的实际宽度给 `format::fit`——与订阅页 `draw_list` 手算
/// `sonnet_col`、实时路由页 `draw_table` 手算 `sub_col` 同一个公式。
const HIGHLIGHT_COL: u16 = 2;

#[derive(Default)]
pub struct Logs {
    /// 当前想看的查询 (分页 + 过滤条件)。
    query: RequestQuery,
    /// 最近一次接受的结果, 连同它对应的查询——只有这份查询与当前的 `query` 一致时才当作"已加载"
    /// (见 `current_page`); 查询已经变了 (比如断线期间 `show_subscription` 换了过滤但发不出新的
    /// fetch) 就当作还没加载, 不能把上一次查询的行/计数展示在新过滤下面 (Finding 2)。`draw`/`⏎`
    /// 都经 `current_page`/`current_items` 读取, 天然遵守这条规则, 不需要各自再判一次。
    data: Option<(RequestQuery, RequestPage)>,
    /// 已接受结果的 `issued`, 挡住晚到的旧结果。
    accepted: u64,
    loading: bool,
    /// 按请求 id 记选中, 不用下标——翻页 / 重拉之后条目会整体替换。
    selected_id: Option<String>,
    table_state: TableState,
    /// 上一帧表体的可视行数, `PageUp`/`PageDown` 按这个翻页。
    last_page_rows: usize,
    /// 测试专用钩子: 生产代码永远不置位, 只有 `app.rs::tests` 里验证「未保存修改 → 先确认」流程
    /// 的用例会调 [`Logs::set_force_dirty`]。与 `pages::placeholder::Placeholder` 曾经的写法一致
    /// (Task 8 把它从占位页原样搬过来)——日志页本身是纯只读页面, 永远不会真的变脏。
    #[cfg(test)]
    force_dirty: bool,
}

#[cfg(test)]
impl Logs {
    pub fn set_force_dirty(&mut self, dirty: bool) {
        self.force_dirty = dirty;
    }

    /// 测试专用 (review fix round 1): 当前是否有加载在飞行中。`draw_table` 的「`data` 为 `None`
    /// 时一律显示加载中」这条约定生效之后, 渲染文本层面已经看不出一次失败的加载有没有真的清掉
    /// 这个标记——`app.rs::mod tests` 靠这个钩子直接读, 与 `set_force_dirty` 同一套「只在编译本
    /// crate 单测时存在」的做法。
    pub fn is_loading(&self) -> bool {
        self.loading
    }
}

impl Logs {
    /// 只看这条订阅: 清掉其它过滤维度, 回到第 1 页, 清掉选中。不在这里设置 `loading`/发起加载——
    /// 那是调用方 (`App::update` 的 `Action::OpenLogsFor` 分支) 紧接着用 `Refresh`/`SwitchTab`
    /// 触发的, 与 `PickerDone` 改过滤条件时的分工一致。
    pub fn show_subscription(&mut self, id: &str) {
        self.query = RequestQuery { page: 1, filters: RequestFilters { subscription_id: Some(id.to_string()), ..Default::default() } };
        self.selected_id = None;
    }

    /// `self.data` 里存的查询与当前 `self.query` 一致时才返回它的内容——否则说明 `self.query`
    /// 已经变了 (比如 `show_subscription` 换了过滤, 但断线期间 `App::switch_tab` 发不出新的
    /// fetch, 见 Finding 2), 旧结果不该被当成这份新查询的答案。`current_items`/`total_pages`/
    /// `draw_table` 统一走这一个函数, 不能各自判一次而漏掉某处。
    fn current_page(&self) -> Option<&RequestPage> {
        match &self.data {
            Some((q, page)) if *q == self.query => Some(page),
            _ => None,
        }
    }

    fn current_items(&self) -> &[RequestLog] {
        self.current_page().map(|page| page.items.as_slice()).unwrap_or(&[])
    }

    fn total_pages(&self) -> u32 {
        let Some(page) = self.current_page() else { return 1 };
        let total = page.total.max(0) as u64;
        let size = u64::from(REQUEST_PAGE_SIZE);
        (total.div_ceil(size).max(1)) as u32
    }

    fn start_fetch(&mut self) -> Vec<Cmd> {
        self.loading = true;
        vec![Cmd::Fetch(Fetch::Requests(self.query.clone()))]
    }

    /// 若 `selected_id` 不在新条目里, 改选第一条; 没有条目时为 `None`。
    fn reselect(&mut self) {
        let items = self.current_items();
        let still_here = self.selected_id.as_deref().is_some_and(|id| items.iter().any(|r| r.id == id));
        if !still_here {
            self.selected_id = items.first().map(|r| r.id.clone());
        }
    }

    fn accept_fetch(&mut self, q: &RequestQuery, issued: u64, result: &Result<FetchData, String>) {
        if *q != self.query {
            return;
        }
        match result {
            Ok(FetchData::Requests(page)) => {
                if issued >= self.accepted {
                    self.data = Some((q.clone(), page.clone()));
                    self.accepted = issued;
                    self.loading = false;
                    self.reselect();
                }
            }
            Ok(_) => {}
            Err(_) => {
                self.loading = false;
            }
        }
    }

    /// `"*"` → 清空全部过滤; `"status:<..>"` / `"vm:<..>"` / `"sub:<..>"` → 设置该维度, 已经是这个
    /// 值时取消它。理论上不会出现的 id 静默忽略。
    fn toggle_filter(&mut self, id: &str) {
        if id == "*" {
            self.query.filters = RequestFilters::default();
            return;
        }
        if let Some(wire) = id.strip_prefix("status:") {
            let Some(status) = parse_status(wire) else { return };
            self.query.filters.status = if self.query.filters.status == Some(status) { None } else { Some(status) };
        } else if let Some(vm) = id.strip_prefix("vm:") {
            self.query.filters.virtual_model_name =
                if self.query.filters.virtual_model_name.as_deref() == Some(vm) { None } else { Some(vm.to_string()) };
        } else if let Some(sub) = id.strip_prefix("sub:") {
            self.query.filters.subscription_id =
                if self.query.filters.subscription_id.as_deref() == Some(sub) { None } else { Some(sub.to_string()) };
        }
    }

    fn apply_filter(&mut self, id: &str) -> Vec<Cmd> {
        self.toggle_filter(id);
        self.query.page = 1;
        self.selected_id = None;
        self.start_fetch()
    }

    fn move_selection(&mut self, delta: isize) {
        let items = self.current_items();
        if items.is_empty() {
            return;
        }
        let idx = self.selected_id.as_deref().and_then(|id| items.iter().position(|r| r.id == id)).unwrap_or(0);
        let next = (idx as isize + delta).clamp(0, items.len() as isize - 1) as usize;
        self.selected_id = Some(items[next].id.clone());
    }

    fn select_first(&mut self) {
        if let Some(first) = self.current_items().first() {
            self.selected_id = Some(first.id.clone());
        }
    }

    fn select_last(&mut self) {
        if let Some(last) = self.current_items().last() {
            self.selected_id = Some(last.id.clone());
        }
    }

    fn next_page(&mut self) -> Option<Action> {
        if self.query.page < self.total_pages() {
            self.query.page += 1;
            self.selected_id = None;
            Some(Action::Refresh)
        } else {
            None
        }
    }

    fn prev_page(&mut self) -> Option<Action> {
        if self.query.page > 1 {
            self.query.page -= 1;
            self.selected_id = None;
            Some(Action::Refresh)
        } else {
            None
        }
    }

    fn clear_filters(&mut self) -> Option<Action> {
        if self.query.filters.is_empty() {
            return None;
        }
        self.query.filters = RequestFilters::default();
        self.query.page = 1;
        self.selected_id = None;
        Some(Action::Refresh)
    }

    fn open_detail(&self, store: &Store, s: &'static Strings) -> Option<Action> {
        let id = self.selected_id.as_deref()?;
        let item = self.current_items().iter().find(|r| r.id == id)?;
        Some(Action::OpenDetail(detail_spec(item, store, s)))
    }

    /// `/`: 条目依次是「清除全部过滤」(仅当前有过滤时)、三个状态、每个虚拟模型、每条订阅; 当前
    /// 生效的那个条目在 hint 后面追加 `" · " + s.filter_active`。
    fn open_filter_picker(&self, store: &Store, s: &'static Strings) -> Action {
        let mut items = Vec::new();
        if !self.query.filters.is_empty() {
            items.push(PickerItem { id: "*".to_string(), label: s.lg_filter_clear.to_string(), hint: None });
        }
        for (status, wire, label) in
            [(RequestStatus::Success, "success", s.lg_status_success), (RequestStatus::Error, "error", s.lg_status_error), (RequestStatus::Timeout, "timeout", s.lg_status_timeout)]
        {
            let active = self.query.filters.status == Some(status);
            items.push(PickerItem { id: format!("status:{wire}"), label: label.to_string(), hint: Some(dim_hint(s.filter_dim_status, active, s)) });
        }
        for vm in store.virtual_models() {
            let active = self.query.filters.virtual_model_name.as_deref() == Some(vm.name.as_str());
            items.push(PickerItem { id: format!("vm:{}", vm.name), label: vm.name.clone(), hint: Some(dim_hint(s.filter_dim_vm, active, s)) });
        }
        for sub in store.subscriptions() {
            let active = self.query.filters.subscription_id.as_deref() == Some(sub.id.as_str());
            items.push(PickerItem { id: format!("sub:{}", sub.id), label: sub.display_name.clone(), hint: Some(dim_hint(s.filter_dim_sub, active, s)) });
        }
        Action::OpenPicker(PickerSpec { tag: PickerTag::LogsFilter, title: s.lg_filter_title.to_string(), items, allow_custom: false, initial: "*".to_string() })
    }

    /// 顶栏右上角的过滤摘要各维度, 按「订阅名 · 虚拟模型 · 状态文案」的顺序。
    fn filter_dims(&self, store: &Store, s: &'static Strings) -> Vec<String> {
        let mut dims = Vec::new();
        if let Some(id) = &self.query.filters.subscription_id {
            dims.push(store.subscription(id).map(|sub| sub.display_name.clone()).unwrap_or_else(|| id.clone()));
        }
        if let Some(vm) = &self.query.filters.virtual_model_name {
            dims.push(vm.clone());
        }
        if let Some(status) = self.query.filters.status {
            dims.push(status_label(status, s).to_string());
        }
        dims
    }

    fn draw_table(&mut self, frame: &mut Frame, area: Rect, ctx: &mut DrawCtx) {
        let s = ctx.s;
        let theme = ctx.theme;
        let store = ctx.store;
        let wide = area.width >= WIDE_THRESHOLD;

        let dims = self.filter_dims(store, s);
        let total = self.current_page().map(|p| p.total).unwrap_or(0);
        let mut page_text = (s.lg_page)(self.query.page, self.total_pages(), total);
        if self.loading {
            let glyph = Throbber::default().throbber_set(BRAILLE_SIX).to_symbol_span(&spinner_state(ctx.tick));
            page_text = format!("{} {page_text}", glyph.content);
        }

        let mut block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(theme.border_style())
            .title_top(format!(" {} ", s.lg_title))
            .title_bottom(Line::from(format!(" {page_text} ")).right_aligned().style(theme.muted_style()))
            .padding(Padding::horizontal(1));
        if !dims.is_empty() {
            let what = dims.join(" · ");
            block = block.title_top(Line::styled(format!(" {} ", (s.filter_summary)(&what)), Style::new().fg(theme.accent)).right_aligned());
        }
        let inner = block.inner(area);
        let capacity = inner.height.saturating_sub(1) as usize; // 减掉表头一行
        self.last_page_rows = capacity.max(1);

        // `current_page()` 为 `None` 时一律显示加载中 (与 `Subscriptions::draw_placeholder` 同一套
        // 约定), 不看 `loading`——`switch_tab` 只在已连接时才发 `Refresh` (`App::switch_tab`), 断线/
        // 重连期间 `loading` 一直是 false, 这时候如果退化成「没有记录」的空态文案就是在撒谎; 加载
        // 失败之后同理 (`accept_fetch` 的 `Err` 分支只清 `loading`, 不产出任何占位数据), 一直显示
        // 加载中直到下一次轮询/手动刷新真的成功——错误信息本身已经由 `App` 弹过 toast, 断线时头部
        // 也已经在显示「重连中」, 不需要这里再额外区分。`data` 存的查询与当前 `self.query` 不一致
        // (Finding 2, 比如断线期间 `show_subscription` 换了过滤但发不出新的 fetch) 时同样当作还没
        // 加载, 不能把上一次查询的行展示在新过滤下面。
        if self.current_page().is_none() {
            frame.render_widget(block, area);
            let mut state = spinner_state(ctx.tick);
            let throbber = Throbber::default().label(s.loading).throbber_set(BRAILLE_SIX).style(theme.muted_style());
            frame.render_stateful_widget(throbber, inner.centered_vertically(Constraint::Length(1)), &mut state);
            return;
        }

        let items = self.current_items();
        if items.is_empty() {
            frame.render_widget(block, area);
            draw_empty_message(frame, inner, theme, s, !self.query.filters.is_empty());
            return;
        }

        let mut cols = Cols::new(s, wide);
        // `Constraint::Fill(1)` (模型名) 的实际宽度: `inner.width` 已经减掉了边框 + 内距, 再减选中
        // 前缀 (`HIGHLIGHT_COL`)、其余定宽列、以及列间距 (`column_spacing(1)`, 列数 - 1 个间隔)——
        // 不这样算的话 `format::fit` 不知道该截到多宽, 模型名超长时 ratatui 会不带省略号地硬切
        // (Finding 5)。
        let fixed_cols = cols.time as u16
            + cols.status as u16
            + cols.vm as u16
            + cols.sub as u16
            + if wide { cols.client as u16 } else { 0 }
            + cols.latency as u16
            + cols.tokens as u16;
        let gap_count: u16 = if wide { 7 } else { 6 }; // 8 (wide) / 7 (窄) 列各少 1 个间隔
        let mut model_col = inner.width.saturating_sub(HIGHLIGHT_COL).saturating_sub(fixed_cols).saturating_sub(gap_count) as usize;
        // 模型列至少放得下自己的表头: 定宽列的表头 / 「✕ 超时」更宽的语言 (日文) 在 80 列上会把它挤到
        // 连表头都截成片段, 差额从订阅列借——订阅名本来就按 `fit` 截断, 模型列放不下时连一个有意义的
        // 字都显示不出来。中英文的模型列本来就不窄于表头, 不借。
        let borrow = widest([s.lg_col_model]).saturating_sub(model_col).min(cols.sub.saturating_sub(SUB_COL_MIN));
        cols.sub -= borrow;
        model_col += borrow;
        let mut header_cells =
            vec![Cell::from(fit(s.lg_col_time, cols.time)), Cell::from(fit(s.lg_col_status, cols.status)), Cell::from(fit(s.lg_col_vm, cols.vm)), Cell::from(fit(s.lg_col_sub, cols.sub))];
        if wide {
            header_cells.push(Cell::from(fit(s.lg_col_client, cols.client)));
        }
        header_cells.push(Cell::from(fit(s.lg_col_model, model_col)));
        header_cells.push(Cell::from(fit(s.lg_col_latency, cols.latency)));
        header_cells.push(Cell::from(fit(s.lg_col_tokens, cols.tokens)));
        let header = Row::new(header_cells).style(theme.muted_style().add_modifier(Modifier::BOLD));

        let mut widths =
            vec![Constraint::Length(cols.time as u16), Constraint::Length(cols.status as u16), Constraint::Length(cols.vm as u16), Constraint::Length(cols.sub as u16)];
        if wide {
            widths.push(Constraint::Length(cols.client as u16));
        }
        widths.push(Constraint::Fill(1));
        widths.push(Constraint::Length(cols.latency as u16));
        widths.push(Constraint::Length(cols.tokens as u16));

        let rows: Vec<Row> = items.iter().map(|item| build_row(item, store, theme, s, ctx.now_ms, ctx.tz, wide, &cols, model_col)).collect();

        let idx = self.selected_id.as_deref().and_then(|id| items.iter().position(|r| r.id == id));
        self.table_state.select(idx);

        let table = Table::new(rows, widths)
            .header(header)
            .column_spacing(1)
            .highlight_symbol("▌ ")
            .highlight_spacing(HighlightSpacing::Always)
            .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .block(block);
        frame.render_stateful_widget(&table, area, &mut self.table_state);
    }
}

fn draw_empty_message(frame: &mut Frame, inner: Rect, theme: &Theme, s: &'static Strings, filtered: bool) {
    let msg = if filtered { s.lg_empty_filtered } else { s.lg_empty };
    frame.render_widget(Line::styled(msg, theme.muted_style()).centered(), inner.centered_vertically(Constraint::Length(1)));
}

fn dim_hint(dim: &'static str, active: bool, s: &'static Strings) -> String {
    if active {
        format!("{dim} · {}", s.filter_active)
    } else {
        dim.to_string()
    }
}

fn parse_status(wire: &str) -> Option<RequestStatus> {
    match wire {
        "success" => Some(RequestStatus::Success),
        "error" => Some(RequestStatus::Error),
        "timeout" => Some(RequestStatus::Timeout),
        _ => None,
    }
}

fn status_label(status: RequestStatus, s: &'static Strings) -> &'static str {
    match status {
        RequestStatus::Success => s.lg_status_success,
        RequestStatus::Error => s.lg_status_error,
        RequestStatus::Timeout => s.lg_status_timeout,
        RequestStatus::Unknown => s.lg_status_unknown,
    }
}

/// 状态列里超时那一格的文字; 它是状态列里最宽的内容, [`Cols::new`] 按它量列宽。用短形: 状态列
/// 多一列, 80 列终端上就从订阅 / 模型列里少一列。
fn timeout_cell(s: &Strings) -> String {
    format!("✕ {}", s.lg_status_timeout_short)
}

fn status_cell_text(item: &RequestLog, s: &'static Strings) -> String {
    let http = || item.http_status.map(|c| c.to_string()).unwrap_or_else(|| "—".to_string());
    match item.status {
        RequestStatus::Success => format!("✓ {}", http()),
        RequestStatus::Error => format!("✕ {}", http()),
        RequestStatus::Timeout => timeout_cell(s),
        RequestStatus::Unknown => "? —".to_string(),
    }
}

fn status_cell_style(status: RequestStatus, theme: &Theme) -> Style {
    match status {
        RequestStatus::Success => Style::new().fg(theme.ok),
        RequestStatus::Error => Style::new().fg(theme.err),
        RequestStatus::Timeout => Style::new().fg(theme.warn),
        RequestStatus::Unknown => theme.muted_style(),
    }
}

fn tokens_text(item: &RequestLog) -> String {
    if item.input_tokens.is_none() && item.output_tokens.is_none() {
        return "—".to_string();
    }
    let i = item.input_tokens.map(compact).unwrap_or_else(|| "—".to_string());
    let o = item.output_tokens.map(compact).unwrap_or_else(|| "—".to_string());
    format!("{i}/{o}")
}

/// 订阅列展示文案: 取 `store` 的备注名, 找不到用 id 前 8 位 (muted)。
fn subscription_text(item: &RequestLog, store: &Store) -> (String, bool) {
    match store.subscription(&item.subscription_id) {
        Some(sub) => (sub.display_name.clone(), false),
        None => (item.subscription_id.chars().take(8).collect(), true),
    }
}

/// `model_col`: `Fill(1)` 列的实际宽度 (调用方按 `draw_table` 里的公式手算), 超长模型名用 `fit`
/// 截断补省略号, 不让 ratatui 不带提示地硬切 (Finding 5)。
#[allow(clippy::too_many_arguments)] // 与订阅页 / 实时路由页的表格行构造函数同一条先例
fn build_row(
    item: &RequestLog,
    store: &Store,
    theme: &Theme,
    s: &'static Strings,
    now_ms: i64,
    tz: crate::format::Tz,
    wide: bool,
    cols: &Cols,
    model_col: usize,
) -> Row<'static> {
    let time_cell = Cell::from(fit(&short_stamp(item.timestamp, now_ms, tz), cols.time));
    let status_style = status_cell_style(item.status, theme);
    let status_cell = Cell::from(ratatui::text::Span::styled(fit(&status_cell_text(item, s), cols.status), status_style));
    let vm_cell = Cell::from(fit(&item.virtual_model_name, cols.vm));
    let (sub_text, sub_muted) = subscription_text(item, store);
    let sub_style = if sub_muted { theme.muted_style() } else { Style::default() };
    let sub_cell = Cell::from(ratatui::text::Span::styled(fit(&sub_text, cols.sub), sub_style));

    let mut cells = vec![time_cell, status_cell, vm_cell, sub_cell];
    if wide {
        let client = item.client_tool.as_deref().unwrap_or("—");
        cells.push(Cell::from(fit(client, cols.client)));
    }
    cells.push(Cell::from(fit(&item.real_model_name, model_col)));
    let latency = item.total_latency_ms.map(duration).unwrap_or_else(|| "—".to_string());
    cells.push(Cell::from(fit(&latency, cols.latency)));
    cells.push(Cell::from(fit(&tokens_text(item), cols.tokens)));
    Row::new(cells)
}

fn opt_thousands(v: Option<i64>) -> String {
    v.map(thousands).unwrap_or_else(|| "—".to_string())
}

fn opt_number(v: Option<i64>) -> String {
    v.map(|n| n.to_string()).unwrap_or_else(|| "—".to_string())
}

/// 解析 `tool_use_names` (JSON 字符串数组): 同名合并计数, 保持首次出现的顺序; 数组里的 `"…"`
/// 表示被截断 (返回 true, 不计入名字); 非字符串或空串记作 `""`。不是合法 JSON 数组 → (空, false)。
/// 与桌面端 `parseToolNames` 同规则 (空名到底显示成什么由展示层——这里是 `detail_spec` ——决定,
/// 不在这个纯解析函数里做)。
pub fn tool_names(raw: &str) -> (Vec<(String, usize)>, bool) {
    let Ok(serde_json::Value::Array(values)) = serde_json::from_str::<serde_json::Value>(raw) else {
        return (Vec::new(), false);
    };
    let mut truncated = false;
    let mut ordered: Vec<(String, usize)> = Vec::new();
    for v in values {
        let name = match v {
            serde_json::Value::String(text) if text == "…" => {
                truncated = true;
                continue;
            }
            serde_json::Value::String(text) => text,
            _ => String::new(),
        };
        match ordered.iter_mut().find(|(n, _)| *n == name) {
            Some(entry) => entry.1 += 1,
            None => ordered.push((name, 1)),
        }
    }
    (ordered, truncated)
}

/// 详情弹窗的内容 (纯函数)。分组与桌面端 `src/components/RequestDetailDialog.tsx` 一致: 基本信息
/// 恒出现; 思考强度 / 工具调用 / 错误信息 / 上游响应四节各自只在有对应数据时才出现。
pub fn detail_spec(item: &RequestLog, store: &Store, s: &Strings) -> DetailSpec {
    let mut rows = vec![DetailRow::Section(s.lg_d_basic.to_string())];
    rows.push(DetailRow::Stamp { label: s.lg_d_time.to_string(), ms: item.timestamp });
    rows.push(DetailRow::Field { label: s.lg_d_id.to_string(), value: item.id.clone(), tone: Tone::Normal });

    let (status_text, status_tone) = match item.status {
        RequestStatus::Success => (s.lg_status_success, Tone::Ok),
        RequestStatus::Error => (s.lg_status_error, Tone::Err),
        RequestStatus::Timeout => (s.lg_status_timeout, Tone::Warn),
        RequestStatus::Unknown => (s.lg_status_unknown, Tone::Muted),
    };
    rows.push(DetailRow::Field { label: s.lg_d_status.to_string(), value: (s.lg_d_status_value)(status_text, item.http_status), tone: status_tone });

    rows.push(DetailRow::Field { label: s.lg_d_vm.to_string(), value: item.virtual_model_name.clone(), tone: Tone::Normal });
    rows.push(DetailRow::Field { label: s.lg_d_real_model.to_string(), value: item.real_model_name.clone(), tone: Tone::Normal });

    if let Some(resp_model) = &item.response_model_name {
        if resp_model != &item.real_model_name {
            rows.push(DetailRow::Field { label: s.lg_d_resp_model.to_string(), value: resp_model.clone(), tone: Tone::Normal });
        }
    }

    let sub_name =
        store.subscription(&item.subscription_id).map(|sub| sub.display_name.clone()).unwrap_or_else(|| item.subscription_id.chars().take(8).collect());
    rows.push(DetailRow::Field { label: s.lg_d_sub.to_string(), value: sub_name, tone: Tone::Normal });
    rows.push(DetailRow::Field { label: s.lg_d_provider.to_string(), value: format!("{} / {}", item.provider_id, item.endpoint_id), tone: Tone::Normal });

    let latency = item.total_latency_ms.map(duration).unwrap_or_else(|| "—".to_string());
    rows.push(DetailRow::Field { label: s.lg_d_latency.to_string(), value: latency, tone: Tone::Normal });
    rows.push(DetailRow::Field {
        label: s.lg_d_streaming.to_string(),
        value: if item.is_streaming { s.lg_yes } else { s.lg_no }.to_string(),
        tone: Tone::Normal,
    });
    rows.push(DetailRow::Field {
        label: s.lg_d_tokens.to_string(),
        value: (s.lg_d_tokens_value)(
            &opt_thousands(item.input_tokens),
            &opt_thousands(item.output_tokens),
            &opt_thousands(item.cache_creation_tokens),
            &opt_thousands(item.cache_read_tokens),
        ),
        tone: Tone::Normal,
    });

    if let Some(tool) = &item.client_tool {
        let value = match &item.client_version {
            Some(v) => format!("{tool} {v}"),
            None => tool.clone(),
        };
        rows.push(DetailRow::Field { label: s.lg_d_client.to_string(), value, tone: Tone::Normal });
    }
    if let Some(ip) = &item.client_ip {
        rows.push(DetailRow::Field { label: s.lg_d_ip.to_string(), value: ip.clone(), tone: Tone::Normal });
    }
    if let Some(ua) = &item.client_user_agent {
        rows.push(DetailRow::Field { label: s.lg_d_ua.to_string(), value: ua.clone(), tone: Tone::Normal });
    }
    if let Some(entry) = &item.entry_kind {
        rows.push(DetailRow::Field { label: s.lg_d_entry.to_string(), value: format!("/v1/{entry}"), tone: Tone::Normal });
    }
    if let Some(http_version) = &item.downstream_http_version {
        rows.push(DetailRow::Field { label: s.lg_d_http_version.to_string(), value: http_version.clone(), tone: Tone::Normal });
    }

    let net = &item.net;
    if net.prep_ms.is_some() {
        rows.push(DetailRow::Section(s.lg_d_net.to_string()));
        let ms = |v: Option<i64>| v.map(|v| duration(v.max(0))).unwrap_or_else(|| "—".to_string());
        let mut conn = match net.conn_reused {
            Some(true) => s.lg_d_net_reused.to_string(),
            Some(false) => s.lg_d_net_new.to_string(),
            None => "—".to_string(),
        };
        if net.via_proxy == Some(true) {
            conn.push_str(" · ");
            conn.push_str(s.lg_d_net_proxy);
        }
        rows.push(DetailRow::Field { label: s.lg_d_net_conn.to_string(), value: conn, tone: Tone::Normal });
        rows.push(DetailRow::Field { label: s.lg_d_net_prep.to_string(), value: ms(net.prep_ms), tone: Tone::Normal });
        if net.conn_reused == Some(false) {
            rows.push(DetailRow::Field { label: s.lg_d_net_dns.to_string(), value: ms(net.dns_ms), tone: Tone::Normal });
            rows.push(DetailRow::Field { label: s.lg_d_net_connect.to_string(), value: ms(net.connect_ms), tone: Tone::Normal });
        }
        rows.push(DetailRow::Field { label: s.lg_d_net_headers.to_string(), value: ms(net.headers_ms), tone: Tone::Normal });
        if let Some(addr) = &net.remote_addr {
            rows.push(DetailRow::Field { label: s.lg_d_net_remote.to_string(), value: addr.clone(), tone: Tone::Normal });
        }
    }

    if item.client_effort.is_some() || item.effective_effort.is_some() || item.effort_source.is_some() || item.upstream_effort.is_some() {
        rows.push(DetailRow::Section(s.lg_d_effort.to_string()));
        rows.push(DetailRow::Field {
            label: s.lg_d_effort_client.to_string(),
            value: item.client_effort.clone().unwrap_or_else(|| "—".to_string()),
            tone: Tone::Normal,
        });
        let mut effective = item.effective_effort.clone().unwrap_or_else(|| "—".to_string());
        if let Some(src) = &item.effort_source {
            effective.push_str(&(s.lg_d_effort_source_suffix)(&(s.lg_effort_source)(src)));
        }
        rows.push(DetailRow::Field { label: s.lg_d_effort_effective.to_string(), value: effective, tone: Tone::Normal });
        let upstream = item.upstream_effort.clone().unwrap_or_else(|| s.lg_d_effort_upstream_none.to_string());
        rows.push(DetailRow::Field { label: s.lg_d_effort_upstream.to_string(), value: upstream, tone: Tone::Normal });
    }

    let has_tools = item.stop_reason.is_some()
        || item.tools_offered_count.is_some()
        || item.tool_result_count.is_some()
        || item.tool_use_count.is_some()
        || item.tool_use_names.is_some();
    if has_tools {
        rows.push(DetailRow::Section(s.lg_d_tools.to_string()));
        rows.push(DetailRow::Field {
            label: s.lg_d_stop_reason.to_string(),
            value: item.stop_reason.clone().unwrap_or_else(|| "—".to_string()),
            tone: Tone::Normal,
        });
        rows.push(DetailRow::Field { label: s.lg_d_tools_offered.to_string(), value: opt_number(item.tools_offered_count), tone: Tone::Normal });
        rows.push(DetailRow::Field { label: s.lg_d_tool_results.to_string(), value: opt_number(item.tool_result_count), tone: Tone::Normal });
        rows.push(DetailRow::Field { label: s.lg_d_tool_uses.to_string(), value: opt_number(item.tool_use_count), tone: Tone::Normal });

        let (names, truncated) = tool_names(item.tool_use_names.as_deref().unwrap_or(""));
        if !names.is_empty() || truncated {
            let parts: Vec<String> = names
                .iter()
                .map(|(name, count)| {
                    let label = if name.is_empty() { s.lg_d_unnamed.to_string() } else { name.clone() };
                    if *count > 1 {
                        format!("{label} ×{count}")
                    } else {
                        label
                    }
                })
                .collect();
            let mut value = parts.join(", ");
            if truncated {
                value.push(' ');
                value.push_str(s.lg_d_truncated);
            }
            rows.push(DetailRow::Field { label: s.lg_d_tool_names.to_string(), value, tone: Tone::Normal });
        }
    }

    if let Some(err) = &item.error_message {
        rows.push(DetailRow::Section(s.lg_d_error.to_string()));
        rows.push(DetailRow::Text { text: err.clone(), tone: Tone::Err });
    }

    if let Some(body) = &item.upstream_response_body {
        rows.push(DetailRow::Section(s.lg_d_body.to_string()));
        let pretty = serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
            .unwrap_or_else(|| body.clone());
        rows.push(DetailRow::Text { text: pretty, tone: Tone::Normal });
    }

    DetailSpec { title: s.lg_d_title.to_string(), rows }
}

impl Component for Logs {
    fn handle_key(&mut self, key: KeyEvent, store: &Store, s: &'static Strings) -> Option<Action> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_selection(-1);
                None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_selection(1);
                None
            }
            KeyCode::Char('g') => {
                self.select_first();
                None
            }
            KeyCode::Char('G') => {
                self.select_last();
                None
            }
            KeyCode::PageUp => {
                self.move_selection(-(self.last_page_rows.max(1) as isize));
                None
            }
            KeyCode::PageDown => {
                self.move_selection(self.last_page_rows.max(1) as isize);
                None
            }
            KeyCode::Char('n') => self.next_page(),
            KeyCode::Char('p') => self.prev_page(),
            KeyCode::Char('/') => Some(self.open_filter_picker(store, s)),
            KeyCode::Esc => self.clear_filters(),
            KeyCode::Enter => self.open_detail(store, s),
            _ => None,
        }
    }

    fn update(&mut self, action: &Action, _store: &Store, _s: &'static Strings) -> Vec<Cmd> {
        match action {
            Action::Connected { .. } | Action::Refresh => self.start_fetch(),
            Action::Poll => {
                if self.query.page == 1 {
                    self.start_fetch()
                } else {
                    Vec::new()
                }
            }
            Action::FetchDone { fetch: Fetch::Requests(q), issued, result } => {
                self.accept_fetch(q, *issued, result);
                Vec::new()
            }
            Action::PickerDone { tag: PickerTag::LogsFilter, choice: PickerChoice::Item(id) } => self.apply_filter(id),
            _ => Vec::new(),
        }
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, ctx: &mut DrawCtx) {
        self.draw_table(frame, area, ctx);
    }

    fn hints(&self, s: &'static Strings) -> Vec<Hint<'static>> {
        let mut hints = vec![("↑↓", s.key_select), ("n/p", s.key_page)];
        if self.selected_id.is_some() {
            hints.push(("⏎", s.key_detail));
        }
        hints.push(("/", s.key_filter));
        if !self.query.filters.is_empty() {
            hints.push(("Esc", s.key_clear_filter));
        }
        hints
    }

    fn help(&self, s: &'static Strings) -> &'static [(&'static str, &'static str)] {
        s.lg_help_rows
    }

    fn is_dirty(&self) -> bool {
        #[cfg(test)]
        {
            self.force_dirty
        }
        #[cfg(not(test))]
        {
            false
        }
    }

    fn discard_changes(&mut self) {
        #[cfg(test)]
        {
            self.force_dirty = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::*;
    use crate::client::dto::Subscription;
    use crate::format::Tz;
    use crate::fx::Fx;
    use crate::i18n::ZH;
    use crate::theme::ColorMode;

    const NOW: i64 = 1_700_000_000_000;

    /// 只用来渲染 `Logs::draw` 本身的最小 `DrawCtx` 环境——日志页不用 `fx`/`busy`/`last_outcome`,
    /// 这几项给空值即可 (Finding 2 的渲染断言需要真的画一帧, 不能只看内部状态)。
    fn render(logs: &mut Logs, store: &Store, width: u16, height: u16) -> String {
        let theme = Theme::new(ColorMode::TrueColor);
        let mut fx = Fx::new(false);
        let busy = HashMap::new();
        let last_outcome = HashMap::new();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| {
                let area = f.area();
                let mut ctx = DrawCtx { theme: &theme, s: &ZH, now_ms: NOW, tick: 0, fx: &mut fx, store, busy: &busy, last_outcome: &last_outcome, tz: Tz::Fixed(8 * 3600) };
                logs.draw(f, area, &mut ctx);
            })
            .unwrap();
        terminal.backend().to_string()
    }

    fn store_with(subs: Vec<Subscription>) -> Store {
        let mut store = Store::default();
        store.apply_subscriptions(1, subs);
        store
    }

    fn sub(id: &str, name: &str) -> Subscription {
        Subscription {
            id: id.into(),
            display_name: name.into(),
            provider_display_name: "p".into(),
            provider_names: None,
            enabled: true,
            state: crate::client::dto::SubscriptionState::Healthy,
            cooldown_until: None,
            last_error_message: None,
            is_dispatchable: true,
            quota_usage: vec![],
            provider_id: "p".into(),
            base_url: "https://example.invalid".into(),
            auth_type: "api_key".into(),
            endpoint_protocol: "messages".into(),
            model_slots: crate::client::dto::ModelSlots { fable: "d".into(), opus: "a".into(), sonnet: "b".into(), haiku: "c".into(), fallback: String::new(), jev: String::new() },
            slot_efforts: Default::default(),
            slot_models_now: Default::default(),
            slot_schedules: Default::default(),
            referenced_by: vec![],
            balance_supported: false,
            balance_cache: None,
            model_cache: None,
        }
    }

    fn log(id: &str, sub_id: &str, status: RequestStatus, ts: i64) -> RequestLog {
        RequestLog {
            id: id.into(),
            timestamp: ts,
            virtual_model_name: "model-sonnet".into(),
            subscription_id: sub_id.into(),
            provider_id: "zhipu".into(),
            endpoint_id: "default".into(),
            real_model_name: "glm-4.6".into(),
            response_model_name: None,
            is_streaming: true,
            status,
            http_status: Some(200),
            total_latency_ms: Some(1800),
            input_tokens: Some(12300),
            output_tokens: Some(3400),
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
            net: Default::default(),
        }
    }

    fn full_log() -> RequestLog {
        RequestLog {
            id: "req-1".into(),
            timestamp: NOW,
            virtual_model_name: "model-sonnet".into(),
            subscription_id: "1".into(),
            provider_id: "zhipu".into(),
            endpoint_id: "default".into(),
            real_model_name: "glm-4.6".into(),
            response_model_name: Some("glm-4.6-0620".into()),
            is_streaming: true,
            status: RequestStatus::Success,
            http_status: Some(200),
            total_latency_ms: Some(1800),
            input_tokens: Some(12300),
            output_tokens: Some(3400),
            cache_creation_tokens: Some(0),
            cache_read_tokens: Some(100),
            error_message: Some("上游返回 429".into()),
            upstream_response_body: Some(r#"{"a":1}"#.into()),
            client_tool: Some("claude-code".into()),
            client_user_agent: Some("cc/1.0".into()),
            client_version: Some("1.0".into()),
            client_ip: Some("127.0.0.1".into()),
            entry_kind: Some("messages".into()),
            downstream_http_version: Some("HTTP/2".into()),
            client_effort: Some("high".into()),
            effective_effort: Some("high".into()),
            effort_source: Some("slot".into()),
            upstream_effort: Some("high".into()),
            stop_reason: Some("end_turn".into()),
            tools_offered_count: Some(3),
            tool_result_count: Some(2),
            tool_use_count: Some(3),
            tool_use_names: Some(r#"["Read","Bash","Read","…"]"#.into()),
            net: crate::client::dto::NetTiming {
                prep_ms: Some(4),
                dns_ms: Some(30),
                connect_ms: Some(180),
                headers_ms: Some(260),
                conn_reused: Some(false),
                via_proxy: Some(true),
                remote_addr: Some("1.2.3.4:443".into()),
                upstream_stream: Some(true),
            },
        }
    }

    fn page(items: Vec<RequestLog>, total: i64) -> RequestPage {
        RequestPage { items, total }
    }

    #[test]
    fn tool_names_merge_counts_and_detect_truncation() {
        assert_eq!(tool_names(r#"["Read","Bash","Read","…"]"#), (vec![("Read".to_string(), 2), ("Bash".to_string(), 1)], true));
        assert_eq!(tool_names("not json"), (vec![], false));
        assert_eq!(tool_names(r#"[1, ""]"#), (vec![("".to_string(), 2)], false));
    }

    #[test]
    fn detail_spec_has_the_same_groups_as_the_desktop_dialog() {
        let store = store_with(vec![sub("1", "智谱主号")]);
        let full = detail_spec(&full_log(), &store, &ZH);
        let sections: Vec<&str> =
            full.rows.iter().filter_map(|r| if let DetailRow::Section(t) = r { Some(t.as_str()) } else { None }).collect();
        assert_eq!(sections, vec![ZH.lg_d_basic, ZH.lg_d_net, ZH.lg_d_effort, ZH.lg_d_tools, ZH.lg_d_error, ZH.lg_d_body], "{full:?}");
        let field = |l: &str| {
            full.rows.iter().find_map(|r| if let DetailRow::Field { label, value, .. } = r { (label == l).then(|| value.clone()) } else { None })
        };
        assert_eq!(field(ZH.lg_d_net_conn).as_deref(), Some("新建连接 · 经代理"));
        assert_eq!(field(ZH.lg_d_net_connect).as_deref(), Some("180ms"));
        assert_eq!(field(ZH.lg_d_net_remote).as_deref(), Some("1.2.3.4:443"));

        let effort_row = full
            .rows
            .iter()
            .find_map(|r| if let DetailRow::Field { label, value, .. } = r { (label == ZH.lg_d_effort_effective).then_some(value) } else { None })
            .unwrap();
        assert!(effort_row.contains("（订阅槽位强制）"), "{effort_row}");

        let tool_names_row = full
            .rows
            .iter()
            .find_map(|r| if let DetailRow::Field { label, value, .. } = r { (label == ZH.lg_d_tool_names).then_some(value) } else { None })
            .unwrap();
        assert!(tool_names_row.contains("Read ×2, Bash (已截断)"), "{tool_names_row}");

        let body_row = full.rows.iter().rev().find_map(|r| if let DetailRow::Text { text, .. } = r { Some(text) } else { None }).unwrap();
        assert!(body_row.contains('\n') || body_row == "{\n  \"a\": 1\n}", "上游响应应该是 pretty JSON, 实际 {body_row:?}");

        let mut empty_log = log("req-2", "9", RequestStatus::Unknown, NOW);
        empty_log.http_status = None;
        empty_log.total_latency_ms = None;
        empty_log.input_tokens = None;
        empty_log.output_tokens = None;
        let store = Store::default();
        let bare = detail_spec(&empty_log, &store, &ZH);
        let bare_sections: Vec<&str> =
            bare.rows.iter().filter_map(|r| if let DetailRow::Section(t) = r { Some(t.as_str()) } else { None }).collect();
        assert_eq!(bare_sections, vec![ZH.lg_d_basic], "全 None 记录只应该有基本信息一节\n{bare:?}");
    }

    #[test]
    fn stale_and_mismatched_results_are_ignored() {
        let mut logs = Logs::default();
        logs.query.page = 2;
        let p2 = logs.query.clone();
        let mut p1 = p2.clone();
        p1.page = 1;

        // p1 的结果 (查询已经不是当前的了) 应该被忽略。
        logs.accept_fetch(&p1, 99, &Ok(FetchData::Requests(page(vec![log("a", "1", RequestStatus::Success, NOW)], 1))));
        assert!(logs.data.is_none(), "查询不匹配的结果应该被忽略");

        // p2 issued=5 被接受。
        logs.accept_fetch(&p2, 5, &Ok(FetchData::Requests(page(vec![log("a", "1", RequestStatus::Success, NOW)], 1))));
        assert_eq!(logs.accepted, 5);
        assert_eq!(logs.data.as_ref().unwrap().1.items.len(), 1);

        // p2 issued=3 (晚到的旧结果) 应该被忽略。
        logs.accept_fetch(&p2, 3, &Ok(FetchData::Requests(page(vec![], 0))));
        assert_eq!(logs.data.as_ref().unwrap().1.items.len(), 1, "issued 更小的结果不该覆盖");

        // p2 的 Err 应该让 loading 变回 false (不管 issued)。
        logs.loading = true;
        logs.accept_fetch(&p2, 1, &Err("boom".into()));
        assert!(!logs.loading);
    }

    #[test]
    fn poll_only_reloads_the_first_page() {
        let store = Store::default();
        let mut logs = Logs::default();
        assert_eq!(logs.update(&Action::Poll, &store, &ZH), vec![Cmd::Fetch(Fetch::Requests(RequestQuery::default()))]);

        logs.query.page = 2;
        assert!(logs.update(&Action::Poll, &store, &ZH).is_empty(), "第 2 页不该被 Poll 刷新");
        assert_eq!(logs.update(&Action::Refresh, &store, &ZH), vec![Cmd::Fetch(Fetch::Requests(logs.query.clone()))], "Refresh 不受页码限制");
    }

    #[test]
    fn n_and_p_stay_within_bounds() {
        let store = Store::default();
        let s = &ZH;
        let mut logs = Logs::default();
        logs.data = Some((logs.query.clone(), page(vec![log("a", "1", RequestStatus::Success, NOW)], 120)));
        assert_eq!(logs.total_pages(), 3);

        logs.query.page = 3;
        assert_eq!(logs.handle_key(key(KeyCode::Char('n')), &store, s), None, "已经是最后一页, n 不该翻页");

        logs.query.page = 1;
        assert_eq!(logs.handle_key(key(KeyCode::Char('p')), &store, s), None, "已经是第一页, p 不该翻页");

        assert_eq!(logs.handle_key(key(KeyCode::Char('n')), &store, s), Some(Action::Refresh));
        assert_eq!(logs.query.page, 2);
    }

    /// Finding 2: 切换过滤之后, 在匹配的结果真的落地之前, `total_pages()` 不该继续信任上一个查询
    /// 算出来的页数——否则 `n` 会在还没加载的新过滤下面把用户翻到一个凭空算出来的「第 2 页」。
    #[test]
    fn paging_is_bounded_by_the_current_querys_total() {
        let store = Store::default();
        let s = &ZH;
        let mut logs = Logs::default();
        let q = logs.query.clone();
        logs.accept_fetch(&q, 1, &Ok(FetchData::Requests(page(vec![log("a", "1", RequestStatus::Success, NOW)], 120))));
        assert_eq!(logs.total_pages(), 3, "准备: 无过滤时应该有 3 页");

        logs.update(&Action::PickerDone { tag: PickerTag::LogsFilter, choice: PickerChoice::Item("status:error".into()) }, &store, s);
        assert_eq!(logs.total_pages(), 1, "查询已经变了, 旧查询算出来的 total 不该再被信任");
        assert_eq!(logs.handle_key(key(KeyCode::Char('n')), &store, s), None, "匹配结果落地之前 n 不该翻页");

        let q2 = logs.query.clone();
        logs.accept_fetch(&q2, 2, &Ok(FetchData::Requests(page(vec![log("b", "1", RequestStatus::Error, NOW)], 120))));
        assert_eq!(logs.total_pages(), 3, "匹配的结果落地后应该恢复");
        assert_eq!(logs.handle_key(key(KeyCode::Char('n')), &store, s), Some(Action::Refresh), "落地后 n 应该恢复正常翻页");
    }

    /// Finding 2: `show_subscription` (实时路由页 `⏎` 跳转过来) 直接改 `query`, 断线期间发不出新
    /// 的 fetch, `data` 还留着上一个查询的结果——渲染必须当作「还没加载」, 不能把旧查询的行 (以及
    /// 旧查询的订阅备注名) 展示在新过滤下面。
    #[test]
    fn rows_of_another_query_are_not_shown_under_the_new_filter() {
        let store = store_with(vec![sub("1", "智谱主号"), sub("3", "另一条订阅")]);
        let mut logs = Logs::default();
        let q = logs.query.clone();
        logs.accept_fetch(&q, 1, &Ok(FetchData::Requests(page(vec![log("a", "1", RequestStatus::Success, NOW)], 1))));
        assert_eq!(logs.current_items().len(), 1, "准备: 应该先有一页无过滤的数据落地");

        logs.show_subscription("3");

        let out = render(&mut logs, &store, 100, 24);
        assert!(out.contains(ZH.loading), "过滤已经变了但还没发出新的 fetch, 应该显示加载中\n{out}");
        assert!(!out.contains("智谱主号"), "旧查询 (订阅 1) 的行不该出现在新过滤 (订阅 3) 下面\n{out}");
    }

    #[test]
    fn filter_picker_toggles_dimensions_and_resets_to_page_one() {
        let store = Store::default();
        let mut logs = Logs::default();
        logs.query.page = 2;

        let cmds = logs.update(
            &Action::PickerDone { tag: PickerTag::LogsFilter, choice: PickerChoice::Item("status:error".into()) },
            &store,
            &ZH,
        );
        assert_eq!(logs.query.filters.status, Some(RequestStatus::Error));
        assert_eq!(logs.query.page, 1);
        assert!(logs.loading);
        assert_eq!(cmds, vec![Cmd::Fetch(Fetch::Requests(logs.query.clone()))]);

        // 再选一次同一个值应该取消它。
        logs.update(&Action::PickerDone { tag: PickerTag::LogsFilter, choice: PickerChoice::Item("status:error".into()) }, &store, &ZH);
        assert_eq!(logs.query.filters.status, None);

        logs.update(&Action::PickerDone { tag: PickerTag::LogsFilter, choice: PickerChoice::Item("vm:model-opus".into()) }, &store, &ZH);
        assert_eq!(logs.query.filters.virtual_model_name, Some("model-opus".into()));

        logs.update(&Action::PickerDone { tag: PickerTag::LogsFilter, choice: PickerChoice::Item("sub:1".into()) }, &store, &ZH);
        assert_eq!(logs.query.filters.subscription_id, Some("1".into()));

        logs.update(&Action::PickerDone { tag: PickerTag::LogsFilter, choice: PickerChoice::Item("*".into()) }, &store, &ZH);
        assert!(logs.query.filters.is_empty());
    }

    #[test]
    fn esc_clears_filters() {
        let store = Store::default();
        let s = &ZH;
        let mut logs = Logs::default();
        assert_eq!(logs.handle_key(key(KeyCode::Esc), &store, s), None, "没有过滤时 Esc 不该产出动作");

        logs.query.filters.status = Some(RequestStatus::Error);
        logs.query.page = 2;
        assert_eq!(logs.handle_key(key(KeyCode::Esc), &store, s), Some(Action::Refresh));
        assert!(logs.query.filters.is_empty());
        assert_eq!(logs.query.page, 1);
    }

    #[test]
    fn selection_follows_the_request_id_across_reloads() {
        let mut logs = Logs::default();
        let q = logs.query.clone();
        logs.accept_fetch(
            &q,
            1,
            &Ok(FetchData::Requests(page(vec![log("a", "1", RequestStatus::Success, NOW), log("b", "1", RequestStatus::Success, NOW)], 2))),
        );
        assert_eq!(logs.selected_id.as_deref(), Some("a"), "首次加载应该选中第一条");

        logs.selected_id = Some("b".to_string());
        // 重新加载, "b" 仍在新结果里, 应该继续选中它。
        logs.accept_fetch(&q, 2, &Ok(FetchData::Requests(page(vec![log("a", "1", RequestStatus::Success, NOW), log("b", "1", RequestStatus::Success, NOW)], 2))));
        assert_eq!(logs.selected_id.as_deref(), Some("b"));

        // "b" 不再出现, 应该改选第一条。
        logs.accept_fetch(&q, 3, &Ok(FetchData::Requests(page(vec![log("c", "1", RequestStatus::Success, NOW)], 1))));
        assert_eq!(logs.selected_id.as_deref(), Some("c"));

        // 没有条目时应该是 None。
        logs.accept_fetch(&q, 4, &Ok(FetchData::Requests(page(vec![], 0))));
        assert_eq!(logs.selected_id, None);
    }

    #[test]
    fn show_subscription_replaces_other_filters() {
        let mut logs = Logs::default();
        logs.query.filters.status = Some(RequestStatus::Error);
        logs.query.filters.virtual_model_name = Some("model-opus".into());
        logs.query.page = 3;
        logs.selected_id = Some("x".into());

        logs.show_subscription("42");
        assert_eq!(logs.query.filters.subscription_id, Some("42".into()));
        assert_eq!(logs.query.filters.status, None);
        assert_eq!(logs.query.filters.virtual_model_name, None);
        assert_eq!(logs.query.page, 1);
        assert_eq!(logs.selected_id, None);
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, ratatui::crossterm::event::KeyModifiers::NONE)
    }
}
