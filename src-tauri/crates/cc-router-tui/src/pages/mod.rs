//! 页面。每个页面是一个 [`Component`]: 官方 Component 模板的简化版 (spec §4.3)。

use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::Frame;

use crate::action::{Action, Cmd};
use crate::fx::Fx;
use crate::i18n::Strings;
use crate::theme::Theme;
use crate::widgets::keybar::Hint;

pub mod overview;
pub mod placeholder;

/// 画一帧需要的只读环境 + 动效入口。
pub struct DrawCtx<'a> {
    pub theme: &'a Theme,
    pub s: &'static Strings,
    /// Unix 毫秒
    pub now_ms: i64,
    /// 250ms tick 计数, 驱动 throbber。
    pub tick: u64,
    /// 页面在 `draw` 里才知道某一行 / 某个数字落在哪个 `Rect`, 所以「这一行闪一下」只能在这里触发。
    pub fx: &'a mut Fx,
}

pub trait Component {
    /// 页面自己的键位。全局键 (切页 / 帮助 / 退出 / 刷新) 由 `App` 先处理, 到不了这里。
    fn handle_key(&mut self, key: KeyEvent) -> Option<Action>;
    fn update(&mut self, action: &Action) -> Vec<Cmd>;
    /// `&mut self`: 只允许为动效记录几何信息, 不改业务状态 —— 同一状态画两次必须得到同一帧。
    fn draw(&mut self, frame: &mut Frame, area: Rect, ctx: &mut DrawCtx);
    /// 底栏左侧显示的页面键位。
    fn hints(&self, s: &'static Strings) -> Vec<Hint<'static>>;
}
