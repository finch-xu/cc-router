//! 过滤选择弹窗: 顶部一行 `tui-input` 输入框, 下面是按空白拆词、每个词都要匹配 `label`/`id` 的
//! 过滤列表。打开时除 `Ctrl+C` 外所有按键归它 (路由在 `App::handle_key`, 这里只处理弹窗自己的键)。
//!
//! 与 `Popup::Confirm` 不同: 输入框 / 选中下标是弹窗自己的可变状态, 打字 / 移动选中不经过
//! `Action` 往返——`handle_key` 的签名 (`&mut self`) 就是照这个设计写的, 只有「选定一行」
//! (⏎) 与「取消」(Esc) 两件事才产出 `Action`, 这与订阅页 `Component::handle_key` 里方向键直接
//! 改自身 `selected_id` 而不经 `Action` 是同一套约定。

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, List, ListItem, ListState, Padding, Paragraph};
use ratatui::Frame;
use tui_input::backend::crossterm::EventHandler;
use tui_input::Input;

use crate::action::Action;
pub use crate::client::dto::Slot;
use crate::i18n::Strings;
use crate::theme::Theme;

/// `PageUp` / `PageDown` 在第一帧画出来之前没有真实的可视行数可用, 先给个不至于原地不动的默认值
/// (仿 `pages::subscriptions::DEFAULT_PAGE_ROWS` 同款写法, Fix round G)。
const DEFAULT_LIST_ROWS: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerItem {
    pub id: String,
    pub label: String,
    pub hint: Option<String>,
}

/// 弹窗是给谁开的; 结果原样带回, 页面据此知道该把值填到哪 (Task 5/6 起消费)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerTag {
    SlotModel { slot: Slot },
    SlotEffort { slot: Slot },
    VmAddSubscription,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerSpec {
    pub tag: PickerTag,
    pub title: String,
    pub items: Vec<PickerItem>,
    pub allow_custom: bool,
    /// 输入框初值。**不参与首次过滤**——打开时显示全部, 只用来定位初始选中行 (`id == initial`
    /// 的那一项, 没有就第一行); 用户一旦编辑过输入框, 之后就按当前输入过滤 (见 [`PickerState::dirty`])。
    pub initial: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerChoice {
    Item(String),
    Custom(String),
}

/// 过滤后的一行: 置顶的「使用输入的文本」行 (`allow_custom` 时) 或者一个 [`PickerItem`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerRow {
    UseTyped(String),
    Item(PickerItem),
}

#[derive(Debug, Clone)]
pub struct PickerState {
    spec: PickerSpec,
    input: Input,
    /// 输入框的值是否被编辑过 (哪怕改回和 `initial`一样的文本也算)。控制 `initial` 要不要参与
    /// 过滤: 见 [`PickerSpec::initial`] 与 [`PickerState::visible`]。
    dirty: bool,
    selected: usize,
    list_state: ListState,
    /// 上一帧列表区域的可视行数, `PageUp`/`PageDown` 按这个翻页; 第一帧画出来之前用
    /// `DEFAULT_LIST_ROWS` 兜底 (Fix round G, 不再是猜的固定步长)。
    last_list_rows: usize,
}

// `tui_input::Input` 不实现 `PartialEq` (它的内部还有 yank 缓冲等实现细节, 不适合参与相等比较),
// 所以不能整体 `#[derive(PartialEq)]`——按「逻辑上是同一个状态」手写: 规格、输入框的文本与光标
// 位置、是否已编辑、选中下标。`list_state` (滚动偏移缓存) 与 `last_list_rows` (上一帧量出来的
// 几何) 都不参与相等判断, 跟 `Fx` / `TableState` 同一条道理: 不是业务状态。
impl PartialEq for PickerState {
    fn eq(&self, other: &Self) -> bool {
        self.spec == other.spec
            && self.input.value() == other.input.value()
            && self.input.cursor() == other.input.cursor()
            && self.dirty == other.dirty
            && self.selected == other.selected
    }
}

