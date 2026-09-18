//! 还没做的页面 (订阅 / 虚拟模型 / 实时路由 / 日志) 的占位, 后续阶段逐个替换掉。

use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::{Constraint, Rect};
use ratatui::text::Line;
use ratatui::Frame;

use super::{Component, DrawCtx};
use crate::action::{Action, Cmd};
use crate::i18n::Strings;
use crate::widgets::keybar::Hint;

#[derive(Debug, Default)]
pub struct Placeholder;

impl Component for Placeholder {
    fn handle_key(&mut self, _key: KeyEvent) -> Option<Action> {
        None
    }

    fn update(&mut self, _action: &Action) -> Vec<Cmd> {
        Vec::new()
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, ctx: &mut DrawCtx) {
        let line = Line::styled(ctx.s.coming_soon, ctx.theme.muted_style()).centered();
        frame.render_widget(line, area.centered_vertically(Constraint::Length(1)));
    }

    fn hints(&self, _s: &'static Strings) -> Vec<Hint<'static>> {
        Vec::new()
    }
}
