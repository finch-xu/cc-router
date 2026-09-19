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

/// 两组 hint 之间至少留出的空档 (哪怕刚好放得下也不让它们贴在一起, 否则「m 模型」和
/// 「? 帮助」会连成一个词, 看不出是两条独立的提示)。
const MIN_GAP: u16 = 1;

/// 放不下时页面键位 (`left`) 从右往左 (最后加进去的先丢) 依次丢弃, 直到跟全局键位 (`right`) 一起
/// 放得下为止; `right` 永远不裁——`?` 帮助 / `q` 退出必须一直在。`available` 是留给两条 hint 行
/// 的总宽度 (已经扣掉 `horizontal_margin(1)` 两侧各 1 列 + [`MIN_GAP`])。
fn fit_left_count(left: &[Hint], right_width: u16, available: u16, theme: &Theme) -> usize {
    let mut n = left.len();
    while n > 0 && line(&left[..n], theme).width() as u16 + right_width > available {
        n -= 1;
    }
    n
}

pub fn draw(frame: &mut Frame, area: Rect, left: &[Hint], right: &[Hint], theme: &Theme) {
    let right = line(right, theme);
    let available = area.width.saturating_sub(2 + MIN_GAP);
    let n = fit_left_count(left, right.width() as u16, available, theme);
    let left = line(&left[..n], theme);
    let [l, r] = Layout::horizontal([Constraint::Length(left.width() as u16), Constraint::Length(right.width() as u16)])
        .flex(Flex::SpaceBetween)
        .horizontal_margin(1)
        .areas(area);
    frame.render_widget(left, l);
    frame.render_widget(right, r);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ColorMode, Theme};

    /// 放不下时应该从右往左 (最后加进去的先) 丢页面键, 全局键永远保留。
    #[test]
    fn left_hints_are_dropped_from_the_right_when_they_do_not_fit() {
        let theme = Theme::new(ColorMode::TrueColor);
        let left: Vec<Hint> = vec![("1-5", "切页"), ("↑↓", "选择"), ("e", "启停"), ("t", "测试"), ("m", "模型"), ("b", "余额")];
        let right: Vec<Hint> = vec![("?", "帮助"), ("q", "退出")];

        // 全部放得下: 一个不丢。
        let full = fit_left_count(&left, line(&right, &theme).width() as u16, 200, &theme);
        assert_eq!(full, left.len());

        // 只够放前两个页面键位 + 全局键位。
        let right_width = line(&right, &theme).width() as u16;
        let available = line(&left[..2], &theme).width() as u16 + right_width;
        let n = fit_left_count(&left, right_width, available, &theme);
        assert_eq!(n, 2, "应该只保留最靠左的两个页面键位");

        // 极窄: 一个页面键位都放不下时, 全局键位仍然要保留 (调用方 `draw` 不裁 `right`)。
        let n_tiny = fit_left_count(&left, right_width, right_width, &theme);
        assert_eq!(n_tiny, 0);
    }
}
