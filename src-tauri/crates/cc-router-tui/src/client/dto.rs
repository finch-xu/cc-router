//! 视图结构体: 只声明 TUI 实际用到的字段, 其余忽略 (serde 默认行为)。
//! 与后端 DTO 的契约由主 crate 的 `src/tui_contract.rs` 锁住: 那边把主 crate 作为被测对象、
//! 本 crate 作为 dev-dependency, 用真实 DTO 序列化后反序列化进这里的结构体。
//! 后端改字段名 / 枚举值 → 那边的测试当场失败。给 TUI 加新字段时同步在那边加一条断言。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ProxyStatus {
    pub running: bool,
    pub mode: String,
    pub http_port: Option<u16>,
    pub https_port: Option<u16>,
    pub listen_all: bool,
    pub base_url: String,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionState {
    Healthy,
    RateLimited,
    QuotaExhausted,
    TransientError,
    AuthFailed,
    Disabled,
    /// 后端将来加了新状态时, 旧 TUI 不应该整个列表都解析失败。
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuotaPeriod {
    Daily,
    Weekly,
    Monthly,
    Total,
    #[serde(other)]
    Unknown,
}

/// 一个周期的 token 用量。`limit` 为 None = 该周期没设上限。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct QuotaUsage {
    pub period: QuotaPeriod,
    #[serde(default)]
    pub limit: Option<u64>,
    pub input: u64,
    pub output: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
    pub exceeded: bool,
}

impl QuotaUsage {
    /// 与后端 `QuotaBucket::total` 同口径: 四类 token 之和。
    pub fn used(&self) -> u64 {
        self.input + self.output + self.cache_creation + self.cache_read
    }

    /// 已用比例 (0.0–1.0); 没设上限返回 None。
    pub fn ratio(&self) -> Option<f64> {
        let limit = self.limit.filter(|l| *l > 0)?;
        Some((self.used() as f64 / limit as f64).min(1.0))
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Subscription {
    pub id: String,
    pub display_name: String,
    pub provider_display_name: String,
    pub enabled: bool,
    pub state: SubscriptionState,
    /// Unix 毫秒
    pub cooldown_until: Option<i64>,
    pub last_error_message: Option<String>,
    /// 后端 `SubscriptionRuntime::is_dispatchable` 的结果 (启用 + 健康 + 不在冷却 + 限额未满)。
    /// TUI 不自己重算这四个条件。
    pub is_dispatchable: bool,
    #[serde(default)]
    pub quota_usage: Vec<QuotaUsage>,
    pub provider_id: String,
    pub base_url: String,
    /// 后端是枚举 (`AuthType`, `#[serde(rename_all = "snake_case")]`), 这里按字符串收, 只用于显示。
    pub auth_type: String,
    pub model_slots: ModelSlots,
    #[serde(default)]
    pub slot_efforts: SlotEfforts,
    #[serde(default)]
    pub referenced_by: Vec<String>,
    #[serde(default)]
    pub balance_supported: bool,
    #[serde(default)]
    pub balance_cache: Option<BalanceCache>,
    #[serde(default)]
    pub model_cache: Option<ModelCache>,
}

/// 与后端 `SubscriptionPatch::model_slots` 整块替换: `fallback` 空串 = 未配置, 与其它槽位一样
/// **总是**序列化 (后端字段是普通 `String` + `#[serde(default)]`, 不是 `Option`)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ModelSlots {
    pub fable: String,
    pub opus: String,
    pub sonnet: String,
    pub haiku: String,
    #[serde(default)]
    pub fallback: String,
}

/// 五个模型槽位, 与 [`ModelSlots`] 的字段一一对应。`widgets::picker::PickerTag` 用它区分「给哪个
/// 槽位选值」(Task 5 起)。放在 dto.rs 而不是 action.rs, 因为它描述的是后端数据形状 (槽位这个
/// 概念), 不是某一次 UI 交互。`Fallback` 不参与 `SlotEfforts` (后端 `SlotEfforts::get` 同样不含
/// fallback), `get`/`set` 对它分别恒返回 `None` / 忽略写入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    Fable,
    Opus,
    Sonnet,
    Haiku,
    Fallback,
}

impl ModelSlots {
    pub fn get(&self, slot: Slot) -> &str {
        match slot {
            Slot::Fable => &self.fable,
            Slot::Opus => &self.opus,
            Slot::Sonnet => &self.sonnet,
            Slot::Haiku => &self.haiku,
            Slot::Fallback => &self.fallback,
        }
    }

    pub fn set(&mut self, slot: Slot, value: String) {
        match slot {
            Slot::Fable => self.fable = value,
            Slot::Opus => self.opus = value,
            Slot::Sonnet => self.sonnet = value,
            Slot::Haiku => self.haiku = value,
            Slot::Fallback => self.fallback = value,
        }
    }
}

