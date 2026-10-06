use std::fmt;
use std::str::FromStr;

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

/// yaml 里上屏的文字 (厂商名 / 描述 / 端点名 ...), 三种界面语言各一份。
///
/// yaml 有两种写法: 纯字符串 = 三语相同 (`DeepSeek` 这类品牌名); `{zh, en, ja}` 对象 = 各写一份,
/// 缺任何一个键都解析失败 —— 界面上没有回退逻辑, 三语必须齐全。含中文的字段不许用纯字符串写法,
/// 由 `loader.rs::tests::cjk_text_is_translated` 锁住。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalizedText {
    pub zh: String,
    pub en: String,
    pub ja: String,
}

impl LocalizedText {
    pub fn same(text: impl Into<String>) -> Self {
        let text = text.into();
        Self { zh: text.clone(), en: text.clone(), ja: text }
    }
}

impl<'de> Deserialize<'de> for LocalizedText {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct PerLang {
            zh: String,
            en: String,
            ja: String,
        }

        // 手写 visitor 而不是 `#[serde(untagged)]`: untagged 失败时只报 "did not match any variant",
        // 这里能直接报出 "missing field `ja`" / "unknown field `jp`"。
        struct TextVisitor;
        impl<'de> Visitor<'de> for TextVisitor {
            type Value = LocalizedText;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a string, or a map with zh / en / ja")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(LocalizedText::same(v))
            }
            fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<Self::Value, M::Error> {
                let p = PerLang::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(LocalizedText { zh: p.zh, en: p.en, ja: p.ja })
            }
        }
        deserializer.deserialize_any(TextVisitor)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Compatibility {
    Verified,
    Partial,
    Untested,
}

/// 用于 UI 在 provider 下拉里分组展示. 不影响调度/翻译, 纯展示层语义。
/// 默认 `FirstParty` 让现有 yaml 无需改动也能继续加载 (serde default).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCategory {
    /// 大模型原厂 (Anthropic / OpenAI / DeepSeek / 智谱 / Moonshot 等).
    #[default]
    FirstParty,
    /// AI 应用订阅 (Kiro / Cursor / Copilot 等). 不卖按 token 计费的模型 API,
    /// 而是基于他家(或自家)大模型做上层产品对终端用户卖订阅, 通常走 OAuth 接入.
    SecondParty,
    /// API 分发 / 聚合站 (OpenRouter / Requesty / aiberm 等). 不直接提供模型推理,
    /// 而是把多家原厂模型聚合成统一 API.
    Aggregator,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthType {
    ApiKey,
    /// ChatGPT Plus/Pro 订阅 OAuth: 用户在 cc-router UI 完成 Device Code 登录,
    /// pipeline 在每次请求前从 OAuth manager 取实时 access_token, 不读 subscriptions.api_key.
    ChatgptOauth,
    /// Kiro IDE / AWS Builder ID OAuth: 凭据来自 Kiro IDE 落盘 JSON 或 AWS SSO OIDC Device Flow,
    /// 上游协议为 AWS CodeWhisperer Streaming RPC (二进制 Event Stream), 需协议翻译.
    /// pipeline 按 oauth_metadata.auth_method 走 social (kiro 桌面) 或 idc (AWS OIDC) refresh 分支.
    KiroOauth,
    /// Google AI Studio (Gemini): 用户输入 Google API key (x-goog-api-key header),
    /// 上游协议为 Gemini generateContent / streamGenerateContent, 需协议翻译
    /// (Anthropic Messages ↔ Gemini contents/parts). model 嵌在 URL 路径里,
    /// dispatch 层做 `{model}` 占位符替换 + 强制 `?alt=sse`.
    GeminiApiKey,
    /// OpenAI Responses API key: 普通 `sk-...` 形式 (Bearer header), 上游协议为
    /// OpenAI `/v1/responses` (官方 api.openai.com 或 OpenAI 兼容中转/网关), 需协议翻译
    /// (Anthropic Messages ↔ OpenAI Responses). 客户端 stream 决定上游 stream;
    /// 支持 reasoning 双向 (thinking ↔ reasoning encrypted_content 多轮回灌)。
    OpenaiResponsesApiKey,
    /// OpenAI Chat Completions API key: 普通 `sk-...` 形式 (Bearer header), 上游协议为
    /// OpenAI `/v1/chat/completions` (官方 api.openai.com 或 DeepSeek/Together/Groq/Ollama/
    /// 各类 one-api/new-api 中转), 需协议翻译 (Anthropic Messages ↔ OpenAI Chat Completions).
    /// 客户端 stream 决定上游 stream; 上游 `reasoning_content` (DeepSeek R1 风格) 单向暴露为
    /// Anthropic thinking content_block (Phase 1 不做多轮回灌, 由 Phase 2 决定).
    OpenaiChatCompletionsApiKey,
    /// Google Gemini Interactions API (新统一接口): 用户输入 Google API key (x-goog-api-key header),
    /// 上游协议为 `/v1beta/interactions` (与旧 generateContent 完全不同), 需协议翻译
    /// (Anthropic Messages ↔ Interactions step_list). model 在 **body** 里 (不像 generateContent 嵌 URL),
    /// dispatch 层 URL 固定 + 强制 `?alt=sse`. 支持 thinking 双向 (thought signature 多轮回灌)。
    GeminiInteractionsApiKey,
}

