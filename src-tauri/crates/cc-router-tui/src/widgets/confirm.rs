//! 「是 / 否」确认弹窗。`y`/`Y` 是, `n`/`N`/`Esc`/`⏎` 否 (默认 N), 其余按键被吞掉 (键盘路由在
//! `App::handle_key` 里, 这里只画)。

use ratatui::layout::{Constraint, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};
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
pub fn area(screen: Rect, prompt: &str) -> Rect {
    let width = (prompt.width() as u16 + 8).clamp(MIN_WIDTH, screen.width.saturating_sub(SCREEN_MARGIN));
    screen.centered(Constraint::Length(width), Constraint::Length(HEIGHT))
}

pub fn draw(frame: &mut Frame, area: Rect, state: &ConfirmState, theme: &Theme, s: &Strings) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme.border_style())
        .title_top(format!(" {} ", s.confirm_title))
        .title_bottom(Line::from(format!(" {} ", s.confirm_keys)).right_aligned())
        .padding(Padding::new(2, 2, 1, 1));
    frame.render_widget(Clear, area);
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
}
