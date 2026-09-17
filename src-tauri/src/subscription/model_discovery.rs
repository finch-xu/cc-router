//! 调用 `{base_url}/v1/models` 并缓存结果（设计稿 §8）。
//!
//! snapshot 模型: 全部连接信息从订阅 row 自身字段读, 不再回查 state.providers。
//!
//! Envelope 解析按 `auth_type` 分发:
//! - `ApiKey` / `ChatgptOauth` / `KiroOauth` → OpenAI 风格 `{data:[{id,model,display_name}]}`
//! - `GeminiApiKey` / `GeminiInteractionsApiKey` → Gemini 风格 `{models:[{name:"models/gemini-...", displayName, supportedGenerationMethods}]}`

use std::collections::BTreeMap;

use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;
use sqlx::SqlitePool;
use tracing::{info, warn};

use crate::error::AppError;
use crate::provider::model::{join_base_path, AuthType};
use crate::subscription::{
    model::{ModelCache, ModelInfo, SubscriptionRow},
    store,
};

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("network: {0}")]
    Network(#[from] reqwest::Error),
    #[error("http {0}")]
    Http(u16),
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    #[error("app error: {0}")]
    App(#[from] AppError),
}

impl From<serde_json::Error> for DiscoveryError {
    fn from(e: serde_json::Error) -> Self {
        Self::InvalidResponse(e.to_string())
    }
}

#[derive(Debug, Deserialize)]
struct ModelsEnvelope {
    #[serde(default)]
    data: Vec<ModelsItem>,
}

#[derive(Debug, Deserialize)]
struct ModelsItem {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
}

/// 一次发现成功的结果. `url` 是实际打通的完整地址 —— 自定义订阅靠候选探测得到,
/// 调用方把它写回 snapshot 的 `model_discovery.url`, 之后刷新就不必再猜。
#[derive(Debug, Clone)]
pub struct Discovered {
    pub models: Vec<ModelInfo>,
    pub url: String,
}

/// 不依赖已落库订阅的探测目标: 新建自定义订阅的表单在保存前就要拉模型列表 (issue #44)。
#[derive(Debug, Clone)]
pub struct ProbeTarget {
    pub base_url: String,
    /// 协议默认的 models 路径, 如 `/v1/models` / `/v1beta/models`。
    pub path: String,
    pub auth_type: AuthType,
    pub auth_header_name: String,
    pub auth_header_value: String,
    pub required_headers: BTreeMap<String, String>,
}

/// 协议家族约定俗成的模型列表路径 (相对 base_url)。
pub fn default_models_path(auth_type: AuthType) -> &'static str {
    match auth_type {
        AuthType::GeminiApiKey | AuthType::GeminiInteractionsApiKey => "/v1beta/models",
        _ => "/v1/models",
    }
}

impl ProbeTarget {
    fn from_row(row: &SubscriptionRow) -> Self {
        // 老的自定义 Anthropic 订阅 snapshot 里 path 是空串 (当年 derive(Default) 的产物,
        // 因为 enabled=false 从不发请求所以没暴露)。空 path 会把 base_url 本身当 models 地址去请求。
        let path = match row.model_discovery.path.trim() {
            "" => default_models_path(row.auth_type).to_string(),
            p => p.to_string(),
        };
        let target = Self {
            base_url: row.base_url.clone(),
            path,
            auth_type: row.auth_type,
            auth_header_name: row.auth_header_name.clone(),
            auth_header_value: row.auth_header_value(),
            required_headers: row.required_headers.clone(),
        };
        if row.is_user_defined {
            target.with_custom_defaults()
        } else {
            target
        }
    }

    /// 自定义订阅专用的补头. Anthropic 官方 /v1/models 缺 `anthropic-version` 直接 400,
    /// 而自定义 Anthropic 订阅的 required_headers 默认是空的。用户自己填过则不覆盖。
    /// 内置 provider 不走这里 —— 它们的头以 yaml 为准。
    pub fn with_custom_defaults(mut self) -> Self {
        if self.auth_type == AuthType::ApiKey
            && !self
                .required_headers
                .keys()
                .any(|k| k.eq_ignore_ascii_case("anthropic-version"))
        {
            self.required_headers
                .insert("anthropic-version".into(), "2023-06-01".into());
        }
        self
    }
}