impl PickerState {
    pub fn new(spec: PickerSpec) -> Self {
        let input = Input::new(spec.initial.clone());
        let mut state =
            Self { spec, input, dirty: false, selected: 0, list_state: ListState::default(), last_list_rows: DEFAULT_LIST_ROWS };
        let rows = state.visible();
        state.selected = rows
            .iter()
            .position(|row| matches!(row, PickerRow::Item(item) if item.id == state.spec.initial))
            .unwrap_or(0);
        state
    }

    /// 当前过滤后的可见行 (含置顶的「使用输入的文本」行)。纯函数, 供测试与 `draw` 共用。
    ///
    /// 过滤规则: 输入按空白拆成多个词 (大小写不敏感), 每个词都要是某一项 `label` 或 `id` 的子串
    /// 才算命中; 还没编辑过输入框时忽略当前文本, 按空查询处理 (= 显示全部), 对应
    /// [`PickerSpec::initial`] 「不参与首次过滤」的约定。
    ///
    /// 「使用输入的文本」这一行由 `allow_custom` 单独控制, 与是否编辑过无关——哪怕 `initial`
    /// 本身就是一段自定义文本 (不对应任何 item), 打开弹窗时也该看得到「使用「xxx」」这一行。
    pub fn visible(&self) -> Vec<PickerRow> {
        let query = if self.dirty { self.input.value() } else { "" };
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();

        let mut rows = Vec::new();
        let typed = self.input.value().trim();
        if self.spec.allow_custom && !typed.is_empty() {
            let exact_id_match = self.spec.items.iter().any(|item| item.id == typed);
            if !exact_id_match {
                rows.push(PickerRow::UseTyped(typed.to_string()));
            }
        }

        rows.extend(self.spec.items.iter().filter(|item| Self::item_matches(item, &words)).cloned().map(PickerRow::Item));
        rows
    }

    fn item_matches(item: &PickerItem, words: &[String]) -> bool {
        if words.is_empty() {
            return true;
        }
        let label = item.label.to_lowercase();
        let id = item.id.to_lowercase();
        words.iter().all(|w| label.contains(w.as_str()) || id.contains(w.as_str()))
    }

    /// ⏎ → [`Action::PickerDone`]; Esc → [`Action::ClosePopup`]; 其余 (方向键 / 打字) 改自身状态,
    /// 不产出 `Action`。
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Up => {
                self.move_selection(-1);
                None
            }
            KeyCode::Down => {
                self.move_selection(1);
                None
            }
            KeyCode::Char('p') if ctrl => {
                self.move_selection(-1);
                None
            }
            KeyCode::Char('n') if ctrl => {
                self.move_selection(1);
                None
            }
            KeyCode::PageUp => {
                self.move_selection(-(self.last_list_rows.max(1) as isize));
                None
            }
            KeyCode::PageDown => {
                self.move_selection(self.last_list_rows.max(1) as isize);
                None
            }
            // 列表导航的 Home/End, 不是输入框光标的 Home/End (那个交给 tui-input 会跳去 `_` 分支,
            // 这里必须先接住)。
            KeyCode::Home => {
                self.selected = 0;
                None
            }
            KeyCode::End => {
                self.select_last();
                None
            }
            KeyCode::Enter => self.enter(),
            KeyCode::Esc => Some(Action::ClosePopup),
            _ => {
                if self.input.handle_event(&Event::Key(key)).is_some_and(|changed| changed.value) {
                    self.dirty = true;
                    // 过滤结果变了: 选中下标回到第一行, 而不是停在旧下标上 (可能已经指向别的项目
                    // 甚至越界)。
                    self.selected = 0;
                }
                None
            }
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let len = self.visible().len();
        if len == 0 {
            self.selected = 0;
            return;
        }
        let next = (self.selected as isize + delta).clamp(0, len as isize - 1);
        self.selected = next as usize;
    }

    fn select_last(&mut self) {
        let len = self.visible().len();
        self.selected = len.saturating_sub(1);
    }

    fn enter(&mut self) -> Option<Action> {
        let rows = self.visible();
        let row = rows.get(self.selected)?;
        let choice = match row {
            PickerRow::UseTyped(text) => PickerChoice::Custom(text.clone()),
            PickerRow::Item(item) => PickerChoice::Item(item.id.clone()),
        };
        Some(Action::PickerDone { tag: self.spec.tag.clone(), choice })
    }
}