/// 每个模型槽位的 reasoning effort 覆盖。字段缺失 (老数据 `'{}'`) = 全 auto。**auto = 该键不序列化**
/// (`skip_serializing_if`, 与后端 `SlotEfforts` 的 `Option<String>` + `skip_serializing_if` 同一套
/// 编码), 绝不发 `null` 或 `""`。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct SlotEfforts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opus: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sonnet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub haiku: Option<String>,
}

impl SlotEfforts {
    pub fn get(&self, slot: Slot) -> Option<&str> {
        match slot {
            Slot::Fable => self.fable.as_deref(),
            Slot::Opus => self.opus.as_deref(),
            Slot::Sonnet => self.sonnet.as_deref(),
            Slot::Haiku => self.haiku.as_deref(),
            Slot::Fallback => None,
        }
    }

    pub fn set(&mut self, slot: Slot, value: Option<String>) {
        match slot {
            Slot::Fable => self.fable = value,
            Slot::Opus => self.opus = value,
            Slot::Sonnet => self.sonnet = value,
            Slot::Haiku => self.haiku = value,
            Slot::Fallback => {}
        }
    }
}

/// 允许的槽位 effort 档位; 必须与后端 `commands::subscriptions::ALLOWED_SLOT_EFFORTS` 相等
/// (契约测试 `tui_effort_choices_equal_the_backend_allowlist` 锁住)。刻意不含 `minimal`
/// (OpenAI 系专有档, 理由见后端同名常量的注释)。
pub const EFFORT_CHOICES: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// 虚拟模型的调度模式。与后端 `virtual_model::model::RoutingMode` 对应 (`rename_all = "snake_case"`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingMode {
    Sequential,
    RoundRobin,
    Sticky,
    /// 后端将来加了新模式时, 旧 TUI 不应该整个列表解析失败。
    #[serde(other)]
    Unknown,
}

impl RoutingMode {
    /// 虚拟模型页按 `m` 循环切换模式用: `Unknown` (旧 TUI 不认得的新模式) 落回 `Sequential`,
    /// 不在三个已知模式之间瞎绕。
    pub fn next(self) -> Self {
        match self {
            RoutingMode::Sequential => RoutingMode::RoundRobin,
            RoutingMode::RoundRobin => RoutingMode::Sticky,
            RoutingMode::Sticky => RoutingMode::Sequential,
            RoutingMode::Unknown => RoutingMode::Sequential,
        }
    }

    /// 发给后端的线上名字。与后端 `#[serde(rename_all = "snake_case")]` 的输出必须逐字相同——
    /// `tui_contract.rs::routing_mode_wire_names_round_trip` 锁住这一点, 这里手写而不是复用
    /// `serde` 派生是因为 `runtime.rs` 直接拿这个函数的返回值拼 JSON, 不经过 `Serialize`。
    pub fn as_wire(self) -> &'static str {
        match self {
            RoutingMode::Sequential => "sequential",
            RoutingMode::RoundRobin => "round_robin",
            RoutingMode::Sticky => "sticky",
            // Unknown 不该被发回后端 (只应该出现在「读到了不认识的模式」这一刻); 给个安全的
            // 缺省值而不是 panic, 与 `next()` 的兜底一致。
            RoutingMode::Unknown => "sequential",
        }
    }
}

