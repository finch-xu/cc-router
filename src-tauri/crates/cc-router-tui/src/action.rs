//! 单向数据流的两种消息: [`Action`] 进 (按键 / 定时 / 网络结果), [`Cmd`] 出 (要主循环去做的副作用)。
//! `App::update` 是 `(状态, Action) → (新状态, Vec<Cmd>)` 的同步函数, 不碰网络也不碰终端, 所以能直接单测。

use crate::client::dto::{
    OverallStats, ProxyStatus, RefreshBalanceResult, RefreshModelsResult, SeriesPoint, Settings, Subscription, TestConnectionResult,
};
use crate::widgets::picker::{PickerChoice, PickerSpec, PickerTag};

/// 五个标签页, 顺序即 `1`–`5` 与 `Strings::tabs` 的下标。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tab {
    Overview,
    Subscriptions,
    VirtualModels,
    Live,
    Logs,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::Overview, Tab::Subscriptions, Tab::VirtualModels, Tab::Live, Tab::Logs];

    /// 0 起算的下标, 与 `Strings::tabs` 对齐。
    pub fn index(self) -> usize {
        match self {
            Tab::Overview => 0,
            Tab::Subscriptions => 1,
            Tab::VirtualModels => 2,
            Tab::Live => 3,
            Tab::Logs => 4,
        }
    }

    pub fn from_index(i: usize) -> Option<Tab> {
        Tab::ALL.get(i).copied()
    }

    /// 末尾绕回开头。
    pub fn next(self) -> Tab {
        Tab::ALL[(self.index() + 1) % Tab::ALL.len()]
    }

    /// 开头绕回末尾。
    pub fn prev(self) -> Tab {
        let n = Tab::ALL.len();
        Tab::ALL[(self.index() + n - 1) % n]
    }
}

/// 总览页一次整页加载的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct OverviewData {
    pub status: ProxyStatus,
    pub settings: Settings,
    pub stats: OverallStats,
    pub series: Vec<SeriesPoint>,
    pub subscriptions: Vec<Subscription>,
}

/// 可去重、可补跑的加载。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fetch {
    Overview,
    Subscriptions,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FetchData {
    Overview(Box<OverviewData>),
    Subscriptions(Vec<Subscription>),
}

/// 订阅页的四个就地操作。**永不去重、永不补跑** (与 [`Fetch`] 相反): `runtime.rs` 对每一个
/// `Cmd::Mutate` 都直接 `spawn`, 不经过 `Fetches`。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Mutation {
    SetEnabled { id: String, enabled: bool },
    TestConnection { id: String },
    RefreshModels { id: String },
    RefreshBalance { id: String },
}

/// 忙碌表 (`App::busy`) 判重用的键。目前四种就地操作都作用于订阅, 只产生 `Subscription` 变体;
/// `VirtualModel` 留给虚拟模型页的编辑操作 (Task 4 起) 用, 本 Task 只定义不产出。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BusyKey {
    Subscription(String),
    VirtualModel(String),
}

impl Mutation {
    /// 取代旧的 `subscription_id()`: 旧函数假设「所有变更都作用于某条订阅」, 虚拟模型页的编辑
    /// 操作不满足这个假设, 需要一个能区分「这次变更判重键属于哪一类」的类型。
    pub fn busy_key(&self) -> BusyKey {
        match self {
            Mutation::SetEnabled { id, .. }
            | Mutation::TestConnection { id }
            | Mutation::RefreshModels { id }
            | Mutation::RefreshBalance { id } => BusyKey::Subscription(id.clone()),
        }
    }

    /// 这次变更完成后该重新拉取哪些加载。现有四种都只影响订阅列表。
    pub fn refetch(&self) -> &'static [Fetch] {
        &[Fetch::Subscriptions]
    }
}

