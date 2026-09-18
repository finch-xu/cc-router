//! 右上角的短提示。一次只显示一条, 后来的排队。

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::format::fit;
use crate::theme::Theme;

/// 从出现到消失的总时长; 最后 [`crate::fx::ms::TOAST_OUT`] 毫秒在播消散效果。
pub const LIFETIME_MS: i64 = 3000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub kind: ToastKind,
    pub text: String,
    /// 第一次被画出来的时刻 (Unix 毫秒); 还在排队时为 None。
    pub shown_at: Option<i64>,
    pub fading: bool,
}

impl Toast {
    pub fn new(kind: ToastKind, text: impl Into<String>) -> Self {
        Self { kind, text: text.into(), shown_at: None, fading: false }
    }

    pub fn expired(&self, now_ms: i64) -> bool {
        self.shown_at.is_some_and(|t| now_ms - t >= LIFETIME_MS)
    }
}

/// 贴着右边、紧挨标签栏下方。
pub fn area(screen: Rect, text: &str) -> Rect {
    let max = screen.width.saturating_sub(4);
    let width = (text.width() as u16 + 4).min(max);
    Rect::new(screen.right().saturating_sub(width + 1), screen.y + 3, width, 3).intersection(screen)
}

pub fn draw(frame: &mut Frame, area: Rect, toast: &Toast, theme: &Theme) {
    let color = match toast.kind {
        ToastKind::Info => theme.accent,
        ToastKind::Success => theme.ok,
        ToastKind::Error => theme.err,
    };
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(color));
    let inner_width = area.width.saturating_sub(4) as usize;
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(format!(" {} ", fit(&toast.text, inner_width))).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_toast_does_not_age() {
        let mut t = Toast::new(ToastKind::Info, "x");
        assert!(!t.expired(i64::MAX));
        t.shown_at = Some(1_000);
        assert!(!t.expired(1_000 + LIFETIME_MS - 1));
        assert!(t.expired(1_000 + LIFETIME_MS));
    }

    #[test]
    fn area_hugs_the_right_edge_and_never_leaves_the_screen() {
        let screen = Rect::new(0, 0, 80, 24);
        let a = area(screen, "已重新连接"); // 10 列
        assert_eq!((a.width, a.height, a.right(), a.y), (14, 3, 79, 3));
        let long = "长".repeat(100);
        let a = area(screen, &long);
        assert_eq!(a.width, 76);
        assert!(screen.contains(ratatui::layout::Position::new(a.x, a.y)));
    }
}
