//! `?` 弹出的键位表。

use ratatui::layout::{Constraint, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};
use ratatui::Frame;

use crate::format::fit;
use crate::i18n::Strings;
use crate::theme::Theme;

const KEY_COL: usize = 18;
const WIDTH: u16 = 48;

pub fn area(screen: Rect, s: &Strings) -> Rect {
    // 行数 + 上下边框 + 上下内距
    let height = s.help_rows.len() as u16 + 4;
    screen.centered(Constraint::Length(WIDTH), Constraint::Length(height))
}

pub fn draw(frame: &mut Frame, area: Rect, theme: &Theme, s: &Strings) {
    let lines: Vec<Line> = s
        .help_rows
        .iter()
        .map(|(key, desc)| Line::from(vec![Span::styled(fit(key, KEY_COL), theme.accent_bold()), Span::raw(*desc)]))
        .collect();
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme.border_style())
        .title_top(format!(" {} ", s.help_title))
        .title_bottom(Line::from(format!(" Esc {} ", s.key_close)).right_aligned())
        .padding(Padding::new(2, 2, 1, 1));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}