/// 自定义订阅的 models 地址候选, 按尝试顺序排列, 第一个永远是 `base_url + path`。
///
/// 用户填的 base_url 是 messages 的前缀, 不一定也是 models 的前缀
/// (例: `https://relay.example.com/anthropic` 的模型列表常在 `https://relay.example.com/v1/models`)。
///
/// 保守策略: 最多两个候选。base_url 带路径时追加 `origin + path`, 不逐级剥路径段 ——
/// 每多一个候选, 用户点一次按钮就可能多等一次超时, 而 `/a/b` → `/a` 这种中间层命中率很低。
/// 顺带覆盖 base_url 以 `/v1` 结尾的情况 (首选拼成 `/v1/v1/models`, origin 候选正好是对的)。
pub fn candidate_model_urls(base_url: &str, path: &str) -> Vec<String> {
    let primary = join_base_path(base_url, path);
    let mut urls = vec![primary];

    // 解析失败 (validate_base_url 只查了 scheme 前缀) 就只留首选, 让请求自己报错。
    if let Ok(parsed) = reqwest::Url::parse(base_url) {
        if !matches!(parsed.path(), "" | "/") {
            // origin 的 ascii_serialization 不带尾斜杠, 且保留非默认端口
            let origin = parsed.origin().ascii_serialization();
            let fallback = join_base_path(&origin, path);
            if !urls.contains(&fallback) {
                urls.push(fallback);
            }
        }
    }
    urls
}

/// 换一个候选地址有没有意义. 404/405 = 路径不对; InvalidResponse = 200 但不是模型列表
/// (中转站对未知路径常返回前端页面 HTML)。鉴权失败 / 网络错误换地址也没用, 立即上抛,
/// 否则用户看到的是最后一个候选的 404 而不是真实原因。
fn worth_trying_next(e: &DiscoveryError) -> bool {
    matches!(
        e,
        DiscoveryError::Http(404) | DiscoveryError::Http(405) | DiscoveryError::InvalidResponse(_)
    )
}

async fn fetch_url(
    client: &reqwest::Client,
    url: &str,
    target: &ProbeTarget,
) -> Result<Vec<ModelInfo>, DiscoveryError> {
    let mut req = client
        .get(url)
        .header(&target.auth_header_name, &target.auth_header_value);
    for (k, v) in target.required_headers.iter() {
        req = req.header(k, v);
    }
    let resp = req.send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(DiscoveryError::Http(status.as_u16()));
    }
    let text = resp.text().await?;

    match target.auth_type {
        // Gemini generateContent 与 Interactions 都用 /v1beta/models (Gemini envelope 格式)。
        AuthType::GeminiApiKey | AuthType::GeminiInteractionsApiKey => parse_gemini_envelope(&text),
        _ => parse_openai_envelope(&text),
    }
}

/// 按 [`candidate_model_urls`] 逐个尝试, 返回第一个成功的。全部失败时返回**首选地址**的错误
/// (它最可能是用户预期的那个, 后面的只是猜测)。
pub async fn probe(
    client: &reqwest::Client,
    target: &ProbeTarget,
) -> Result<Discovered, DiscoveryError> {
    let mut first_err: Option<DiscoveryError> = None;
    for url in candidate_model_urls(&target.base_url, &target.path) {
        match fetch_url(client, &url, target).await {
            Ok(models) => return Ok(Discovered { models, url }),
            Err(e) if worth_trying_next(&e) => {
                warn!(%url, error = %e, "models 候选地址失败, 尝试下一个");
                first_err.get_or_insert(e);
            }
            Err(e) => return Err(e),
        }
    }
    Err(first_err.unwrap_or_else(|| DiscoveryError::InvalidResponse("没有可尝试的 models 地址".into())))
}

