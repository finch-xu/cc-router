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

pub mod draft;
pub mod overview;
pub mod placeholder;
pub mod subscriptions;
pub mod virtual_models;

use overview::Overview;
use placeholder::Placeholder;
use subscriptions::Subscriptions;
use virtual_models::VirtualModels;

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
    /// 页面自己的键位。全局键 (切页 / 帮助 / 退出 / 刷新) 由 `App` 先处理, 到不了这里。`s`:
    /// Task 5 起页面自己需要拼装本地化文案 (比如 picker 标题、拒绝操作的 toast) 才加的参数,
    /// 之前的页面 (总览 / 占位) 用不到就地忽略。
    fn handle_key(&mut self, key: KeyEvent, store: &Store, s: &'static Strings) -> Option<Action>;
    fn update(&mut self, action: &Action, store: &Store, s: &'static Strings) -> Vec<Cmd>;
    /// `&mut self`: 只允许为动效记录几何信息, 不改业务状态 —— 同一状态画两次必须得到同一帧。
    fn draw(&mut self, frame: &mut Frame, area: Rect, ctx: &mut DrawCtx);
    /// 底栏左侧显示的页面键位。
    fn hints(&self, s: &'static Strings) -> Vec<Hint<'static>>;
    /// 帮助弹窗里本页面的键位 (全局键由 `App` 提供)。默认空。
    fn help(&self, _s: &'static Strings) -> &'static [(&'static str, &'static str)] {
        &[]
    }
    /// `Store` 刚接受了一份订阅列表, `changed` 是状态变了的 id。默认什么都不做。`store`/`s`
    /// (S1(c), fix round P3b): 编辑类页面 (订阅详情页) 需要在这一刻 (而不是等下一次真正的
    /// `update()` 调用) 就核对一遍自己的草稿是不是对应的订阅已经消失——`Store` 这一刻已经接受了
    /// 新列表, 早一帧发现总比晚一帧好。
    fn on_subscriptions_changed(&mut self, _changed: &[String], _store: &Store, _s: &'static Strings) {}

    /// 是否有未保存的草稿。默认否——大多数页面只读展示后端数据; 编辑类页面 (P3b 起) 覆盖它。
    /// 草稿只放页面自己的状态, 不进 `Store` (spec 全局约束)。
    fn is_dirty(&self) -> bool {
        false
    }

    /// 丢弃当前草稿, 回到与 `Store` 一致的状态。默认什么都不做 (配合 `is_dirty` 默认 `false`)。
    fn discard_changes(&mut self) {}

    /// 一次就地操作 (`Mutation`) 落地了 (成功或失败), `App::finish_mutation` 对**全部**页面各调
    /// 一次 (Task 5 起; Task 6 的虚拟模型页也会用到)——页面自己按 `mutation` 的形状判断这跟自己
    /// 有没有关系 (比如订阅页只关心 `id` 匹配自己当前草稿的 `UpdateSlots`)。默认什么都不做。
    fn on_mutation_done(&mut self, _mutation: &Mutation, _ok: bool) {}

    /// I1 (fix round final): 一次就地操作**刚刚发出去** (`App::start_mutation` 真的往忙碌表里插入
    /// 那一刻), `App` 对**全部**页面各调一次——与 `on_mutation_done` 成对, 让编辑类页面知道"这个
    /// 实体现在有一次保存在飞行中", 从而拒绝任何会继续修改同一份草稿的按键 (避免飞行中的编辑被
    /// 落地的保存结果悄悄冲掉, D1 的姊妹问题)。默认什么都不做。
    fn on_mutation_started(&mut self, _mutation: &Mutation) {}

    /// M1 (fix round final): `Store` 刚**接受**了一份新的订阅列表或虚拟模型列表 (不管有没有变化),
    /// `App` 对**全部**页面各调一次——编辑类页面借这个时机立刻核对一遍 `is_dirty()` 缓存 (`Draft`
    /// 的 `sync`), 不用等下一次真正的 `Component::update()` 调用 (轮询最多要等 5 秒) 才发现"刚接受
    /// 的新值其实已经和草稿相等了"。与 `on_subscriptions_changed` 的区别: 后者只在订阅列表**变化**
    /// 时触发、只广播"哪些 id 变了" (给闪烁用); 这个方法订阅列表和虚拟模型列表都触发、不管内容
    /// 变没变、也不带 diff, 纯粹是"该核对草稿了"的信号。默认什么都不做。
    fn on_store_changed(&mut self, _store: &Store) {}

    /// 页面在 `update()` 内部想弹一条 toast (`update` 签名只能返回 `Vec<Cmd>`, 塞不进一个
    /// `Action`) 时, 存进这里; `App` 在调用 `update()` 之后轮询一次取走。默认没有待发的通知。
    fn take_notice(&mut self) -> Option<(ToastKind, String)> {
        None
    }
}

/// 三个页面的集合, 取代散字段 + 两个手写的 `select_page` / `select_page_ref` helper。字段公开是
/// 为了调用方能直接访问具体类型自己的方法 (比如 `App::draw` 取 `pages.overview.logo_area()`),
/// 不用为每个页面特有的方法单独在这里开一个洞。
#[derive(Default)]
pub struct Pages {
    pub overview: Overview,
    pub subscriptions: Subscriptions,
    pub virtual_models: VirtualModels,
    pub placeholder: Placeholder,
}

impl Pages {
    /// 按 `tab` 选一个页面的只读引用。穷尽 match, 不许 `_ =>`——新增 `Tab` 忘了在这里接住会编译
    /// 失败, 而不是静默落到占位页。
    pub fn get(&self, tab: Tab) -> &dyn Component {
        match tab {
            Tab::Overview => &self.overview,
            Tab::Subscriptions => &self.subscriptions,
            Tab::VirtualModels => &self.virtual_models,
            Tab::Live | Tab::Logs => &self.placeholder,
        }
    }

    /// 同上, 可变版本。
    pub fn get_mut(&mut self, tab: Tab) -> &mut dyn Component {
        match tab {
            Tab::Overview => &mut self.overview,
            Tab::Subscriptions => &mut self.subscriptions,
            Tab::VirtualModels => &mut self.virtual_models,
            Tab::Live | Tab::Logs => &mut self.placeholder,
        }
    }

    /// 给每个页面各恰好一次的机会 (占位页被 `Live` / `Logs` 两个 `Tab` 共用, 这里也只调一次,
    /// 不是两次)。`App::notify_subscriptions_changed` 用它取代手写的「一个个列出字段名」, 以后加
    /// 新页面只改这一处。
    pub fn for_each_mut(&mut self, mut f: impl FnMut(&mut dyn Component)) {
        f(&mut self.overview);
        f(&mut self.subscriptions);
        f(&mut self.virtual_models);
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
        assert_eq!(count, 4, "总览 / 订阅 / 虚拟模型 / 占位四个页面字段各恰好一次, 占位页不该因为被多个 Tab 共用就多算");
    }
}
