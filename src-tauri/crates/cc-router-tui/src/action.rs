//! 单向数据流的两种消息: [`Action`] 进 (按键 / 定时 / 网络结果), [`Cmd`] 出 (要主循环去做的副作用)。
//! `App::update` 是 `(状态, Action) → (新状态, Vec<Cmd>)` 的同步函数, 不碰网络也不碰终端, 所以能直接单测。

use crate::client::dto::{
    OverallStats, ProxyStatus, RefreshBalanceResult, RefreshModelsResult, SeriesPoint, Settings, Subscription, TestConnectionResult,
};

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

impl Mutation {
    pub fn subscription_id(&self) -> &str {
        match self {
            Mutation::SetEnabled { id, .. }
            | Mutation::TestConnection { id }
            | Mutation::RefreshModels { id }
            | Mutation::RefreshBalance { id } => id,
        }
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
    Quit,
    SwitchTab(Tab),
    NextTab,
    PrevTab,
    ToggleHelp,
    ClosePopup,
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
    /// 一次就地操作跑完了 (成功或失败)。
    MutationDone { mutation: Mutation, result: Result<MutationOutcome, String> },
}

#[cfg(test)]
mod tests {
    use super::*;

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
