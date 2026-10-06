//! 视图结构体: 只声明 TUI 实际用到的字段, 其余忽略 (serde 默认行为)。
//! 与后端 DTO 的契约由主 crate 的 `src/tui_contract.rs` 锁住: 那边把主 crate 作为被测对象、
//! 本 crate 作为 dev-dependency, 用真实 DTO 序列化后反序列化进这里的结构体。
//! 后端改字段名 / 枚举值 → 那边的测试当场失败。给 TUI 加新字段时同步在那边加一条断言。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::i18n::Lang;
use crate::secret::Secret;

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
    /// 创建时写进 DB 的厂商名快照 (中文); 显示一律走 [`Subscription::provider_name`]。
    pub provider_display_name: String,
    /// 内置厂商的三语名字 (后端按 yaml 现查); 自定义订阅与 yaml 已删的厂商没有这个字段。
    #[serde(default)]
    pub provider_names: Option<LocalizedName>,
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
    /// 端点协议快照 ("messages" / "systemone"), 后端 `SubscriptionDto.endpoint_protocol`。老后端缺字段按 messages。
    #[serde(default = "default_protocol")]
    pub endpoint_protocol: String,
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
///
/// `Default` (五个槽位全空串) 给向导的 `SlotsDraft` 用: 它是「从零填」的草稿, 没有真实订阅可以
/// 打底。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ModelSlots {
    pub fable: String,
    pub opus: String,
    pub sonnet: String,
    pub haiku: String,
    #[serde(default)]
    pub fallback: String,
    /// Jev 槽 (仅 systemone 订阅): 空串 = 透传客户端 model。与 fallback 一样总是序列化。
    #[serde(default)]
    pub jev: String,
}

/// 五个模型槽位, 与 [`ModelSlots`] 的字段一一对应。`widgets::picker::PickerTag` 用它区分「给哪个
/// 槽位选值」。放在 dto.rs 而不是 action.rs, 因为它描述的是后端数据形状 (槽位这个
/// 概念), 不是某一次 UI 交互。`Fallback` 不参与 `SlotEfforts` (后端 `SlotEfforts::get` 同样不含
/// fallback), `get`/`set` 对它分别恒返回 `None` / 忽略写入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    Fable,
    Opus,
    Sonnet,
    Haiku,
    Fallback,
    /// 只属于 systemone 订阅 (对话订阅没有这个槽)。
    Jev,
}

fn default_protocol() -> String {
    "messages".to_string()
}

/// 第 6 个虚拟模型的名字; 只收 systemone 订阅。
pub const JEV_VM: &str = "model-jev";

/// 与后端 `VirtualModelName::accepts` 同一条规则 (后端仍会兜底拒绝)。
pub fn vm_accepts(vm_name: &str, sub: &Subscription) -> bool {
    (vm_name == JEV_VM) == sub.is_systemone()
}

/// 新建订阅时四个核心槽位的占位值。与桌面端 `uniformSlots("(pending)")` 逐字相同;
/// **兜底槽不填占位, 留空串** (桌面端的 `uniformSlots` 压根不设这个键)。订阅页与向导都引用这一份。
pub const PENDING_MODEL: &str = "(pending)";

impl ModelSlots {
    pub fn get(&self, slot: Slot) -> &str {
        match slot {
            Slot::Fable => &self.fable,
            Slot::Opus => &self.opus,
            Slot::Sonnet => &self.sonnet,
            Slot::Haiku => &self.haiku,
            Slot::Fallback => &self.fallback,
            Slot::Jev => &self.jev,
        }
    }

    pub fn set(&mut self, slot: Slot, value: String) {
        match slot {
            Slot::Fable => self.fable = value,
            Slot::Opus => self.opus = value,
            Slot::Sonnet => self.sonnet = value,
            Slot::Haiku => self.haiku = value,
            Slot::Fallback => self.fallback = value,
            Slot::Jev => self.jev = value,
        }
    }

    /// 两步向导第一步创建订阅时用: 四个核心槽位填 [`PENDING_MODEL`], 兜底槽留空串
    /// (与桌面端 `uniformSlots("(pending)")` 同一份约定——那边压根不设 fallback 这个键)。
    pub fn pending() -> Self {
        ModelSlots {
            fable: PENDING_MODEL.to_string(),
            opus: PENDING_MODEL.to_string(),
            sonnet: PENDING_MODEL.to_string(),
            haiku: PENDING_MODEL.to_string(),
            fallback: String::new(),
            jev: String::new(),
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
            Slot::Jev => None,
        }
    }