impl AuthType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::ChatgptOauth => "chatgpt_oauth",
            Self::KiroOauth => "kiro_oauth",
            Self::GeminiApiKey => "gemini_api_key",
            Self::OpenaiResponsesApiKey => "openai_responses_api_key",
            Self::OpenaiChatCompletionsApiKey => "openai_chat_completions_api_key",
            Self::GeminiInteractionsApiKey => "gemini_interactions_api_key",
        }
    }
}

impl FromStr for AuthType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "api_key" => Ok(Self::ApiKey),
            "chatgpt_oauth" => Ok(Self::ChatgptOauth),
            "kiro_oauth" => Ok(Self::KiroOauth),
            "gemini_api_key" => Ok(Self::GeminiApiKey),
            "openai_responses_api_key" => Ok(Self::OpenaiResponsesApiKey),
            "openai_chat_completions_api_key" => Ok(Self::OpenaiChatCompletionsApiKey),
            "gemini_interactions_api_key" => Ok(Self::GeminiInteractionsApiKey),
            other => Err(format!("无效 auth_type: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthHeaderFormat {
    Raw,
    Bearer,
}

impl AuthHeaderFormat {
    /// 把 api_key 包装成 header 值。Bearer 加前缀, Raw 原样。
    pub fn apply(&self, api_key: &str) -> String {
        match self {
            Self::Bearer => format!("Bearer {api_key}"),
            Self::Raw => api_key.to_string(),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Bearer => "bearer",
            Self::Raw => "raw",
        }
    }
}

impl FromStr for AuthHeaderFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "bearer" => Ok(Self::Bearer),
            "raw" => Ok(Self::Raw),
            other => Err(format!("无效 auth_header_format: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Auth {
    #[serde(rename = "type")]
    pub auth_type: AuthType,
    pub header_name: String,
    pub header_format: AuthHeaderFormat,
}

impl Auth {
    pub fn header_value(&self, api_key: &str) -> String {
        self.header_format.apply(api_key)
    }
}

/// 拼接 base + path, 处理首尾斜杠规范化。被 endpoint URL 和 model_discovery URL 共用。
pub fn join_base_path(base: &str, path: &str) -> String {
    let trimmed_base = base.trim_end_matches('/');
    if path.starts_with('/') {
        format!("{trimmed_base}{path}")
    } else {
        format!("{trimmed_base}/{path}")
    }
}

/// 端点服务哪一族入站请求。缺省 `Messages` = 对话类 (三个对话入口共用的 pipeline);
/// `Systemone` = Jev 决策协议, 只服务 `POST /v1/systemone`, 请求原样透传、不翻译。
/// 订阅创建时快照进 `subscriptions.endpoint_protocol`, 之后不可变。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EndpointProtocol {
    #[default]
    Messages,
    Systemone,
}

impl EndpointProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Messages => "messages",
            Self::Systemone => "systemone",
        }
    }
}

impl FromStr for EndpointProtocol {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "messages" => Ok(Self::Messages),
            "systemone" => Ok(Self::Systemone),
            other => Err(format!("无效 endpoint_protocol: {other}")),
        }
    }
}

/// Upstream dialect of a System One endpoint. `standard` = TypeSafe shape at `/v1/systemone`;
/// `cloudflare_run` = Workers AI `/run/@cf/cloudflare/{model}` with the `{result, success, errors}`
/// envelope (spec §2.1). Only meaningful when `protocol: systemone` (loader enforces).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SystemoneWire {
    #[default]
    Standard,
    CloudflareRun,
}

