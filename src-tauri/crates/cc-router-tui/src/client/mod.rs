pub mod discovery;
pub mod dto;
pub mod http;
pub mod sse;

pub use http::{Client, ClientError, EventStream};

/// TUI 调用的后端 command 名。集中在这里是为了让主 crate 的契约测试
/// (`src/tui_contract.rs::commands_are_registered`) 能逐个核对它们仍在 `web_commands!` 表里。
pub mod commands {
    pub const PROXY_STATUS: &str = "proxy_status";
    pub const GET_SETTINGS: &str = "get_settings";
    pub const LIST_SUBSCRIPTIONS: &str = "list_subscriptions";
    pub const GET_OVERALL_STATS: &str = "get_overall_stats";
    pub const GET_DAILY_SERIES: &str = "get_daily_series";
    pub const SET_SUBSCRIPTION_ENABLED: &str = "set_subscription_enabled";
    pub const TEST_CONNECTION: &str = "test_connection";
    pub const REFRESH_MODEL_LIST: &str = "refresh_model_list";
    pub const REFRESH_SUBSCRIPTION_BALANCE: &str = "refresh_subscription_balance";
    pub const UPDATE_SUBSCRIPTION: &str = "update_subscription";
    pub const LIST_VIRTUAL_MODELS: &str = "list_virtual_models";
    pub const UPDATE_VIRTUAL_MODEL: &str = "update_virtual_model";
    /// 契约测试遍历这张表; 加新 command 时同时加进来。
    pub const ALL: &[&str] = &[
        PROXY_STATUS,
        GET_SETTINGS,
        LIST_SUBSCRIPTIONS,
        GET_OVERALL_STATS,
        GET_DAILY_SERIES,
        SET_SUBSCRIPTION_ENABLED,
        TEST_CONNECTION,
        REFRESH_MODEL_LIST,
        REFRESH_SUBSCRIPTION_BALANCE,
        UPDATE_SUBSCRIPTION,
        LIST_VIRTUAL_MODELS,
        UPDATE_VIRTUAL_MODEL,
    ];
}