    pub fn set(&mut self, slot: Slot, value: Option<String>) {
        match slot {
            Slot::Fable => self.fable = value,
            Slot::Opus => self.opus = value,
            Slot::Sonnet => self.sonnet = value,
            Slot::Haiku => self.haiku = value,
            Slot::Fallback => {}
            Slot::Jev => {}
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

/// `route_attempt_started` / `route_attempt_finished` 的 payload (实时路由页用); started 没有
/// `success`。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RouteAttempt {
    pub subscription_id: String,
    pub virtual_model: String,
    #[serde(default)]
    pub success: Option<bool>,
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
    pub fn is_systemone(&self) -> bool {
        self.endpoint_protocol == "systemone"
    }

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

/// 与后端 `observability::request_log::RequestStatus` 对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestStatus {
    Success,
    Error,
    Timeout,
    /// 后端将来加了新状态时, 旧 TUI 不应该整页解析失败。
    #[serde(other)]
    Unknown,
}

impl RequestStatus {
    /// 过滤条件里发给后端的值; 必须与后端 `RequestStatus::as_str` 逐字相同 (契约锁住)。Unknown → "unknown"
    /// (不会被构造成过滤条件)。
    pub fn as_wire(self) -> &'static str {
        match self {
            RequestStatus::Success => "success",
            RequestStatus::Error => "error",
            RequestStatus::Timeout => "timeout",
            RequestStatus::Unknown => "unknown",
        }
    }
}

/// `list_requests` 的一行。后端 `commands::requests::RequestLogDto` 的全部字段; `Option` 一律
/// `#[serde(default)]`。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RequestLog {
    pub id: String,
    /// Unix 毫秒
    pub timestamp: i64,
    pub virtual_model_name: String,
    pub subscription_id: String,
    pub provider_id: String,
    pub endpoint_id: String,
    pub real_model_name: String,
    #[serde(default)]
    pub response_model_name: Option<String>,
    pub is_streaming: bool,
    pub status: RequestStatus,
    #[serde(default)]
    pub http_status: Option<i64>,
    #[serde(default)]
    pub total_latency_ms: Option<i64>,
    #[serde(default)]
    pub input_tokens: Option<i64>,
    #[serde(default)]
    pub output_tokens: Option<i64>,
    #[serde(default)]
    pub cache_creation_tokens: Option<i64>,
    #[serde(default)]
    pub cache_read_tokens: Option<i64>,
    #[serde(default)]
    pub error_message: Option<String>,
    #[serde(default)]
    pub upstream_response_body: Option<String>,
    #[serde(default)]
    pub client_tool: Option<String>,
    #[serde(default)]
    pub client_user_agent: Option<String>,
    #[serde(default)]
    pub client_version: Option<String>,
    #[serde(default)]
    pub client_ip: Option<String>,
    #[serde(default)]
    pub entry_kind: Option<String>,
    #[serde(default)]
    pub downstream_http_version: Option<String>,
    #[serde(default)]
    pub client_effort: Option<String>,
    #[serde(default)]
    pub effective_effort: Option<String>,
    #[serde(default)]
    pub effort_source: Option<String>,
    #[serde(default)]
    pub upstream_effort: Option<String>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub tools_offered_count: Option<i64>,
    #[serde(default)]
    pub tool_result_count: Option<i64>,
    #[serde(default)]
    pub tool_use_count: Option<i64>,
    /// JSON 字符串数组 (可能以 `"…"` 结尾表示被截断), 原样保存, 由日志页解析。
    #[serde(default)]
    pub tool_use_names: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RequestPage {
    pub items: Vec<RequestLog>,
    pub total: i64,
}

/// 后端 `RequestLogFilters` 里 TUI 暴露的三个维度 (后端另有 provider_id / client_tool, TUI 不用)。
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct RequestFilters {
    pub subscription_id: Option<String>,
    pub virtual_model_name: Option<String>,
    pub status: Option<RequestStatus>,
}

impl RequestFilters {
    pub fn is_empty(&self) -> bool {
        self.subscription_id.is_none() && self.virtual_model_name.is_none() && self.status.is_none()
    }
}

pub const REQUEST_PAGE_SIZE: u32 = 50;

/// 日志页想看的那一页。`page` 从 1 起; `Default` = 第 1 页、无过滤。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestQuery {
    pub page: u32,
    pub filters: RequestFilters,
}

impl Default for RequestQuery {
    fn default() -> Self {
        RequestQuery { page: 1, filters: RequestFilters::default() }
    }
}

