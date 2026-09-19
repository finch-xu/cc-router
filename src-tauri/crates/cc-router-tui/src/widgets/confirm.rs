//! 「是 / 否」确认弹窗。`y`/`Y` 是, `n`/`N`/`Esc`/`⏎` 否 (默认 N), 其余按键被吞掉 (键盘路由在
//! `App::handle_key` 里, 这里只画)。

use ratatui::layout::{Constraint, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Padding, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::i18n::Strings;
use crate::popup::ConfirmState;
use crate::theme::Theme;

/// 弹窗宽度下限; 比这个还窄的提示也按这个宽度画, 免得太局促。
const MIN_WIDTH: u16 = 30;
/// 弹窗与屏幕两侧边缘至少留的空隙 (合计, 不是单边)。
const SCREEN_MARGIN: u16 = 4;
const HEIGHT: u16 = 5;

/// 居中, 宽 = 提示显示宽度 + 8, 夹在 `MIN_WIDTH..=screen.width - SCREEN_MARGIN` 之间, 高固定 5。
///
/// `screen.width < MIN_WIDTH + SCREEN_MARGIN` (34) 时, `screen.width - SCREEN_MARGIN` 会小于
/// `MIN_WIDTH`——`clamp(MIN_WIDTH, 那个更小的上界)` 违反 `min <= max` 会直接 panic (Fix round D)。
/// 用 `.max(MIN_WIDTH)` 兜底上界, 保证任何 `Rect` 传进来都不 panic; 主循环本来就不会在小于
/// `app::MIN_WIDTH`(80)/`MIN_HEIGHT`(24) 的终端上调用这个函数 (`App::draw` 的早退分支挡住了),
/// 这里只是让函数本身对任意输入都是全函数 (total function), 不依赖调用方守规矩。
pub fn area(screen: Rect, prompt: &str) -> Rect {
    let upper = screen.width.saturating_sub(SCREEN_MARGIN).max(MIN_WIDTH);
    let width = (prompt.width() as u16 + 8).clamp(MIN_WIDTH, upper);
    screen.centered(Constraint::Length(width), Constraint::Length(HEIGHT))
}

pub fn draw(frame: &mut Frame, area: Rect, state: &ConfirmState, theme: &Theme, s: &Strings) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme.border_style())
        .title_top(format!(" {} ", s.confirm_title))
        .title_bottom(Line::from(format!(" {} ", s.confirm_keys)).right_aligned())
        .padding(Padding::new(2, 2, 1, 1));
    super::clear_popup_area(frame, area);
    frame.render_widget(Paragraph::new(state.prompt.as_str()).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn area_is_centered_and_clamped() {
        let screen = Rect::new(0, 0, 80, 24);

        // 短提示的宽度应该夹到下限 30, 且整体居中——直接用 ratatui 自己的 `centered` 算出预期值,
        // 不手动重算舍入方向 (`Flex::Center` 奇数余量往哪边多分 1 列是 ratatui 的实现细节)。
        let short = area(screen, "短");
        let expect_short = screen.centered(Constraint::Length(MIN_WIDTH), Constraint::Length(HEIGHT));
        assert_eq!(short, expect_short, "短提示应该夹到下限 30 并居中");

        // 超长提示的宽度应该夹到上限 screen.width - SCREEN_MARGIN。
        let long = area(screen, &"x".repeat(200));
        let expect_long = screen.centered(Constraint::Length(screen.width - SCREEN_MARGIN), Constraint::Length(HEIGHT));
        assert_eq!(long, expect_long, "超长提示应该夹到上限 screen-4 并居中");

        // 中等长度的提示 (30 + 8 = 38) 落在 30..=76 区间内, 应该正好等于「宽度+8」, 不被夹到任一端。
        let mid = area(screen, &"x".repeat(30));
        assert_eq!(mid.width, 38, "没有触顶或触底时, 宽度应该正好是提示宽度 + 8");
    }

    /// Fix round D: `screen.width < MIN_WIDTH + SCREEN_MARGIN` (34) 时旧版会在 `clamp` 里 panic
    /// (下界 30 > 上界 `screen.width - 4`)。20 列宽的屏幕远小于这个阈值, 任何提示长度都不该
    /// panic——内部算出来的目标宽度会被 `.max(MIN_WIDTH)` 兜到 30, 但 `Rect::centered` 用的
    /// `Layout` 约束求解器会把它进一步夹到父矩形自己的宽度以内, 结果不会比屏幕本身更宽。
    #[test]
    fn area_does_not_panic_on_a_tiny_screen() {
        let tiny = Rect::new(0, 0, 20, 10);
        let short = area(tiny, "短");
        assert!(short.width <= tiny.width, "不该比屏幕本身更宽, 实际 {}", short.width);
        let long = area(tiny, &"x".repeat(200));
        assert!(long.width <= tiny.width, "超长提示也不该比屏幕本身更宽, 实际 {}", long.width);
    }
}
