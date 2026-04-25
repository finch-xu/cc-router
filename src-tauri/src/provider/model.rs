use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Compatibility {
    Verified,
    Partial,
    Untested,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthType {
    ApiKey,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthHeaderFormat {
    Raw,
    Bearer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Auth {
    #[serde(rename = "type")]
    pub auth_type: AuthType,
    pub header_name: String,
    pub header_format: AuthHeaderFormat,
}

impl Auth {
    /// 拼出该 provider 鉴权 header 应填的值。Bearer 加前缀, Raw 原样。
    pub fn header_value(&self, api_key: &str) -> String {
        match self.header_format {
            AuthHeaderFormat::Bearer => format!("Bearer {api_key}"),
            AuthHeaderFormat::Raw => api_key.to_string(),
        }
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
    /// 拼出完整 messages 接口 URL: `base_url + messages_path`, 处理斜杠规范化。
    pub fn messages_url(&self) -> String {
        format!(
            "{}{}",
            self.base_url.trim_end_matches('/'),
            if self.messages_path.starts_with('/') {
                self.messages_path.clone()
            } else {
                format!("/{}", self.messages_path)
            }
        )
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
}

impl Provider {
    pub fn endpoint(&self, id: &str) -> Option<&ProviderEndpoint> {
        self.endpoints.iter().find(|e| e.id == id)
    }
}