impl SystemoneWire {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::CloudflareRun => "cloudflare_run",
        }
    }
}

impl FromStr for SystemoneWire {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "standard" => Ok(Self::Standard),
            "cloudflare_run" => Ok(Self::CloudflareRun),
            other => Err(format!("无效 systemone_wire: {other}")),
        }
    }
}

/// A value the user fills in when creating a subscription; referenced as `{id}` in yaml strings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlParam {
    pub id: String,
    pub label: LocalizedText,
    #[serde(default)]
    pub placeholder: Option<LocalizedText>,
    /// Regex the trimmed value must match (validated on create / edit, and in the UI).
    pub pattern: String,
}

/// Response shape of a provider's model list endpoint. `None` in yaml = choose by auth type.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelsEnvelopeKind {
    Openai,
    Gemini,
    Cloudflare,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderEndpoint {
    pub id: String,
    pub label: LocalizedText,
    #[serde(default)]
    pub description: Option<LocalizedText>,
    pub base_url: String,
    pub messages_path: String,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub billing: Option<String>,
    /// 缺省 messages。systemone 只允许出现在 `auth.type = api_key` 的 provider 下 (loader 校验)。
    #[serde(default)]
    pub protocol: EndpointProtocol,
    /// 仅覆盖本端点的模型输入提示; 空 = 用 provider 级 `model_discovery.example_models`。
    /// 创建订阅时非空则写进订阅快照的 `model_discovery.example_models`。
    #[serde(default)]
    pub example_models: Vec<String>,
    /// Endpoint-level extra headers, merged over provider `required_headers` when snapshotting
    /// (endpoint wins). Values may use url_params and `{api_key}`.
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub systemone_wire: SystemoneWire,
}

impl ProviderEndpoint {
    pub fn messages_url(&self) -> String {
        join_base_path(&self.base_url, &self.messages_path)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDiscovery {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_models_path")]
    pub path: String,
    /// 可选完整 URL；如果提供则优先于 `endpoint.base_url + path` 拼接。
    /// 用于 provider 的 models 接口与 messages 接口不同域（例如 DeepSeek、智谱）。
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default = "default_cache_ttl")]
    pub cache_ttl_hours: u32,
    #[serde(default)]
    pub example_models: Vec<String>,
    /// Response shape of the model list. `None` = choose by auth type.
    #[serde(default)]
    pub envelope: Option<ModelsEnvelopeKind>,
}

/// 手写而不是 derive: derive 给出的是 `path: ""` / `cache_ttl_hours: 0`, 与上面的 serde 默认值
/// 不一致 —— `..ModelDiscovery::default()` 曾因此把空 path 写进自定义订阅的 snapshot
/// (issue #44 排查时发现, 老数据由 `model_discovery::ProbeTarget::from_row` 兜底)。
impl Default for ModelDiscovery {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            path: default_models_path(),
            url: None,
            cache_ttl_hours: default_cache_ttl(),
            example_models: Vec::new(),
            envelope: None,
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_models_path() -> String {
    "/v1/models".to_string()
}
fn default_cache_ttl() -> u32 {
    24
}

/// 订阅余额/套餐余量查询配置 (provider yaml `balance_discovery` 字段).
///
/// 设计原则与 `ModelDiscovery` 一致: yaml 声明 endpoint + parser 名字,
/// 真正的响应解析硬编码在 `subscription::balance_discovery::parse_<parser>` 里,
/// 各 provider 响应字段差异太大 (DeepSeek 多币种数组 / Minimax token 配额),
/// 声明式表达不够灵活, 所以保留 dispatch in Rust 的范式.
///
/// Provider 不声明此字段时, `Provider.balance_discovery = None`, UI 不显示余额卡片.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceDiscovery {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Full URL; balance endpoint often lives on a different domain than messages
    /// (DeepSeek: balance at `api.deepseek.com/user/balance`, messages at
    /// `.com/anthropic/v1/messages`), so we keep the full URL instead of base+path.
    pub url: String,
    #[serde(default)]
    pub method: BalanceHttpMethod,
    pub parser: BalanceParser,
    #[serde(default = "default_balance_cache_ttl")]
    pub cache_ttl_minutes: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum BalanceHttpMethod {
    #[default]
    Get,
    Post,
}

impl BalanceHttpMethod {
    pub fn as_reqwest(self) -> reqwest::Method {
        match self {
            Self::Get => reqwest::Method::GET,
            Self::Post => reqwest::Method::POST,
        }
    }
}

/// Known parsers. yaml load fails (via serde) if a provider declares an unknown
/// parser — moves the error from runtime "refresh balance" click to app startup.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BalanceParser {
    Deepseek,
    Openrouter,
}