pub async fn fetch(
    client: &reqwest::Client,
    row: &SubscriptionRow,
) -> Result<Discovered, DiscoveryError> {
    let target = ProbeTarget::from_row(row);

    // models 接口与 messages 不同域时, 订阅 snapshot 里的 model_discovery.url 是完整 URL。
    if let Some(full) = row.model_discovery.url.as_deref() {
        let models = fetch_url(client, full, &target).await?;
        return Ok(Discovered {
            models,
            url: full.to_string(),
        });
    }

    // 自定义订阅无视 enabled: 老的自定义 Anthropic 订阅 snapshot 写死了 enabled=false,
    // 不值得为此写 migration, 直接走候选探测。
    if row.is_user_defined {
        return probe(client, &target).await;
    }

    if !row.model_discovery.enabled {
        return Err(DiscoveryError::InvalidResponse(
            "该订阅未启用 /models 自动发现, 请使用手动输入".into(),
        ));
    }
    let url = join_base_path(&row.base_url, &row.model_discovery.path);
    let models = fetch_url(client, &url, &target).await?;
    Ok(Discovered { models, url })
}

fn parse_openai_envelope(text: &str) -> Result<Vec<ModelInfo>, DiscoveryError> {
    let parsed: ModelsEnvelope = serde_json::from_str(text)?;
    if parsed.data.is_empty() {
        return Err(DiscoveryError::InvalidResponse("data 为空".into()));
    }
    let mut models = Vec::with_capacity(parsed.data.len());
    for item in parsed.data {
        let id = item.id.or(item.model);
        let Some(id) = id else { continue };
        models.push(ModelInfo {
            id,
            display_name: item.display_name,
        });
    }
    if models.is_empty() {
        return Err(DiscoveryError::InvalidResponse("无法解析模型 id".into()));
    }
    Ok(models)
}

