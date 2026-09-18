//! 单向数据流的两种消息: [`Action`] 进 (按键 / 定时 / 网络结果), [`Cmd`] 出 (要主循环去做的副作用)。
//! `App::update` 是 `(状态, Action) → (新状态, Vec<Cmd>)` 的同步函数, 不碰网络也不碰终端, 所以能直接单测。

use crate::client::dto::{OverallStats, ProxyStatus, SeriesPoint, Settings, Subscription};

/// 总览页一次整页加载的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct OverviewData {
    pub status: ProxyStatus,
    pub settings: Settings,
    pub stats: OverallStats,
    pub series: Vec<SeriesPoint>,
    pub subscriptions: Vec<Subscription>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Quit,
    /// 0 起算的标签下标
    SwitchTab(usize),
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
    OverviewLoaded(Box<OverviewData>),
    SubscriptionsLoaded(Vec<Subscription>),
    /// `cmd` 是哪一次加载失败了 —— 主循环靠它清「进行中」标记。
    LoadFailed { cmd: Cmd, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cmd {
    Quit,
    FetchOverview,
    FetchSubscriptions,
}
