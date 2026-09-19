//! 跨页面复用的小部件。

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::widgets::Clear;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

pub mod badge;
pub mod confirm;
pub mod gauge;
pub mod help;
pub mod keybar;
pub mod picker;
pub mod toast;

/// 第 `tick` 帧的 spinner 状态。`calc_step(0)` 在 throbber-widgets-tui 里的含义是「随机取一格」,
/// 所以步长永远不传 0 —— 否则同一状态画两次会得到不同的帧。
pub fn spinner_state(tick: u64) -> throbber_widgets_tui::ThrobberState {
    let mut state = throbber_widgets_tui::ThrobberState::default();
    state.calc_step((tick % 120) as i8 + 1);
    state
}

/// 三个弹窗 (Help / Confirm / Picker) 打开时都要用这个替代直接 `frame.render_widget(Clear, area)`
/// ——见 [`repair_wide_glyphs_at_edges`] 的文档: 光是 `Clear` 修不好紧贴弹窗左右边缘、横跨边界的
/// 宽字符 (CJK 等 2 列宽字符) (Fix round A)。
pub fn clear_popup_area(frame: &mut Frame, area: Rect) {
    repair_wide_glyphs_at_edges(frame.buffer_mut(), area);
    frame.render_widget(Clear, area);
}

/// ratatui 的 `BufferDiff` (画完一帧后, 比较上一帧和这一帧的缓冲区, 算出要真正发给终端的那部分)
/// 有一条通用假设: 宽字符 (`symbol().width() > 1`) 右边紧跟着的那一列永远是它自己的续格
/// (`Buffer::set_stringn` 写宽字符时确实会显式把续格清空), 所以直接跳过、不单独比较/发送——
/// **不管那一列这一帧到底变没变**。
///
/// 弹窗打开时, 底下页面的宽字符如果横跨弹窗左边缘 (本体落在 `area.x - 1`, 续格恰好是弹窗自己的
/// 第一列 `area.x`), 弹窗接下来在那一列画的边框字符就会被这条假设连带跳过——真实终端 (以及
/// `TestBackend`, 它自己的 `buffer()` 同样只应用 diff 里出现的格子, 不是整帧覆盖) 上永远看不到
/// 这个边框。这正是 `ui__picker_popup_80x24.snap` 一开始 "Kimi 备" 那一行缺左边框的成因。
///
/// 修法: 把横跨边界的宽字符本体重置成空格——它在这一帧"变了" (不再是宽字符), diff 就不会再
/// 无条件跳过它右边那一列, 弹窗随后画的内容才能正常被发送。
///
/// 右边缘对称处理: 宽字符本体如果贴着弹窗最后一列 (`area.right() - 1`, 弹窗马上要把它整个
/// 覆盖掉), 它的续格落在 `area.right()` (弹窗外, 弹窗自己的内容不会碰它)。本体被弹窗覆盖之后,
/// 这个续格就成了没有主人的孤儿格, 一并清空, 不留半个字符的残影。
///
/// 必须在弹窗画任何内容 **之前** 调用 (所以 [`clear_popup_area`] 把它排在 `Clear` 前面) ——这里
/// 检查的是底下页面还没被覆盖时的原始状态。
fn repair_wide_glyphs_at_edges(buf: &mut Buffer, area: Rect) {
    let screen = buf.area;
    for y in area.top()..area.bottom() {
        if area.left() > screen.left() {
            reset_if_wide(buf, area.left() - 1, y);
        }
        // 这里不能复用 `reset_if_wide` (它会先检查*目标格自己*是不是宽字符再决定要不要重置)——
        // 续格本身天然就是窄的 (`set_stringn` 写宽字符时就是这么初始化它的), 判断依据是它左边
        // 那个宽字符本体是否横跨了边界, 不是它自己的宽度; 找到了就无条件清空目标格, 不管目标格
        // 当下放的是什么。
        if area.right() < screen.right() && area.right() > 0 && cell_is_wide(buf, area.right() - 1, y) {
            buf[(area.right(), y)].reset();
        }
    }
}

fn cell_is_wide(buf: &Buffer, x: u16, y: u16) -> bool {
    buf.area.contains(Position::new(x, y)) && buf[(x, y)].symbol().width() > 1
}

fn reset_if_wide(buf: &mut Buffer, x: u16, y: u16) {
    if cell_is_wide(buf, x, y) {
        buf[(x, y)].reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    #[test]
    fn wide_glyphs_crossing_either_popup_edge_are_reset() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        // A×4(0-3) 备(4-5) B×7(6-12) 用(13-14) A×2(15-16) — 备横跨左边缘, 用横跨右边缘。
        buf.set_string(0, 0, "AAAA备BBBBBBB用AA", Style::default());
        assert_eq!(buf[(4, 0)].symbol(), "备", "测试串本身要保证宽字符落在预期列");
        assert_eq!(buf[(13, 0)].symbol(), "用");

        // 手动在右边缘续格塞一个脏值, 证明函数确实会强制清空它, 不是巧合地本来就是空格。
        buf[(14, 0)].set_symbol("X");

        let popup = Rect::new(5, 0, 9, 1); // x=5 (紧跟"备"), right=14 (紧贴"用"前)
        repair_wide_glyphs_at_edges(&mut buf, popup);

        assert_eq!(buf[(4, 0)].symbol(), " ", "跨左边缘的宽字符本体应该被重置成空格");
        assert_eq!(buf[(14, 0)].symbol(), " ", "跨右边缘的宽字符续格 (弹窗外) 应该被重置");
        assert_eq!(buf[(0, 0)].symbol(), "A", "不相关的列不受影响");
        assert_eq!(buf[(13, 0)].symbol(), "用", "宽字符本体贴着弹窗最后一列, 弹窗自己的内容会覆盖它, 这个函数不用动它");
    }

    #[test]
    fn narrow_glyphs_at_the_edges_are_left_alone() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        buf.set_string(0, 0, "abcdefghijklmnopqrst", Style::default());
        let before: Vec<String> = (0..20).map(|x| buf[(x, 0)].symbol().to_string()).collect();

        repair_wide_glyphs_at_edges(&mut buf, Rect::new(5, 0, 9, 1));

        let after: Vec<String> = (0..20).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert_eq!(before, after, "没有宽字符跨边界时不该动任何格子");
    }

    /// 弹窗贴着屏幕边缘 (左边缘 `area.x == 0` 或右边缘 `area.right() == screen.width`) 时,
    /// `area.left() - 1` / `area.right()` 都不该越界或 panic。
    #[test]
    fn popup_flush_with_screen_edges_does_not_panic() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 2));
        buf.set_string(0, 0, "AAAAAAAAAA", Style::default());
        buf.set_string(0, 1, "AAAAAAAAAA", Style::default());
        repair_wide_glyphs_at_edges(&mut buf, Rect::new(0, 0, 10, 1)); // 弹窗占满整行宽度
        repair_wide_glyphs_at_edges(&mut buf, Rect::new(0, 1, 5, 1)); // 只贴左边缘
    }
}
