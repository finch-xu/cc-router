//! 页面。每个页面是一个 [`Component`]: 官方 Component 模板的简化版 (spec §4.3)。

use std::collections::HashMap;

use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::Frame;

use crate::action::{Action, Cmd, Mutation};
use crate::fx::Fx;
use crate::i18n::Strings;
use crate::store::Store;
use crate::theme::Theme;
use crate::widgets::keybar::Hint;
use crate::widgets::toast::ToastKind;

pub mod overview;
pub mod placeholder;
pub mod subscriptions;

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
    /// 多个页面共用的订阅列表, 只读。
    pub store: &'a Store,
    /// 正在进行的就地操作, 键是订阅 id。订阅页拿它把忙碌行的状态符号换成 spinner。
    pub busy: &'a HashMap<String, Mutation>,
    /// 每条订阅最近一次就地操作的结果 (与对应 toast 同一份文本), 键是订阅 id; 发起新操作时移除
    /// (`App::start_mutation`)。订阅详情页拿它在「状态」行后面画一条「上次操作」(I1 fix)。
    pub last_outcome: &'a HashMap<String, (ToastKind, String)>,
}

pub trait Component {
    /// 页面自己的键位。全局键 (切页 / 帮助 / 退出 / 刷新) 由 `App` 先处理, 到不了这里。
    fn handle_key(&mut self, key: KeyEvent, store: &Store) -> Option<Action>;
    fn update(&mut self, action: &Action, store: &Store) -> Vec<Cmd>;
    /// `&mut self`: 只允许为动效记录几何信息, 不改业务状态 —— 同一状态画两次必须得到同一帧。
    fn draw(&mut self, frame: &mut Frame, area: Rect, ctx: &mut DrawCtx);
    /// 底栏左侧显示的页面键位。
    fn hints(&self, s: &'static Strings) -> Vec<Hint<'static>>;
    /// 帮助弹窗里本页面的键位 (全局键由 `App` 提供)。默认空。
    fn help(&self, _s: &'static Strings) -> &'static [(&'static str, &'static str)] {
        &[]
    }
    /// `Store` 刚接受了一份订阅列表, `changed` 是状态变了的 id。默认什么都不做。
    fn on_subscriptions_changed(&mut self, _changed: &[String]) {}
}