/// `list_virtual_models` 的一项。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct VirtualModel {
    pub name: String,
    pub mode: RoutingMode,
    pub subscription_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ModelCache {
    pub fetched_at: i64,
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct BalanceCache {
    pub fetched_at: i64,
    pub snapshot: BalanceSnapshot,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct BalanceSnapshot {
    /// 账户是否可用 (DeepSeek `is_available`)。None = 该 provider 不报告此字段。
    #[serde(default)]
    pub is_available: Option<bool>,
    pub entries: Vec<BalanceEntry>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct BalanceEntry {
    pub label: String,
    pub value_text: String,
    pub unit: String,
    #[serde(default)]
    pub hint: Option<String>,
    pub severity: BalanceSeverity,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BalanceSeverity {
    Normal,
    Low,
    Critical,
    /// 后端将来加了新级别时, 旧 TUI 不应该整个订阅解析失败。
    #[serde(other)]
    Unknown,
}

/// `test_connection` 的返回值。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct TestConnectionResult {
    pub ok: bool,
    pub message: String,
    /// 上游 HTTP 状态码; 网络错误时为 None。
    #[serde(default)]
    pub http_status: Option<u16>,
    /// 实际用于测试的 model 名 (从 slots 或 example_models 兜底选出)。
    #[serde(default)]
    pub model_used: Option<String>,
    /// 测试通过且触发了状态机复活 → true。
    pub state_reset: bool,
}

/// `refresh_model_list` 的返回值。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RefreshModelsResult {
    Auto { models: Vec<ModelInfo>, fetched_at: i64 },
    ManualFallback { reason: String },
}

/// `refresh_subscription_balance` 的返回值。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RefreshBalanceResult {
    Success { snapshot: BalanceSnapshot, fetched_at: i64 },
    Failed { reason: String },
    Unsupported,
}

impl Subscription {
    /// 设了上限的周期里已用比例最高的那个 —— 总览页每行只放得下一条限额。
    pub fn tightest_quota(&self) -> Option<&QuotaUsage> {
        self.quota_usage
            .iter()
            .filter(|q| q.ratio().is_some())
            .max_by(|a, b| a.ratio().partial_cmp(&b.ratio()).unwrap_or(std::cmp::Ordering::Equal))
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Settings {
    pub preferred_language: String,
    pub tui_enabled: bool,
    /// 代理的 Bearer 鉴权是否开启 (总览页显示)。
    pub auth_enabled: bool,
}

/// `get_overall_stats` 的返回值里总览页用到的部分。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct OverallStats {
    pub total_requests: i64,
    pub success_rate_pct: f64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_creation_tokens: i64,
    pub total_cache_read_tokens: i64,
}

impl OverallStats {
    pub fn total_tokens(&self) -> i64 {
        self.total_input_tokens + self.total_output_tokens + self.total_cache_creation_tokens + self.total_cache_read_tokens
    }
}

/// `get_daily_series` 的一个点。range = today 时按小时分桶, `hour` 有值 (本地 0–23);
/// 后端只返回有数据的桶, 补零由 [`hourly_buckets`] 负责。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct SeriesPoint {
    #[serde(default)]
    pub hour: Option<i64>,
    pub request_count: i64,
}

/// 把稀疏的按小时序列铺成固定 24 格。
pub fn hourly_buckets(points: &[SeriesPoint]) -> [u64; 24] {
    let mut out = [0u64; 24];
    for p in points {
        if let Some(h) = p.hour.filter(|h| (0..24).contains(h)) {
            out[h as usize] += p.request_count.max(0) as u64;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_state_does_not_break_the_list() {
        let subs: Vec<Subscription> = serde_json::from_str(
            r#"[{"id":"a","display_name":"n","provider_display_name":"p","enabled":true,
                 "state":"some_future_state","cooldown_until":null,"last_error_message":null,
                 "is_dispatchable":false,"provider_id":"p","base_url":"","auth_type":"api_key",
                 "model_slots":{"fable":"","opus":"","sonnet":"","haiku":""},"extra":1}]"#,
        )
        .unwrap();
        assert_eq!(subs[0].state, SubscriptionState::Unknown);
        assert!(subs[0].quota_usage.is_empty());
    }

    fn quota(period: QuotaPeriod, limit: Option<u64>, used: u64) -> QuotaUsage {
        QuotaUsage { period, limit, input: used, output: 0, cache_creation: 0, cache_read: 0, exceeded: false }
    }

    #[test]
    fn tightest_quota_ignores_unlimited_periods() {
        let mut sub: Subscription = serde_json::from_str(
            r#"{"id":"a","display_name":"n","provider_display_name":"p","enabled":true,"state":"healthy",
                "is_dispatchable":true,"provider_id":"p","base_url":"","auth_type":"api_key",
                "model_slots":{"fable":"","opus":"","sonnet":"","haiku":""}}"#,
        )
        .unwrap();
        assert!(sub.tightest_quota().is_none());
        sub.quota_usage = vec![
            quota(QuotaPeriod::Daily, Some(100), 20),
            quota(QuotaPeriod::Weekly, None, 9_999),
            quota(QuotaPeriod::Monthly, Some(1000), 900),
        ];
        assert_eq!(sub.tightest_quota().unwrap().period, QuotaPeriod::Monthly);
    }

    #[test]
    fn ratio_is_clamped_and_zero_limit_is_unlimited() {
        assert_eq!(quota(QuotaPeriod::Daily, Some(10), 25).ratio(), Some(1.0));
        assert_eq!(quota(QuotaPeriod::Daily, Some(0), 25).ratio(), None);
    }

    #[test]
    fn hourly_buckets_fill_gaps_and_ignore_bad_hours() {
        let pts = vec![
            SeriesPoint { hour: Some(0), request_count: 3 },
            SeriesPoint { hour: Some(23), request_count: 7 },
            SeriesPoint { hour: Some(24), request_count: 99 },
            SeriesPoint { hour: None, request_count: 99 },
        ];
        let b = hourly_buckets(&pts);
        assert_eq!((b[0], b[23], b.iter().sum::<u64>()), (3, 7, 10));
    }
}
