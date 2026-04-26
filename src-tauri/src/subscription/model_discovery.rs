//! 调用 `{base_url}/v1/models` 并缓存结果（设计稿 §8）。
//!
//! snapshot 模型: 全部连接信息从订阅 row 自身字段读, 不再回查 state.providers。

use chrono::Utc;
use serde::Deserialize;
use sqlx::SqlitePool;
use tracing::{info, warn};

use crate::error::AppError;
use crate::provider::model::join_base_path;
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

    let mut req = client.get(&url);
    req = req.header(&row.auth_header_name, row.auth_header_value());
    for (k, v) in row.required_headers.iter() {
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