fn default_balance_cache_ttl() -> u32 {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub display_name: LocalizedText,
    #[serde(default)]
    pub description: Option<LocalizedText>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub docs_url: Option<String>,
    #[serde(default)]
    pub api_key_url: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,

    pub compatibility: Compatibility,
    #[serde(default)]
    pub compatibility_notes: Option<LocalizedText>,

    /// UI 分组展示用 (大模型原厂 / API 分发站). 默认 first_party.
    #[serde(default)]
    pub category: ProviderCategory,

    /// User-supplied values referenced as `{id}` in endpoint / discovery strings (spec §3.2).
    #[serde(default)]
    pub url_params: Vec<UrlParam>,

    pub endpoints: Vec<ProviderEndpoint>,
    #[serde(default)]
    pub default_endpoint: Option<String>,

    pub auth: Auth,

    #[serde(default)]
    pub required_headers: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub forward_headers: Vec<String>,

    #[serde(default)]
    pub model_discovery: ModelDiscovery,

    /// 余额/套餐余量查询配置 (可选). 大多数 provider 不声明此字段, UI 不显示余额卡片.
    /// 声明的 provider 由 `subscription::balance_discovery` 模块按 `parser` 字段分发解析.
    #[serde(default)]
    pub balance_discovery: Option<BalanceDiscovery>,

    /// OpenAI Responses 翻译路径专用 (auth_type=OpenaiResponsesApiKey / ChatgptOauth):
    /// 是否在响应翻译时把 reasoning 内容暴露成 Anthropic thinking content_block。
    /// codex 默认 false (向后兼容, opt-in), openai 官方 yaml 默认 true. 其他 provider 忽略此字段。
    #[serde(default)]
    pub expose_reasoning: bool,

    /// OpenAI Responses 翻译路径专用: reasoning_effort 默认值 (minimal/low/medium/high).
    /// 客户端 body 未指定 + 订阅级未设置时, 使用此值. 空字符串视为「不传」。
    #[serde(default)]
    pub default_reasoning_effort: Option<String>,

    /// Anthropic 协议透传 provider 专用: 进入 dispatch 时是否给缺 thinking content_block 的
    /// `role: assistant` 消息插入空 placeholder `{type:"thinking", thinking:"", signature:""}`.
    ///
    /// 默认 false (不动). 设 true 用于 DeepSeek 这类要求"每个含 tool_use 的 assistant 消息
    /// 必须有 thinking block"的兼容子集 — 多 provider 轮询时由 GLM/anthropic 等不发 thinking
    /// 的 provider 生成的 assistant 消息回灌到 DeepSeek 时会触发 400 "thinking must be passed
    /// back to the API", 插入空 placeholder 后 DeepSeek 接受 (curl 实测确认)。
    ///
    /// 本字段保留用户已有的 thinking 上下文不变, 只对缺 thinking 的 assistant 消息补 placeholder,
    /// 因此 tool_use 推理质量不受影响 (DeepSeek 官方说 tool_use 场景 thinking 上下文必需)。
    #[serde(default)]
    pub inject_missing_thinking_placeholder: bool,
}

impl Provider {
    pub fn endpoint(&self, id: &str) -> Option<&ProviderEndpoint> {
        self.endpoints.iter().find(|e| e.id == id)
    }

    pub fn url_param(&self, id: &str) -> Option<&UrlParam> {
        self.url_params.iter().find(|p| p.id == id)
    }

    /// Declared params referenced by this endpoint (or provider-level strings it inherits),
    /// in declaration order.
    pub fn params_used(&self, endpoint: &ProviderEndpoint) -> Vec<String> {
        use crate::provider::url_template::placeholders;
        let mut found: Vec<String> = Vec::new();
        let mut scan = |s: &str| found.extend(placeholders(s));
        scan(&endpoint.base_url);
        scan(&endpoint.messages_path);
        endpoint.headers.values().for_each(|v| scan(v));
        self.required_headers.values().for_each(|v| scan(v));
        if let Some(u) = self.model_discovery.url.as_deref() {
            scan(u);
        }
        self.url_params.iter().filter(|p| found.contains(&p.id)).map(|p| p.id.clone()).collect()
    }
}
