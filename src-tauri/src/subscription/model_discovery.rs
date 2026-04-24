//! 调用 `{base_url}/v1/models` 并缓存结果（设计稿 §8）。

use chrono::Utc;
use serde::Deserialize;
use sqlx::SqlitePool;
use tracing::{info, warn};
use uuid::Uuid;

use crate::error::AppError;
use crate::provider::Provider;
use crate::subscription::{
    model::{ModelCache, ModelInfo},
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
    provider: &Provider,
    endpoint_id: &str,
    api_key: &str,
) -> Result<Vec<ModelInfo>, DiscoveryError> {
    if !provider.model_discovery.enabled {
        return Err(DiscoveryError::InvalidResponse(
            "该厂商未提供 /models 接口, 请使用手动输入".into(),
        ));
    }

    let endpoint = provider
        .endpoint(endpoint_id)
        .ok_or_else(|| AppError::EndpointNotFound(endpoint_id.to_string()))?;

    // models 接口与 messages 不同域时，允许 YAML 里配置完整 url（例如 DeepSeek）。
    let url = if let Some(full) = provider.model_discovery.url.as_deref() {
        full.to_string()
    } else {
        let path = provider.model_discovery.path.as_str();
        let normalized = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        format!("{}{}", endpoint.base_url.trim_end_matches('/'), normalized)
    };

    let mut req = client.get(&url);
    let header_value = match provider.auth.header_format {
        crate::provider::model::AuthHeaderFormat::Bearer => format!("Bearer {api_key}"),
        crate::provider::model::AuthHeaderFormat::Raw => api_key.to_string(),
    };
    req = req.header(&provider.auth.header_name, header_value);
    for (k, v) in provider.required_headers.iter() {
        req = req.header(k, v);
    }

    let resp = req.send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(DiscoveryError::Http(status.as_u16()));
    }
    let text = resp.text().await?;
    let parsed: ModelsEnvelope = serde_json::from_str(&text)?;
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

pub async fn fetch_and_cache(
    pool: &SqlitePool,
    client: &reqwest::Client,
    provider: &Provider,
    endpoint_id: &str,
    subscription_id: &Uuid,
    api_key: &str,
) -> Result<ModelCache, DiscoveryError> {
    match fetch(client, provider, endpoint_id, api_key).await {
        Ok(models) => {
            let cache = ModelCache {
                fetched_at: Utc::now(),
                models,
            };
            if let Err(e) = store::save_model_cache(pool, subscription_id, endpoint_id, &cache).await {
                warn!(?e, "model cache 持久化失败");
            } else {
                info!(%subscription_id, "model list cached");
            }
            Ok(cache)
        }
        Err(e) => Err(e),
    }
}