/// 居中; 宽 `min(60, screen-4)`, 高 `min(16, screen-4)`。
pub fn area(screen: Rect) -> Rect {
    const WIDTH_MAX: u16 = 60;
    const HEIGHT_MAX: u16 = 16;
    const SCREEN_MARGIN: u16 = 4;
    let width = WIDTH_MAX.min(screen.width.saturating_sub(SCREEN_MARGIN));
    let height = HEIGHT_MAX.min(screen.height.saturating_sub(SCREEN_MARGIN));
    screen.centered(Constraint::Length(width), Constraint::Length(height))
}

pub fn draw(frame: &mut Frame, area: Rect, state: &mut PickerState, theme: &Theme, s: &Strings) {
    let rows = state.visible();
    let total = rows.len();
    let current = if total == 0 { 0 } else { state.selected + 1 };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme.border_style())
        .title_top(format!(" {} ", state.spec.title))
        .title_bottom(Line::from(format!(" {} ", s.picker_keys)).style(theme.muted_style()))
        .title_bottom(Line::from(format!(" {current}/{total} ")).right_aligned().style(theme.muted_style()))
        .padding(Padding::new(1, 1, 1, 1));
    let inner = block.inner(area);
    // Fix round A: `Clear` 本身修不好紧贴弹窗边缘、横跨边界的宽字符, 必须在弹窗画任何内容之前
    // (含 `Clear` 自己) 先跑一遍 `clear_popup_area` 里的修复——它内部才会真的调 `Clear`。
    crate::widgets::clear_popup_area(frame, area);
    frame.render_widget(block, area);

    let [input_area, list_area] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    // Fix round G: 记录这一帧真实画出来的可视行数, 供 `PageUp`/`PageDown` 下次按键时使用——
    // 只是记几何, 不改业务状态, 符合「同一状态画两次得到同一帧」的约束 (仿
    // `pages::subscriptions::Subscriptions::last_page_rows` 同款写法)。
    state.last_list_rows = list_area.height.max(1) as usize;

    // Fix round F: 留一列给光标——用输入框可视宽度减 1 去算 `visual_scroll`, 否则文本正好填满
    // 输入框时光标会画在最后一个字符上面 (盖住它), 而不是紧跟在它后面的空位。
    let visual_width = input_area.width.max(1).saturating_sub(1) as usize;
    let scroll = state.input.visual_scroll(visual_width);
    frame.render_widget(Paragraph::new(state.input.value()).scroll((0, scroll as u16)), input_area);
    let cursor_x = input_area.x + state.input.visual_cursor().saturating_sub(scroll) as u16;
    frame.set_cursor_position((cursor_x.min(input_area.right().saturating_sub(1)), input_area.y));

    if rows.is_empty() {
        frame.render_widget(Line::raw(s.picker_empty).centered().style(theme.muted_style()), list_area);
        return;
    }

    let items: Vec<ListItem> = rows.iter().map(|row| row_item(row, s, theme)).collect();
    let list = List::new(items).highlight_symbol("▌ ").highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    state.list_state.select(Some(state.selected));
    frame.render_stateful_widget(list, list_area, &mut state.list_state);
}