impl RequestQuery {
    /// `list_requests` 的参数。网页桥接按 **camelCase** 取顶层参数 (`pageSize`, 见
    /// `src-tauri/src/proxy/web/api.rs` 的 `#[serde(rename_all = "camelCase")]`); `filters` 是后端
    /// `RequestLogFilters`, 键是 **snake_case**; 值为 None 的维度不发, `filters` 恒为一个对象 (可能是
    /// `{}`)。例: `{"page":2,"pageSize":50,"filters":{"subscription_id":"s1","virtual_model_name":"model-sonnet","status":"error"}}`
    pub fn to_args(&self) -> serde_json::Value {
        let mut filters = serde_json::Map::new();
        if let Some(v) = &self.filters.subscription_id {
            filters.insert("subscription_id".to_string(), serde_json::Value::String(v.clone()));
        }
        if let Some(v) = &self.filters.virtual_model_name {
            filters.insert("virtual_model_name".to_string(), serde_json::Value::String(v.clone()));
        }
        if let Some(v) = self.filters.status {
            filters.insert("status".to_string(), serde_json::Value::String(v.as_wire().to_string()));
        }
        serde_json::json!({
            "page": self.page,
            "pageSize": REQUEST_PAGE_SIZE,
            "filters": serde_json::Value::Object(filters),
        })
    }
}

/// 后端 `provider::model::LocalizedText`: 三语各一份, 后端保证齐全。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct LocalizedName {
    pub zh: String,
    pub en: String,
    pub ja: String,
}

impl LocalizedName {
    pub fn get(&self, lang: Lang) -> &str {
        match lang {
            Lang::Zh => &self.zh,
            Lang::En => &self.en,
            Lang::Ja => &self.ja,
        }
    }
}

impl Subscription {
    /// 界面上显示的厂商名: 内置厂商按界面语言取, 否则用快照。
    pub fn provider_name(&self, lang: Lang) -> &str {
        match &self.provider_names {
            Some(names) => names.get(lang),
            None => &self.provider_display_name,
        }
    }
}

