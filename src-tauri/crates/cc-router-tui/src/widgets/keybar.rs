//! 底部键位提示栏: 左边是当前页面的键, 右边是全局键。

use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::Frame;

use crate::theme::Theme;

/// (键, 说明)
pub type Hint<'a> = (&'a str, &'a str);

fn line<'a>(hints: &[Hint<'a>], theme: &Theme) -> Line<'a> {
    let mut spans = Vec::with_capacity(hints.len() * 3);
    for (i, (key, desc)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("   "));
        }
        spans.push(Span::styled(*key, theme.accent_bold()));
        spans.push(Span::styled(format!(" {desc}"), theme.muted_style()));
    }
    Line::from(spans)
}

pub fn draw(frame: &mut Frame, area: Rect, left: &[Hint], right: &[Hint], theme: &Theme) {
    let left = line(left, theme);
    let right = line(right, theme);
    let [l, r] = Layout::horizontal([Constraint::Length(left.width() as u16), Constraint::Length(right.width() as u16)])
        .flex(Flex::SpaceBetween)
        .horizontal_margin(1)
        .areas(area);
    frame.render_widget(left, l);
    frame.render_widget(right, r);
}