/// 一次就地操作的结果。
#[derive(Debug, Clone, PartialEq)]
pub enum MutationOutcome {
    EnabledSet,
    Tested(TestConnectionResult),
    Models(RefreshModelsResult),
    Balance(RefreshBalanceResult),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    Quit,
    Fetch(Fetch),
    Mutate(Mutation),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// 来自 `q`: 当前页面有未保存修改时会先弹确认弹窗, 不直接退出——与 [`Action::ForceQuit`]
    /// 的区别只在这一点, `Cmd::Quit` 本身不变。
    Quit,
    /// 来自 `Ctrl+C`: 无论是否有弹窗打开、当前页面是否有未保存修改, 都立即退出, 不确认。
    ForceQuit,
    SwitchTab(Tab),
    NextTab,
    PrevTab,
    ToggleHelp,
    ClosePopup,
    /// 打开一个「是 / 否」确认弹窗; `on_yes` 是选「是」后真正要执行的 `Action`。页面自己想
    /// 请求确认 (比如「放弃修改」) 时也可以从 `handle_key` 直接返回这个。
    OpenConfirm { prompt: String, on_yes: Box<Action> },
    /// 用户在确认弹窗里选了「是」: 先让当前页面丢弃草稿 (`Component::discard_changes`), 再执行
    /// `inner`——`inner` 走一次普通 `App::update`, 但此时 dirty 已经被清空, 不会被再次拦截确认。
    Confirmed(Box<Action>),
    /// 页面主动清空自己的草稿 (比如按 Esc 放弃编辑) 时用; `App` 收到后调用当前页面的
    /// `discard_changes()`, 不产出任何 `Cmd`。
    DiscardDraft,
    Refresh,
    /// 250ms 一次。`now_ms` 是 Unix 毫秒 —— 冷却倒计时要和后端给的 `cooldown_until` 比。
    Tick { now_ms: i64 },
    /// 事件流连上了 (首次或重连)。
    Connected { app_version: String },
    /// 事件流断了, 主循环正在退避重连。
    ConnectionLost,
    /// 后端推来的事件; `data` 是原始 JSON 文本。
    Sse { name: String, data: String },
    /// `issued`: 主循环发起这次加载时盖的单调递增序号。
    FetchDone { fetch: Fetch, issued: u64, result: Result<FetchData, String> },
    /// 订阅页的一次就地操作。
    Mutate(Mutation),
    /// 一次就地操作跑完了 (成功或失败)。`barrier`: 变更完成那一刻已发起的所有加载的最大序号——
    /// 由 `runtime.rs::process_action` 在**收到**这条消息时补盖 (`spawn_mutation` 发送时只是占位符
    /// `0`); `App` 据此对 `mutation.refetch()` 里每个目标调用对应的 `set_*_barrier`, 挡住那些在
    /// 变更完成前就已经发起、内容还是变更前旧值的加载晚到时把乐观更新冲回去。
    MutationDone { mutation: Mutation, barrier: u64, result: Result<MutationOutcome, String> },
    /// 打开一个过滤选择弹窗 (Task 5/6 起从页面发起: 选模型 / 选 effort / 给虚拟模型加订阅)。
    OpenPicker(PickerSpec),
    /// 用户在选择弹窗里选定了一行 (或输入了自定义值)。`App` 收到后先关弹窗 (带关闭动效), 再原样
    /// 转给当前页面的 `update`——页面据 `tag` 知道该把 `choice` 填到哪。Task 5 之前没有真正的消费者,
    /// 页面的 `update` 会直接忽略它。
    PickerDone { tag: PickerTag, choice: PickerChoice },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mutation_declares_its_busy_key_and_refetch() {
        let cases = [
            Mutation::SetEnabled { id: "1".into(), enabled: true },
            Mutation::TestConnection { id: "1".into() },
            Mutation::RefreshModels { id: "1".into() },
            Mutation::RefreshBalance { id: "1".into() },
        ];
        for m in cases {
            assert_eq!(m.busy_key(), BusyKey::Subscription("1".into()), "{m:?} 应该产出 Subscription 忙碌键");
            assert_eq!(m.refetch(), &[Fetch::Subscriptions], "{m:?} 完成后应该重拉订阅列表");
        }
    }

    #[test]
    fn tab_index_round_trips_and_wraps() {
        for (i, tab) in Tab::ALL.into_iter().enumerate() {
            assert_eq!(tab.index(), i);
            assert_eq!(Tab::from_index(i), Some(tab));
        }
        assert_eq!(Tab::from_index(5), None);
        assert_eq!(Tab::from_index(usize::MAX), None);

        assert_eq!(Tab::Overview.next(), Tab::Subscriptions);
        assert_eq!(Tab::Logs.next(), Tab::Overview, "末尾绕回开头");
        assert_eq!(Tab::Overview.prev(), Tab::Logs, "开头绕回末尾");
        assert_eq!(Tab::Logs.prev(), Tab::Live);
    }
}
