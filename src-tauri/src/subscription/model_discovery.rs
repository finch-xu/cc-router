//! 调用 `{base_url}/v1/models` 并缓存结果（设计稿 §8）。
//!
//! snapshot 模型: 全部连接信息从订阅 row 自身字段读, 不再回查 state.providers。
//!
//! Envelope 解析按 `auth_type` 分发:
//! - `ApiKey` / `ChatgptOauth` / `KiroOauth` → OpenAI 风格 `{data:[{id,model,display_name}]}`
//! - `GeminiApiKey` / `GeminiInteractionsApiKey` → Gemini 风格 `{models:[{name:"models/gemini-...", displayName, supportedGenerationMethods}]}`

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

pub async fn fetch(
    client: &reqwest::Client,
    row: &SubscriptionRow,
) -> Result<Vec<ModelInfo>, DiscoveryError> {
    if !row.model_discovery.enabled {
        return Err(DiscoveryError::InvalidResponse(
            "该订阅未启用 /models 自动发现, 请使用手动输入".into(),
        ));
    }

    // models 接口与 messages 不同域时, 订阅 snapshot 里的 model_discovery.url 是完整 URL。
    let url = match row.model_discovery.url.as_deref() {
        Some(full) => full.to_string(),
        None => join_base_path(&row.base_url, &row.model_discovery.path),
    };

    let req = row.apply_auth_and_required_headers(client.get(&url));
    let resp = req.send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(DiscoveryError::Http(status.as_u16()));
    }
    let text = resp.text().await?;

    match row.auth_type {
        // Gemini generateContent 与 Interactions 都用 /v1beta/models (Gemini envelope 格式)。
        AuthType::GeminiApiKey | AuthType::GeminiInteractionsApiKey => parse_gemini_envelope(&text),
        _ => parse_openai_envelope(&text),
    }
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

    #[test]
    fn gemini_envelope_empty_after_filter_errors() {
        let text = r#"{"models":[
          {"name":"models/embedding-001","supportedGenerationMethods":["embedContent"]}
        ]}"#;
        assert!(parse_gemini_envelope(text).is_err());
    }
}

pub async fn fetch_and_cache(
    pool: &SqlitePool,
    client: &reqwest::Client,
    row: &SubscriptionRow,
) -> Result<ModelCache, DiscoveryError> {
    match fetch(client, row).await {
        Ok(models) => {
            let cache = ModelCache {
                fetched_at: Utc::now(),
                models,
            };
            if let Err(e) = store::save_model_cache(pool, &row.id, &row.endpoint_id, &cache).await {
                warn!(?e, "model cache 持久化失败");
            } else {
                info!(subscription_id = %row.id, "model list cached");
            }
            Ok(cache)
        }
        Err(e) => Err(e),
    }
}