/// `list_providers` 的一项。只声明 TUI 用得到的字段, 后端 `ProviderInfo` 其余字段 (homepage /
/// docs_url / compatibility 等) 由 serde 忽略。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Provider {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub endpoints: Vec<ProviderEndpoint>,
    #[serde(default)]
    pub default_endpoint: Option<String>,
    pub auth: ProviderAuth,
    pub model_discovery: ModelDiscovery,
    /// 顶层的 `display_name` / `description` / 端点 `label` 是中文; 英日文在这里,
    /// 由 [`Provider::localized`] 叠加。
    pub translations: ProviderTranslations,
    /// 厂商级的 URL 参数声明 (账户 ID / 网关 ID 等); 端点用到哪些见 `ProviderEndpoint::url_params_used`。
    #[serde(default)]
    pub url_params: Vec<UrlParam>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct UrlParam {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub placeholder: Option<String>,
    pub pattern: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct UrlParamText {
    pub label: String,
    #[serde(default)]
    pub placeholder: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ProviderEndpoint {
    pub id: String,
    pub label: String,
    pub base_url: String,
    #[serde(default = "default_protocol")]
    pub protocol: String,
    #[serde(default)]
    pub example_models: Vec<String>,
    /// 这个端点的 base_url 用到的参数 id, 按声明顺序 (后端算好的)。
    #[serde(default)]
    pub url_params_used: Vec<String>,
}

impl ProviderEndpoint {
    pub fn is_systemone(&self) -> bool {
        self.protocol == "systemone"
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ProviderTranslations {
    pub en: ProviderText,
    pub ja: ProviderText,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ProviderText {
    pub display_name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// key 是 endpoint id
    #[serde(default)]
    pub endpoints: BTreeMap<String, EndpointText>,
    /// key 是 url param id
    #[serde(default)]
    pub url_params: BTreeMap<String, UrlParamText>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct EndpointText {
    pub label: String,
}

/// 后端这个字段的键名是 `type` (`#[serde(rename = "type")]`), **不是** `auth_type`。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ProviderAuth {
    #[serde(rename = "type")]
    pub auth_type: String,
}

/// `Provider` 里的厂商模型发现配置。与本文件顶部 `Subscription::model_cache` 用的 [`ModelCache`]
/// 是两回事——这里是「这个厂商支不支持自动发现模型 / 手填提示」, 那边是「这条订阅缓存到的模型」。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ModelDiscovery {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub example_models: Vec<String>,
}

/// Picker label suffix. Protocol names, identical in every language.
pub fn capability_suffix(llm: bool, jev: bool) -> &'static str {
    match (llm, jev) {
        (true, true) => " [LLM] [Jev]",
        (true, false) => " [LLM]",
        (false, true) => " [Jev]",
        (false, false) => "",
    }
}

/// 需要去桌面端添加的 auth type。TUI 不做设备码流程, 这两类在厂商选择器里置灰。
pub const OAUTH_AUTH_TYPES: [&str; 2] = ["chatgpt_oauth", "kiro_oauth"];

impl Provider {
    /// 把上屏文字换成 `lang` 的版本。中文是原字段, 不用换。
    pub fn localized(mut self, lang: Lang) -> Self {
        let text = match lang {
            Lang::Zh => return self,
            Lang::En => self.translations.en.clone(),
            Lang::Ja => self.translations.ja.clone(),
        };
        self.display_name = text.display_name.clone();
        self.description = text.description.clone();
        for e in &mut self.endpoints {
            if let Some(t) = text.endpoints.get(&e.id) {
                e.label = t.label.clone();
            }
        }
        for u in &mut self.url_params {
            if let Some(t) = text.url_params.get(&u.id) {
                u.label = t.label.clone();
                u.placeholder = t.placeholder.clone();
            }
        }
        self
    }

    /// `(llm, jev)`: has a chat endpoint / has a System One endpoint. Same rule as the desktop
    /// `providerCapabilities.ts`; `tui_contract.rs` pins it against the backend.
    pub fn capabilities(&self) -> (bool, bool) {
        (
            self.endpoints.iter().any(|e| e.protocol == "messages"),
            self.endpoints.iter().any(ProviderEndpoint::is_systemone),
        )
    }

    pub fn is_oauth(&self) -> bool {
        OAUTH_AUTH_TYPES.contains(&self.auth.auth_type.as_str())
    }

    /// `default_endpoint` 指的那一个, 指不到就第一个; 空列表返回 `None`。
    pub fn default_endpoint(&self) -> Option<&ProviderEndpoint> {
        if let Some(id) = self.default_endpoint.as_deref() {
            if let Some(found) = self.endpoints.iter().find(|e| e.id == id) {
                return Some(found);
            }
        }
        self.endpoints.first()
    }
}

/// 自定义厂商的 5 种协议, 与后端 `commands::subscriptions::CustomProtocol` 一一对应
/// (`#[serde(rename_all = "snake_case")]`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomProtocol {
    Anthropic,
    Gemini,
    OpenaiResponses,
    OpenaiChatCompletions,
    GeminiInteractions,
}

impl CustomProtocol {
    pub const ALL: [CustomProtocol; 5] = [
        CustomProtocol::Anthropic,
        CustomProtocol::Gemini,
        CustomProtocol::OpenaiResponses,
        CustomProtocol::OpenaiChatCompletions,
        CustomProtocol::GeminiInteractions,
    ];

    /// 线上取值, 与后端 `#[serde(rename_all = "snake_case")]` 的输出逐字相同
    /// (`tui_contract.rs::custom_protocol_wire_names_round_trip` 锁住)。
    pub fn as_wire(self) -> &'static str {
        match self {
            CustomProtocol::Anthropic => "anthropic",
            CustomProtocol::Gemini => "gemini",
            CustomProtocol::OpenaiResponses => "openai_responses",
            CustomProtocol::OpenaiChatCompletions => "openai_chat_completions",
            CustomProtocol::GeminiInteractions => "gemini_interactions",
        }
    }

    /// 这个协议的预填值, 与桌面端 `LOCKED_CUSTOM_PRESETS` 同值——**除了 `Anthropic`**: 桌面端那张表
    /// 压根没有 `Anthropic` 这一项 (它走的是另一条 `AUTH_PRESETS` 下拉, base_url 表单一开始就是空的),
    /// 这里把它的 `base_url` 也定成空串 `""`: 会走这一项的人基本都在接中转站, 预填官方地址只会
    /// 让人先删掉 (表单为空时显示 [`CUSTOM_BASE_URL_PLACEHOLDER`] 当灰字提示)。
    pub fn preset(self) -> ProtocolPreset {
        match self {
            CustomProtocol::Anthropic => ProtocolPreset {
                base_url: "",
                messages_path: "/v1/messages",
                auth_header_name: "Authorization",
                auth_header_format: AuthHeaderFormat::Bearer,
            },
            CustomProtocol::Gemini => ProtocolPreset {
                base_url: "https://generativelanguage.googleapis.com",
                messages_path: "/v1beta/models/{model}:streamGenerateContent",
                auth_header_name: "x-goog-api-key",
                auth_header_format: AuthHeaderFormat::Raw,
            },
            CustomProtocol::GeminiInteractions => ProtocolPreset {
                base_url: "https://generativelanguage.googleapis.com",
                messages_path: "/v1beta/interactions",
                auth_header_name: "x-goog-api-key",
                auth_header_format: AuthHeaderFormat::Raw,
            },
            CustomProtocol::OpenaiResponses => ProtocolPreset {
                base_url: "https://api.openai.com",
                messages_path: "/v1/responses",
                auth_header_name: "Authorization",
                auth_header_format: AuthHeaderFormat::Bearer,
            },
            CustomProtocol::OpenaiChatCompletions => ProtocolPreset {
                base_url: "https://api.openai.com",
                messages_path: "/v1/chat/completions",
                auth_header_name: "Authorization",
                auth_header_format: AuthHeaderFormat::Bearer,
            },
        }
    }

    /// 只有 Anthropic 允许用户自己挑鉴权头, 其余四种锁定成 `preset()` 里那一对。
    pub fn auth_locked(self) -> bool {
        !matches!(self, CustomProtocol::Anthropic)
    }

    /// 只有 `Gemini` 要求 `messages_path` 含 `{model}`; `GeminiInteractions` 刻意不要求 (后端
    /// `commands/subscriptions.rs` 的校验也是这样分的)。
    pub fn requires_model_placeholder(self) -> bool {
        matches!(self, CustomProtocol::Gemini)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolPreset {
    pub base_url: &'static str,
    pub messages_path: &'static str,
    pub auth_header_name: &'static str,
    pub auth_header_format: AuthHeaderFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthHeaderFormat {
    Raw,
    Bearer,
}

impl AuthHeaderFormat {
    /// `"raw"` / `"bearer"`, 与后端 `AuthHeaderFormat::as_str()` 逐字相同。
    pub fn as_wire(self) -> &'static str {
        match self {
            AuthHeaderFormat::Raw => "raw",
            AuthHeaderFormat::Bearer => "bearer",
        }
    }
}

/// Anthropic 自定义厂商可选的两种鉴权组合, 与桌面端 `AUTH_PRESETS` 同值同序。
pub const ANTHROPIC_AUTH_PRESETS: [(&str, AuthHeaderFormat); 2] =
    [("Authorization", AuthHeaderFormat::Bearer), ("x-api-key", AuthHeaderFormat::Raw)];

/// 自定义表单 Base URL 为空时的灰字提示。语言无关, 所以放这里而不是 `Strings`。目前只有
/// `Anthropic` 的预设 `base_url` 是空的 (会走这一项的人基本都在接中转站, 预填官方地址只会让人先
/// 删掉), 但用户把任何协议的 Base URL 清空后同样显示它。
pub const CUSTOM_BASE_URL_PLACEHOLDER: &str = "https://api.example.com";

/// `create_subscription` 的入参。**不实现 `Serialize`**: `api_key` 是 [`Secret`], 线上 JSON 由
/// [`CreateInput::to_args`] 手写, 明文只在那一处出现 (`secret.rs` 的白名单扫描测试盯着这一点)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateInput {
    pub display_name: String,
    pub api_key: Secret,
    pub model_slots: ModelSlots,
    pub source: CreateSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateSource {
    Builtin { provider_id: String, endpoint_id: String, url_params: BTreeMap<String, String> },
    Custom(Box<CustomSource>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomSource {
    pub provider_display_name: String,
    pub base_url: String,
    pub messages_path: String,
    pub auth_header_name: String,
    pub auth_header_format: AuthHeaderFormat,
    pub protocol: CustomProtocol,
    /// 探测成功、且此后 `base_url` 一个字都没改过时才是 `Some` (规则由向导表单维护)。
    pub models_url: Option<String>,
}

impl CreateSource {
    /// `{"kind": "from_template" | "custom", …}` (后端内部标签枚举 `CreateSource`)。内层字段全是
    /// snake_case——`web_commands!` 只把最外层的参数名转成 camelCase, 后端的 `CreateSource` 自己
    /// 没有 `rename_all`。
    fn to_args(&self) -> serde_json::Value {
        match self {
            CreateSource::Builtin { provider_id, endpoint_id, url_params } => {
                let mut obj = serde_json::json!({ "kind": "from_template", "provider_id": provider_id, "endpoint_id": endpoint_id });
                // 空的不发: 没有参数的厂商保持原有的线上形状。
                if !url_params.is_empty() {
                    obj["url_params"] = serde_json::json!(url_params);
                }
                obj
            }
            CreateSource::Custom(custom) => {
                let mut obj = serde_json::json!({
                    "kind": "custom",
                    "provider_display_name": custom.provider_display_name,
                    "base_url": custom.base_url,
                    "messages_path": custom.messages_path,
                    "auth_header_name": custom.auth_header_name,
                    "auth_header_format": custom.auth_header_format.as_wire(),
                    "protocol": custom.protocol.as_wire(),
                });
                // `models_url` 为 `None` 时整个键不出现——不是发 `null` (后端字段是
                // `#[serde(default)] Option<String>`, 缺失与 `null` 反序列化结果相同, 但「不出现」
                // 是契约测试 `create_subscription_input_matches` 盯着的具体形状)。
                if let Some(url) = &custom.models_url {
                    obj["models_url"] = serde_json::Value::String(url.clone());
                }
                obj
            }
        }
    }
}

impl CreateInput {
    /// `{"input": {…}}`。**内层字段全是 snake_case**——`web_commands!` 只把最外层的参数名转成
    /// camelCase, `CreateSubscriptionInput` 自己没有 `rename_all` (与 `RequestQuery::to_args`
    /// 里 `pageSize` 与 `filters.subscription_id` 并存同一条规律)。`api_key` 在这里 `.expose()`
    /// 是它唯一允许出现明文的位置 (`secret.rs` 的白名单扫描测试盯着 `client/dto.rs` 这一处)。
    pub fn to_args(&self) -> serde_json::Value {
        serde_json::json!({
            "input": {
                "display_name": self.display_name,
                "api_key": self.api_key.expose(),
                "model_slots": self.model_slots,
                "source": self.source.to_args(),
            }
        })
    }
}

/// `probe_custom_models` 的入参。注意它**没有** `messages_path`: 后端按协议推导 `/v1/models` 或
/// `/v1beta/models`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeInput {
    pub base_url: String,
    pub auth_header_name: String,
    pub auth_header_format: AuthHeaderFormat,
    pub api_key: Secret,
    pub protocol: CustomProtocol,
}

impl ProbeInput {
    /// `{"input": {…}}`, 内层 snake_case。`protocol` **总是**发 (哪怕是 Anthropic), 与桌面端
    /// 「只有锁定预设才发」的写法不同但结果一样——后端的缺省值就是 `anthropic`。
    pub fn to_args(&self) -> serde_json::Value {
        serde_json::json!({
            "input": {
                "base_url": self.base_url,
                "auth_header_name": self.auth_header_name,
                "auth_header_format": self.auth_header_format.as_wire(),
                "api_key": self.api_key.expose(),
                "protocol": self.protocol.as_wire(),
            }
        })
    }
}

/// `create_subscription` 的返回值, 只取 id (与桌面端一样)。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CreatedSubscription {
    pub id: String,
}

/// `probe_custom_models` 的返回。形状与 [`RefreshModelsResult`] 相近, 多一个 `models_url`。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProbeModelsResult {
    Auto { models: Vec<ModelInfo>, models_url: String },
    ManualFallback { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_source_sends_url_params_only_when_present() {
        let without = CreateSource::Builtin { provider_id: "p".into(), endpoint_id: "e".into(), url_params: BTreeMap::new() }.to_args();
        assert!(without.get("url_params").is_none(), "keep the old wire shape when empty");
        let mut params = BTreeMap::new();
        params.insert("account_id".to_string(), "abc".to_string());
        let with = CreateSource::Builtin { provider_id: "p".into(), endpoint_id: "e".into(), url_params: params }.to_args();
        assert_eq!(with["url_params"]["account_id"], "abc");
    }

    #[test]
    fn capabilities_follow_endpoint_protocols() {
        let ep = |id: &str, protocol: &str| ProviderEndpoint {
            id: id.into(),
            label: id.into(),
            base_url: "u".into(),
            protocol: protocol.into(),
            example_models: vec![],
            url_params_used: vec![],
        };
        let text = ProviderText { display_name: "p".into(), description: None, endpoints: Default::default(), url_params: Default::default() };
        let mut p = Provider {
            id: "p".into(),
            display_name: "p".into(),
            description: None,
            endpoints: vec![ep("a", "messages"), ep("b", "systemone")],
            default_endpoint: None,
            auth: ProviderAuth { auth_type: "api_key".into() },
            model_discovery: ModelDiscovery { enabled: true, example_models: vec![] },
            translations: ProviderTranslations { en: text.clone(), ja: text },
            url_params: vec![],
        };
        assert_eq!(p.capabilities(), (true, true));
        p.endpoints.truncate(1);
        assert_eq!(p.capabilities(), (true, false));
        // Same literal rule as the desktop `providerCapabilities.ts`: an unknown future protocol
        // is neither LLM nor Jev on both sides.
        p.endpoints = vec![ep("c", "some_future_protocol")];
        assert_eq!(p.capabilities(), (false, false));
        assert_eq!(capability_suffix(true, true), " [LLM] [Jev]");
        assert_eq!(capability_suffix(false, true), " [Jev]");
    }

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

    /// 只含必填字段的 JSON (后端老数据 / 大部分 Option 列为 NULL 时的真实形状) 应该解析成功;
    /// 未来的状态值 (`brand_new`) 应该落到 `Unknown`, 不该让整页解析失败。
    #[test]
    fn request_log_tolerates_missing_optionals_and_unknown_status() {
        let log: RequestLog = serde_json::from_str(
            r#"{"id":"1","timestamp":1700000000000,"virtual_model_name":"model-sonnet",
                "subscription_id":"s1","provider_id":"zhipu","endpoint_id":"default",
                "real_model_name":"glm-4.6","is_streaming":false,"status":"brand_new"}"#,
        )
        .unwrap();
        assert_eq!(log.id, "1");
        assert_eq!(log.timestamp, 1_700_000_000_000);
        assert_eq!(log.virtual_model_name, "model-sonnet");
        assert_eq!(log.subscription_id, "s1");
        assert_eq!(log.provider_id, "zhipu");
        assert_eq!(log.endpoint_id, "default");
        assert_eq!(log.real_model_name, "glm-4.6");
        assert!(!log.is_streaming);
        assert_eq!(log.status, RequestStatus::Unknown);
        assert_eq!(log.response_model_name, None);
        assert_eq!(log.http_status, None);
        assert_eq!(log.total_latency_ms, None);
        assert_eq!(log.input_tokens, None);
        assert_eq!(log.output_tokens, None);
        assert_eq!(log.cache_creation_tokens, None);
        assert_eq!(log.cache_read_tokens, None);
        assert_eq!(log.error_message, None);
        assert_eq!(log.upstream_response_body, None);
        assert_eq!(log.client_tool, None);
        assert_eq!(log.client_user_agent, None);
        assert_eq!(log.client_version, None);
        assert_eq!(log.client_ip, None);
        assert_eq!(log.entry_kind, None);
        assert_eq!(log.downstream_http_version, None);
        assert_eq!(log.client_effort, None);
        assert_eq!(log.effective_effort, None);
        assert_eq!(log.effort_source, None);
        assert_eq!(log.upstream_effort, None);
        assert_eq!(log.stop_reason, None);
        assert_eq!(log.tools_offered_count, None);
        assert_eq!(log.tool_result_count, None);
        assert_eq!(log.tool_use_count, None);
        assert_eq!(log.tool_use_names, None);
    }

    /// 默认查询与带全部三个过滤条件的查询, `to_args()` 精确等于文档里的例子——`pageSize` 驼峰、
    /// `filters` 蛇形、`filters` 恒为对象 (可能是 `{}`)。
    #[test]
    fn request_query_args_shape() {
        assert_eq!(RequestQuery::default().to_args(), serde_json::json!({"page": 1, "pageSize": 50, "filters": {}}));

        let full = RequestQuery {
            page: 2,
            filters: RequestFilters {
                subscription_id: Some("s1".into()),
                virtual_model_name: Some("model-sonnet".into()),
                status: Some(RequestStatus::Error),
            },
        };
        assert_eq!(
            full.to_args(),
            serde_json::json!({
                "page": 2,
                "pageSize": 50,
                "filters": {"subscription_id": "s1", "virtual_model_name": "model-sonnet", "status": "error"},
            })
        );
    }

    fn builtin_input(api_key: &str) -> CreateInput {
        CreateInput {
            display_name: "智谱主号".into(),
            api_key: Secret::new(api_key),
            model_slots: ModelSlots { fable: "f".into(), opus: "o".into(), sonnet: "s".into(), haiku: "h".into(), fallback: String::new(), jev: String::new() },
            source: CreateSource::Builtin { provider_id: "zhipu".into(), endpoint_id: "default".into(), url_params: BTreeMap::new() },
        }
    }

    fn custom_source(models_url: Option<&str>) -> CustomSource {
        CustomSource {
            provider_display_name: "中转站".into(),
            base_url: "https://relay.example.com".into(),
            messages_path: "/v1/messages".into(),
            auth_header_name: "Authorization".into(),
            auth_header_format: AuthHeaderFormat::Bearer,
            protocol: CustomProtocol::Anthropic,
            models_url: models_url.map(str::to_string),
        }
    }

    /// `to_args()["input"]` 的每一层键都是 snake_case——`web_commands!` 只把最外层参数名转
    /// camelCase, `CreateSubscriptionInput`/`CreateSource` 自己没有 `rename_all`。
    #[test]
    fn create_input_to_args_is_snake_case_inside_input() {
        let args = builtin_input("sk-test").to_args();
        assert_eq!(args["input"]["display_name"], "智谱主号");
        assert_eq!(args["input"]["api_key"], "sk-test");
        assert_eq!(args["input"]["model_slots"]["fable"], "f");
        assert_eq!(args["input"]["model_slots"]["fallback"], "", "fallback 应该总是出现, 不是被省略");
        assert_eq!(args["input"]["source"]["kind"], "from_template");
        assert_eq!(args["input"]["source"]["provider_id"], "zhipu");
        assert_eq!(args["input"]["source"]["endpoint_id"], "default");
    }

    #[test]
    fn create_input_omits_models_url_when_absent() {
        let absent = CreateInput {
            display_name: "自定义".into(),
            api_key: Secret::new("sk-test"),
            model_slots: ModelSlots::pending(),
            source: CreateSource::Custom(Box::new(custom_source(None))),
        };
        let args = absent.to_args();
        let source = args["input"]["source"].as_object().expect("source 应该是个对象");
        assert_eq!(source["kind"], "custom");
        assert_eq!(source["provider_display_name"], "中转站");
        assert_eq!(source["base_url"], "https://relay.example.com");
        assert_eq!(source["messages_path"], "/v1/messages");
        assert_eq!(source["auth_header_name"], "Authorization");
        assert_eq!(source["auth_header_format"], "bearer");
        assert_eq!(source["protocol"], "anthropic");
        assert!(!source.contains_key("models_url"), "models_url 为 None 时整个键不该出现: {source:?}");

        let present = CreateInput {
            source: CreateSource::Custom(Box::new(custom_source(Some("https://relay.example.com/v1/models")))),
            ..absent
        };
        let args = present.to_args();
        assert_eq!(args["input"]["source"]["models_url"], "https://relay.example.com/v1/models");
    }

    #[test]
    fn the_key_is_masked_in_debug_but_present_in_the_args() {
        let input = builtin_input("sk-super-secret");
        let debug = format!("{input:?}");
        assert!(!debug.contains("sk-super-secret"), "{debug}");
        assert!(debug.contains("15 chars"), "{debug}"); // "sk-super-secret" 长度 15, 验证走的是 Secret::Debug

        let args = input.to_args();
        assert_eq!(args["input"]["api_key"], "sk-super-secret", "to_args() 是明文唯一该出现的地方");
    }

    #[test]
    fn probe_input_to_args_always_sends_the_protocol() {
        let input = ProbeInput {
            base_url: "https://relay.example.com".into(),
            auth_header_name: "Authorization".into(),
            auth_header_format: AuthHeaderFormat::Bearer,
            api_key: Secret::new("sk-test"),
            protocol: CustomProtocol::Anthropic,
        };
        let args = input.to_args();
        assert_eq!(args["input"]["base_url"], "https://relay.example.com");
        assert_eq!(args["input"]["auth_header_name"], "Authorization");
        assert_eq!(args["input"]["auth_header_format"], "bearer");
        assert_eq!(args["input"]["api_key"], "sk-test");
        // Anthropic 是「默认值」, 但仍然显式发出——桌面端只有锁定预设才发, 这里刻意总是发。
        assert_eq!(args["input"]["protocol"], "anthropic");
    }

    #[test]
    fn every_protocol_has_a_distinct_wire_name_and_preset() {
        let wires: Vec<&str> = CustomProtocol::ALL.iter().map(|p| p.as_wire()).collect();
        let mut sorted = wires.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), wires.len(), "线上取值应该互不相同: {wires:?}");

        for p in CustomProtocol::ALL {
            assert_eq!(p.auth_locked(), p != CustomProtocol::Anthropic, "{p:?}: 只有 Anthropic 允许自选鉴权头");
            assert_eq!(p.requires_model_placeholder(), p == CustomProtocol::Gemini, "{p:?}: 只有 Gemini 要求 {{model}} 占位符");

            let preset = p.preset();
            if p == CustomProtocol::Anthropic {
                assert_eq!(preset.base_url, "", "Anthropic 的预设 base_url 应该留空 (见 preset() 的文档)");
            } else {
                assert!(preset.base_url.starts_with("https://"), "{p:?}: {}", preset.base_url);
            }
            assert!(preset.messages_path.starts_with('/'), "{p:?}: {} 应该以 / 开头 (后端 validate 的要求)", preset.messages_path);
        }
        assert!(
            CustomProtocol::Gemini.preset().messages_path.contains("{model}"),
            "Gemini 的 messages_path 应该含 {{model}} 占位符"
        );
    }

    #[test]
    fn pending_slots_leave_the_fallback_empty() {
        let slots = ModelSlots::pending();
        assert_eq!(slots.fable, PENDING_MODEL);
        assert_eq!(slots.opus, PENDING_MODEL);
        assert_eq!(slots.sonnet, PENDING_MODEL);
        assert_eq!(slots.haiku, PENDING_MODEL);
        assert_eq!(slots.fallback, "", "兜底槽应该留空串, 不是占位值");
    }
}
