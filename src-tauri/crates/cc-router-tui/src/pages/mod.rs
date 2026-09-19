//! 页面。每个页面是一个 [`Component`]: 官方 Component 模板的简化版 (spec §4.3)。

use std::collections::HashMap;

use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::Frame;

use crate::action::{Action, BusyKey, Cmd, Mutation, Tab};
use crate::fx::Fx;
use crate::i18n::Strings;
use crate::store::Store;
use crate::theme::Theme;
use crate::widgets::keybar::Hint;
use crate::widgets::toast::ToastKind;

pub mod overview;
pub mod placeholder;
pub mod subscriptions;

use overview::Overview;
use placeholder::Placeholder;
use subscriptions::Subscriptions;

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
    /// 正在进行的就地操作, 键是 [`BusyKey`] (目前只会出现 `Subscription` 变体)。订阅页拿它把
    /// 忙碌行的状态符号换成 spinner。
    pub busy: &'a HashMap<BusyKey, Mutation>,
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

    /// 是否有未保存的草稿。默认否——大多数页面只读展示后端数据; 编辑类页面 (P3b 起) 覆盖它。
    /// 草稿只放页面自己的状态, 不进 `Store` (spec 全局约束)。
    fn is_dirty(&self) -> bool {
        false
    }

    /// 丢弃当前草稿, 回到与 `Store` 一致的状态。默认什么都不做 (配合 `is_dirty` 默认 `false`)。
    fn discard_changes(&mut self) {}
}

/// 三个页面的集合, 取代散字段 + 两个手写的 `select_page` / `select_page_ref` helper。字段公开是
/// 为了调用方能直接访问具体类型自己的方法 (比如 `App::draw` 取 `pages.overview.logo_area()`),
/// 不用为每个页面特有的方法单独在这里开一个洞。
#[derive(Default)]
pub struct Pages {
    pub overview: Overview,
    pub subscriptions: Subscriptions,
    pub placeholder: Placeholder,
}

impl Pages {
    /// 按 `tab` 选一个页面的只读引用。穷尽 match, 不许 `_ =>`——新增 `Tab` 忘了在这里接住会编译
    /// 失败, 而不是静默落到占位页。
    pub fn get(&self, tab: Tab) -> &dyn Component {
        match tab {
            Tab::Overview => &self.overview,
            Tab::Subscriptions => &self.subscriptions,
            Tab::VirtualModels | Tab::Live | Tab::Logs => &self.placeholder,
        }
    }

    /// 同上, 可变版本。
    pub fn get_mut(&mut self, tab: Tab) -> &mut dyn Component {
        match tab {
            Tab::Overview => &mut self.overview,
            Tab::Subscriptions => &mut self.subscriptions,
            Tab::VirtualModels | Tab::Live | Tab::Logs => &mut self.placeholder,
        }
    }

    /// 给每个页面各恰好一次的机会 (占位页被 `VirtualModels` / `Live` / `Logs` 三个 `Tab` 共用,
    /// 这里也只调一次, 不是三次)。`App::notify_subscriptions_changed` 用它取代手写的「一个个列出
    /// 字段名」, 以后加新页面只改这一处。
    pub fn for_each_mut(&mut self, mut f: impl FnMut(&mut dyn Component)) {
        f(&mut self.overview);
        f(&mut self.subscriptions);
        f(&mut self.placeholder);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn for_each_mut_visits_every_page_exactly_once() {
        let mut pages = Pages::default();
        let mut count = 0;
        pages.for_each_mut(|_| count += 1);
        assert_eq!(count, 3, "总览 / 订阅 / 占位三个页面字段各恰好一次, 占位页不该因为被多个 Tab 共用就多算");
    }
}