/// Gemini /v1beta/models 响应:
/// ```json
/// {"models": [{
///   "name": "models/gemini-2.5-flash",
///   "displayName": "Gemini 2.5 Flash",
///   "supportedGenerationMethods": ["generateContent", "countTokens"],
///   ...
/// }]}
/// ```
/// 过滤规则: 只保留 `supportedGenerationMethods` 含 `generateContent` 的模型 (排除 embedding/imagen 等).
fn parse_gemini_envelope(text: &str) -> Result<Vec<ModelInfo>, DiscoveryError> {
    let parsed: Value = serde_json::from_str(text)?;
    let arr = parsed
        .get("models")
        .and_then(|m| m.as_array())
        .ok_or_else(|| DiscoveryError::InvalidResponse("Gemini envelope 缺少 models 数组".into()))?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if name.is_empty() {
            continue;
        }
        // Gemini name 形如 "models/gemini-2.5-flash" — 去前缀
        let id = name.strip_prefix("models/").unwrap_or(name).to_string();
        // 过滤: 必须支持 generateContent
        let supports_generate = item
            .get("supportedGenerationMethods")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().any(|v| v.as_str() == Some("generateContent")))
            .unwrap_or(false);
        if !supports_generate {
            continue;
        }
        let display_name = item
            .get("displayName")
            .and_then(|v| v.as_str())
            .map(String::from);
        out.push(ModelInfo { id, display_name });
    }
    if out.is_empty() {
        return Err(DiscoveryError::InvalidResponse(
            "Gemini envelope 中无支持 generateContent 的模型".into(),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemini_envelope_filters_non_generate_models() {
        let text = r#"{
          "models": [
            {"name":"models/gemini-2.5-flash","displayName":"Gemini 2.5 Flash",
             "supportedGenerationMethods":["generateContent","countTokens"]},
            {"name":"models/embedding-001","displayName":"Embedding 001",
             "supportedGenerationMethods":["embedContent"]},
            {"name":"models/gemini-2.5-pro","displayName":"Gemini 2.5 Pro",
             "supportedGenerationMethods":["generateContent"]}
          ]
        }"#;
        let out = parse_gemini_envelope(text).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "gemini-2.5-flash");
        assert_eq!(out[0].display_name, Some("Gemini 2.5 Flash".into()));
        assert_eq!(out[1].id, "gemini-2.5-pro");
    }

    #[test]
    fn gemini_envelope_strips_models_prefix() {
        let text = r#"{"models":[
          {"name":"models/gemini-2.5-flash","supportedGenerationMethods":["generateContent"]}
        ]}"#;
        let out = parse_gemini_envelope(text).unwrap();
        assert_eq!(out[0].id, "gemini-2.5-flash");
    }

    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const OPENAI_MODELS: &str = r#"{"data":[{"id":"model-a"},{"id":"model-b"}]}"#;

    fn target(base_url: String, auth_type: AuthType) -> ProbeTarget {
        ProbeTarget {
            base_url,
            path: "/v1/models".into(),
            auth_type,
            auth_header_name: "authorization".into(),
            auth_header_value: "Bearer sk-test".into(),
            required_headers: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn probe_returns_models_and_the_url_that_worked() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("authorization", "Bearer sk-test"))
            .respond_with(ResponseTemplate::new(200).set_body_string(OPENAI_MODELS))
            .mount(&server)
            .await;

        let found = probe(
            &reqwest::Client::new(),
            &target(server.uri(), AuthType::OpenaiChatCompletionsApiKey),
        )
        .await
        .unwrap();
        assert_eq!(found.models.len(), 2);
        assert_eq!(found.url, format!("{}/v1/models", server.uri()));
    }

    /// 鉴权失败换地址也没用: 必须原样上抛 401, 不能被后续候选的 404 盖掉。
    #[tokio::test]
    async fn probe_surfaces_auth_failure_immediately() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/relay/v1/models"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&server)
            .await;
        // 第二候选明明能通 —— 但 401 之后不该再去碰它 (expect(0) 在 server drop 时校验)
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_string(OPENAI_MODELS))
            .expect(0)
            .mount(&server)
            .await;

        let err = probe(
            &reqwest::Client::new(),
            &target(format!("{}/relay", server.uri()), AuthType::OpenaiResponsesApiKey),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DiscoveryError::Http(401)), "{err}");
    }

    /// 中转站对未知路径返回前端页面 (200 + HTML): 归为 InvalidResponse, 且值得换下一个候选。
    #[tokio::test]
    async fn html_page_is_invalid_response_and_worth_retrying() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<!doctype html><html></html>"))
            .mount(&server)
            .await;

        let err = probe(
            &reqwest::Client::new(),
            &target(server.uri(), AuthType::OpenaiChatCompletionsApiKey),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DiscoveryError::InvalidResponse(_)), "{err}");
        assert!(worth_trying_next(&err));
        assert!(worth_trying_next(&DiscoveryError::Http(404)));
        assert!(!worth_trying_next(&DiscoveryError::Http(401)));
        assert!(!worth_trying_next(&DiscoveryError::Http(500)));
    }

    #[tokio::test]
    async fn custom_anthropic_target_sends_anthropic_version() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("anthropic-version", "2023-06-01"))
            .respond_with(ResponseTemplate::new(200).set_body_string(OPENAI_MODELS))
            .mount(&server)
            .await;

        let t = target(server.uri(), AuthType::ApiKey).with_custom_defaults();
        assert!(probe(&reqwest::Client::new(), &t).await.is_ok());
    }

    #[test]
    fn custom_defaults_keep_user_supplied_anthropic_version() {
        let mut t = target("https://example.com".into(), AuthType::ApiKey);
        t.required_headers
            .insert("Anthropic-Version".into(), "2099-01-01".into());
        let t = t.with_custom_defaults();
        assert_eq!(t.required_headers.len(), 1);

        // 非 Anthropic 协议不补
        let t = target("https://example.com".into(), AuthType::OpenaiResponsesApiKey)
            .with_custom_defaults();
        assert!(t.required_headers.is_empty());
    }

    #[test]
    fn first_candidate_is_always_base_plus_path() {
        let urls = candidate_model_urls("https://relay.example.com/anthropic/", "/v1/models");
        assert_eq!(urls[0], "https://relay.example.com/anthropic/v1/models");
    }

    /// 复刻真实日志: 老的自定义 Anthropic 订阅 snapshot 是 `enabled=false` + `path=""`,
    /// 修复前请求打到 `<base>/` 本身, 两个候选全 404。
    #[tokio::test]
    async fn legacy_custom_row_with_empty_path_still_discovers() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/anthropic/v1/models"))
            .and(header("anthropic-version", "2023-06-01"))
            .respond_with(ResponseTemplate::new(200).set_body_string(OPENAI_MODELS))
            .mount(&server)
            .await;

        let mut row = SubscriptionRow::test_fixture("custom", "custom");
        row.is_user_defined = true;
        row.base_url = format!("{}/anthropic", server.uri());
        row.auth_header_name = "authorization".into();
        row.model_discovery = crate::provider::model::ModelDiscovery {
            enabled: false,
            path: String::new(),
            url: None,
            cache_ttl_hours: 0,
            example_models: Vec::new(),
        };

        let found = fetch(&reqwest::Client::new(), &row).await.unwrap();
        assert_eq!(found.url, format!("{}/anthropic/v1/models", server.uri()));
    }

    /// `..ModelDiscovery::default()` 必须与 serde 默认值一致, 否则空 path 会再次写进 snapshot。
    #[test]
    fn model_discovery_default_matches_serde_default() {
        use crate::provider::model::ModelDiscovery;
        let from_serde: ModelDiscovery = serde_json::from_str("{}").unwrap();
        let from_default = ModelDiscovery::default();
        assert_eq!(from_default.path, "/v1/models");
        assert_eq!(from_default.path, from_serde.path);
        assert_eq!(from_default.enabled, from_serde.enabled);
        assert_eq!(from_default.cache_ttl_hours, from_serde.cache_ttl_hours);
    }

    #[test]
    fn root_base_url_has_no_extra_candidate() {
        for base in ["https://api.example.com", "https://api.example.com/"] {
            assert_eq!(
                candidate_model_urls(base, "/v1/models"),
                vec!["https://api.example.com/v1/models"],
                "{base}"
            );
        }
    }

    #[test]
    fn base_url_with_path_adds_origin_candidate_only() {
        // 多级路径也只加 origin 一个, 不逐级剥
        assert_eq!(
            candidate_model_urls("https://relay.example.com/api/anthropic", "/v1/models"),
            vec![
                "https://relay.example.com/api/anthropic/v1/models",
                "https://relay.example.com/v1/models",
            ]
        );
        // 非默认端口必须保留 (本地 Ollama / 自建网关)
        assert_eq!(
            candidate_model_urls("http://localhost:11434/v1", "/v1/models"),
            vec![
                "http://localhost:11434/v1/v1/models",
                "http://localhost:11434/v1/models",
            ]
        );
    }

    #[test]
    fn unparseable_base_url_keeps_primary_only() {
        assert_eq!(candidate_model_urls("http://", "/v1/models").len(), 1);
    }

    /// issue #44 的核心场景: base_url 带 `/anthropic` 这类 messages 专属前缀,
    /// 模型列表实际在根路径下。
    #[tokio::test]
    async fn probe_falls_back_when_base_url_has_messages_only_prefix() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_string(OPENAI_MODELS))
            .mount(&server)
            .await;
        // /anthropic/v1/models 未挂载 → wiremock 默认 404

        let t = target(format!("{}/anthropic", server.uri()), AuthType::ApiKey);
        let found = probe(&reqwest::Client::new(), &t).await.unwrap();
        assert_eq!(found.url, format!("{}/v1/models", server.uri()));
    }

    #[test]
    fn gemini_envelope_empty_after_filter_errors() {
        let text = r#"{"models":[
          {"name":"models/embedding-001","supportedGenerationMethods":["embedContent"]}
        ]}"#;
        assert!(parse_gemini_envelope(text).is_err());
    }
}

/// 返回 `(cache, 实际打通的 url)`. 调用方据此决定要不要把 url 写回订阅 snapshot。
pub async fn fetch_and_cache(
    pool: &SqlitePool,
    client: &reqwest::Client,
    row: &SubscriptionRow,
) -> Result<(ModelCache, String), DiscoveryError> {
    let found = fetch(client, row).await?;
    let cache = ModelCache {
        fetched_at: Utc::now(),
        models: found.models,
    };
    if let Err(e) = store::save_model_cache(pool, &row.id, &row.endpoint_id, &cache).await {
        warn!(?e, "model cache 持久化失败");
    } else {
        info!(subscription_id = %row.id, "model list cached");
    }
    Ok((cache, found.url))
}
