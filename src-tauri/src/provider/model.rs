use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Compatibility {
    Verified,
    Partial,
    Untested,
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
}

impl AuthType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::ChatgptOauth => "chatgpt_oauth",
            Self::KiroOauth => "kiro_oauth",
            Self::GeminiApiKey => "gemini_api_key",
            Self::OpenaiResponsesApiKey => "openai_responses_api_key",
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderEndpoint {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
    pub base_url: String,
    pub messages_path: String,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub billing: Option<String>,
}

impl ProviderEndpoint {
    pub fn messages_url(&self) -> String {
        join_base_path(&self.base_url, &self.messages_path)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    pub display_name: String,
    #[serde(default)]
    pub description: Option<String>,
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
    pub compatibility_notes: Option<String>,

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
}