fn row_item(row: &PickerRow, s: &Strings, theme: &Theme) -> ListItem<'static> {
    match row {
        PickerRow::UseTyped(text) => ListItem::new((s.picker_use_typed)(text)),
        PickerRow::Item(item) => {
            let mut spans = vec![Span::raw(item.label.clone())];
            if let Some(hint) = &item.hint {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(hint.clone(), theme.muted_style()));
            }
            ListItem::new(Line::from(spans))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ColorMode;

    fn item(id: &str, label: &str) -> PickerItem {
        PickerItem { id: id.into(), label: label.into(), hint: None }
    }

    fn spec(items: Vec<PickerItem>, allow_custom: bool, initial: &str) -> PickerSpec {
        PickerSpec { tag: PickerTag::VmAddSubscription, title: "选择".into(), items, allow_custom, initial: initial.into() }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn type_str(state: &mut PickerState, text: &str) {
        for c in text.chars() {
            state.handle_key(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn empty_input_shows_everything() {
        let items = vec![item("a", "Alpha"), item("b", "Beta"), item("c", "Gamma")];
        let state = PickerState::new(spec(items.clone(), false, ""));
        assert_eq!(state.visible(), items.into_iter().map(PickerRow::Item).collect::<Vec<_>>());
    }

    #[test]
    fn every_word_must_match_label_or_id_case_insensitively() {
        let items = vec![item("glm-4.6", "GLM 4.6 主力"), item("glm-4.5-air", "GLM 4.5 Air"), item("gpt-4o", "GPT-4o")];
        let mut state = PickerState::new(spec(items, false, ""));
        type_str(&mut state, "GLM AIR");
        assert_eq!(state.visible(), vec![PickerRow::Item(item("glm-4.5-air", "GLM 4.5 Air"))]);
    }

    #[test]
    fn custom_row_appears_only_when_allowed_nonblank_and_not_an_exact_id() {
        let items = vec![item("a", "Alpha")];

        let mut disallowed = PickerState::new(spec(items.clone(), false, ""));
        type_str(&mut disallowed, "zzz");
        assert!(!disallowed.visible().iter().any(|r| matches!(r, PickerRow::UseTyped(_))), "allow_custom=false 永不出现");

        let mut blank = PickerState::new(spec(items.clone(), true, ""));
        type_str(&mut blank, "   ");
        assert!(!blank.visible().iter().any(|r| matches!(r, PickerRow::UseTyped(_))), "纯空白输入不出现\n{:?}", blank.visible());

        let mut exact = PickerState::new(spec(items.clone(), true, ""));
        type_str(&mut exact, "a");
        assert!(!exact.visible().iter().any(|r| matches!(r, PickerRow::UseTyped(_))), "精确匹配 id 时不出现");

        let mut custom = PickerState::new(spec(items.clone(), true, ""));
        type_str(&mut custom, "zzz");
        assert_eq!(custom.visible().first(), Some(&PickerRow::UseTyped("zzz".into())), "非空且不精确匹配时应该置顶");

        // Fix round H: initial 非空的两种边界情况——同样的判定规则, 但这次是打开弹窗那一刻就该
        // 生效, 不需要用户先敲一个字符。
        let initial_matches = PickerState::new(spec(items.clone(), true, "a"));
        assert!(
            !initial_matches.visible().iter().any(|r| matches!(r, PickerRow::UseTyped(_))),
            "initial 精确等于某个 item 的 id 时, 打开就不该有自定义行"
        );
        let initial_custom = PickerState::new(spec(items, true, "zzz"));
        assert_eq!(
            initial_custom.visible().first(),
            Some(&PickerRow::UseTyped("zzz".into())),
            "initial 不是任何 item 的 id 时, 打开就该看到自定义行, 不用等用户先编辑"
        );
    }

    #[test]
    fn enter_on_the_custom_row_yields_custom_trimmed() {
        let items = vec![item("a", "Alpha")];
        let mut state = PickerState::new(spec(items, true, ""));
        type_str(&mut state, "  zzz  ");
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            Some(Action::PickerDone { tag: PickerTag::VmAddSubscription, choice: PickerChoice::Custom("zzz".into()) })
        );
    }

    #[test]
    fn enter_with_no_rows_does_nothing() {
        let items = vec![item("a", "Alpha")];
        let mut state = PickerState::new(spec(items, false, ""));
        type_str(&mut state, "zzz");
        assert!(state.visible().is_empty());
        assert_eq!(state.handle_key(key(KeyCode::Enter)), None);
    }

    #[test]
    fn initial_selects_but_does_not_filter() {
        let items = vec![item("a", "Alpha"), item("b", "Beta"), item("c", "Gamma")];
        let state = PickerState::new(spec(items.clone(), false, "b"));
        let rows = state.visible();
        assert_eq!(rows.len(), items.len(), "打开时不该按 initial 过滤\n{rows:?}");
        let expected = rows.iter().position(|r| matches!(r, PickerRow::Item(it) if it.id == "b")).unwrap();
        assert_eq!(state.selected, expected, "选中行应该定位到 id == initial 的那一项");
    }

    #[test]
    fn selection_resets_and_clamps_when_the_filter_changes() {
        let items = vec![item("a", "Alpha"), item("b", "Beta"), item("c", "Gamma")];
        let mut state = PickerState::new(spec(items, false, ""));
        state.handle_key(key(KeyCode::Down));
        assert_eq!(state.selected, 1);

        type_str(&mut state, "g");
        assert_eq!(state.visible(), vec![PickerRow::Item(item("c", "Gamma"))]);
        assert_eq!(state.selected, 0, "过滤结果变化后应该回到第一行");
    }

    #[test]
    fn navigation_keys_clamp() {
        let items: Vec<PickerItem> = (0..20).map(|i| item(&i.to_string(), &format!("Item {i}"))).collect();
        let mut state = PickerState::new(spec(items, false, ""));

        state.handle_key(key(KeyCode::Up));
        assert_eq!(state.selected, 0, "到顶不该绕回");

        state.handle_key(ctrl_key(KeyCode::Char('n')));
        assert_eq!(state.selected, 1, "Ctrl+N 等同 Down");
        state.handle_key(ctrl_key(KeyCode::Char('p')));
        assert_eq!(state.selected, 0, "Ctrl+P 等同 Up");

        state.handle_key(key(KeyCode::PageDown));
        assert!(state.selected > 0 && state.selected < 19, "PageDown 应该往下翻一段, 实际 {}", state.selected);

        state.handle_key(key(KeyCode::End));
        assert_eq!(state.selected, 19);
        state.handle_key(key(KeyCode::Down));
        assert_eq!(state.selected, 19, "到底不该越界");

        state.handle_key(key(KeyCode::Home));
        assert_eq!(state.selected, 0);
        state.handle_key(key(KeyCode::PageUp));
        assert_eq!(state.selected, 0, "到顶再 PageUp 不该越界");
    }

    #[test]
    fn esc_cancels() {
        let items = vec![item("a", "Alpha")];
        let mut state = PickerState::new(spec(items, false, ""));
        assert_eq!(state.handle_key(key(KeyCode::Esc)), Some(Action::ClosePopup));
    }

    #[test]
    fn ctrl_u_clears() {
        let items = vec![item("glm-4.6", "GLM"), item("gpt-4o", "GPT")];
        let mut state = PickerState::new(spec(items, false, ""));
        type_str(&mut state, "glm");
        assert_eq!(state.visible().len(), 1);

        state.handle_key(ctrl_key(KeyCode::Char('u')));
        assert_eq!(state.input.value(), "");
        assert_eq!(state.visible().len(), 2, "清空后应该显示全部");
    }

    #[test]
    fn cjk_input_is_accepted() {
        let items = vec![item("zhipu", "智谱主号"), item("kimi", "Kimi 备用")];
        let mut state = PickerState::new(spec(items, false, ""));
        type_str(&mut state, "智谱");
        assert_eq!(state.input.value(), "智谱");
        assert_eq!(state.visible(), vec![PickerRow::Item(item("zhipu", "智谱主号"))]);
    }

    #[test]
    fn area_is_centered_and_clamped() {
        let screen = Rect::new(0, 0, 80, 24);
        let a = area(screen);
        assert_eq!((a.width, a.height), (60, 16));

        let tiny = area(Rect::new(0, 0, 40, 20));
        assert_eq!((tiny.width, tiny.height), (36, 16), "宽度应该夹到 screen-4");
    }

    /// Fix round D: `area()` 用的是 `.min()` 不是 `.clamp()`, 天生不会因为屏幕比 `SCREEN_MARGIN`
    /// 还小而 panic (`saturating_sub` 兜底), 但补一条回归测试锁住这个事实——万一以后有人手滑把
    /// `.min()` 改成 `.clamp(下限, ...)`, 这里会立刻炸。
    #[test]
    fn area_does_not_panic_on_a_tiny_screen() {
        let a = area(Rect::new(0, 0, 2, 2));
        assert_eq!((a.width, a.height), (0, 0));
    }

    /// 冒烟: 一次完整渲染不 panic (弹窗的画法细节由 `tests/ui.rs` 的快照覆盖)。
    #[test]
    fn draw_does_not_panic() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let items = vec![item("a", "Alpha"), item("b", "Beta")];
        let mut state = PickerState::new(spec(items, true, ""));
        type_str(&mut state, "a");
        let theme = Theme::new(ColorMode::TrueColor);
        let s = &crate::i18n::ZH;
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| {
                let a = area(frame.area());
                draw(frame, a, &mut state, &theme, s);
            })
            .unwrap();
    }

    /// Fix round F: 文本正好填满输入框可视宽度时, 光标应该落在最后一个字符之后的空位 (一个空格),
    /// 不能盖在字符本身上面。用一个好辨认的收尾字符 'Z', 直接检查光标那一格画出来的符号是不是空格
    /// ——只看光标坐标本身在修复前后可能是同一个数字 (被 clamp 到同一列), 咬不住这个回归。
    #[test]
    fn cursor_reserves_a_column_when_text_exactly_fills_the_box() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let items = vec![item("a", "Alpha")];
        let mut state = PickerState::new(spec(items, false, ""));
        // 60 列弹窗 - 2 边框 - 2 padding = 56 列输入框; 填 56 个字符正好撑满"旧版不预留光标列"的
        // 宽度, 最后一个字符用 'Z' 收尾, 方便识别它有没有被光标盖住。
        type_str(&mut state, &"a".repeat(55));
        state.handle_key(key(KeyCode::Char('Z')));

        let theme = Theme::new(ColorMode::TrueColor);
        let s = &crate::i18n::ZH;
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let popup = area(Rect::new(0, 0, 80, 24));
        terminal.draw(|frame| draw(frame, popup, &mut state, &theme, s)).unwrap();

        let cursor = terminal.backend().cursor_position();
        let at_cursor = terminal.backend().buffer()[(cursor.x, cursor.y)].symbol().to_string();
        assert_eq!(at_cursor, " ", "光标应该落在 'Z' 之后的空位, 不是盖在 'Z' 上面 (实际那一格是 {at_cursor:?})");
    }

    /// Fix round G: 画过一帧之后, `PageUp`/`PageDown` 应该按真实量出来的可视行数翻页, 不再是
    /// 猜的固定步长。
    #[test]
    fn page_step_follows_the_last_drawn_list_height() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let items: Vec<PickerItem> = (0..30).map(|i| item(&i.to_string(), &format!("Item {i}"))).collect();
        let mut state = PickerState::new(spec(items, false, ""));
        assert_eq!(state.last_list_rows, DEFAULT_LIST_ROWS, "画第一帧之前应该是默认兜底值");

        let theme = Theme::new(ColorMode::TrueColor);
        let s = &crate::i18n::ZH;
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let popup = area(Rect::new(0, 0, 80, 24));
        terminal.draw(|frame| draw(frame, popup, &mut state, &theme, s)).unwrap();

        let real_rows = state.last_list_rows;
        assert_ne!(real_rows, 0, "画过一帧之后应该是真实的可视行数");

        state.handle_key(key(KeyCode::PageDown));
        assert_eq!(state.selected, real_rows.min(29), "PageDown 应该按真实画出来的可视行数翻页, 不是固定步长");
    }
}
