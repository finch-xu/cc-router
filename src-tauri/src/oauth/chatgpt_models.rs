//! 通过 OAuth access_token 调 chatgpt.com/backend-api/codex/models 拉取当前账号
//! 可见的 Codex 模型列表. 与 oauth_dispatch.rs / transform/openai_responses.rs 一样
//! 走完全独立的代码路径, 不复用 subscription/model_discovery.rs 的 OpenAI 标准 envelope.
//!
//! 端点事实 (来源: openai/codex 官方仓库 codex-rs/codex-api/src/endpoint/models.rs,
//! 未亲自实测; 首次跑出问题先用 examples/chatgpt_smoke.rs 抓真实响应):
//! - `GET https://chatgpt.com/backend-api/codex/models?client_version=<ver>`
//! - 鉴权: `Authorization: Bearer <oauth access_token>` (同 /responses)
//! - 响应: `{"models": [{slug, display_name, visibility: "list"|"hide", supported_in_api, priority, ...}]}`
//! - 必带的风控 header (与 /responses 保持一致): User-Agent / OpenAI-Beta / originator / ChatGPT-Account-Id

use std::sync::Arc;

use chrono::Utc;
use serde::Deserialize;
use sqlx::SqlitePool;
use tracing::{info, warn};

use crate::oauth::chatgpt::{
    build_codex_ua, ChatGptOAuthManager, CHATGPT_ACCOUNT_ID_HEADER, CODEX_ORIGINATOR,
};
use crate::subscription::{
    model::{ModelCache, ModelInfo, SubscriptionRow},
    model_discovery::DiscoveryError,
    store,
};

const MODELS_URL: &str = "https://chatgpt.com/backend-api/codex/models";

#[derive(Debug, Deserialize)]
struct CodexModelsEnvelope {
    #[serde(default)]
    models: Vec<CodexModelItem>,
}

#[derive(Debug, Deserialize)]
struct CodexModelItem {
    slug: String,
    #[serde(default)]
    display_name: Option<String>,
    /// 实测预期 "list" | "hide", 用 Option<String> 兜底未知取值.
    #[serde(default)]
    visibility: Option<String>,
}

pub async fn fetch(
    client: &reqwest::Client,
    oauth_manager: &Arc<ChatGptOAuthManager>,
    row: &SubscriptionRow,
) -> Result<Vec<ModelInfo>, DiscoveryError> {
    if !row.model_discovery.enabled {
        return Err(DiscoveryError::InvalidResponse(
            "该订阅未启用 /models 自动发现, 请使用手动输入".into(),
        ));
    }

    let refresh_token = &row.oauth_metadata.refresh_token;
    let account_id = &row.oauth_metadata.account_id;
    if refresh_token.is_empty() || account_id.is_empty() {
        return Err(DiscoveryError::InvalidResponse(
            "订阅缺少 OAuth 凭据 (refresh_token / account_id), 请重新登录".into(),
        ));
    }

    let access_token = oauth_manager
        .get_valid_access_token(row.id, refresh_token)
        .await
        .map_err(|e| DiscoveryError::InvalidResponse(format!("OAuth 刷新失败: {e}")))?;

    let client_version = env!("CARGO_PKG_VERSION");
    let url = format!("{MODELS_URL}?client_version={client_version}");
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {access_token}"))
        .header(CHATGPT_ACCOUNT_ID_HEADER, account_id)
        .header("User-Agent", build_codex_ua())
        .header("OpenAI-Beta", "responses=experimental")
        .header("originator", CODEX_ORIGINATOR)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        return Err(DiscoveryError::Http(status.as_u16()));
    }
    let text = resp.text().await?;
    let parsed: CodexModelsEnvelope = serde_json::from_str(&text)?;

    let models: Vec<ModelInfo> = parsed
        .models
        .into_iter()
        .filter(|m| m.visibility.as_deref() == Some("list"))
        .map(|m| ModelInfo {
            id: m.slug,
            display_name: m.display_name,
        })
        .collect();
    if models.is_empty() {
        return Err(DiscoveryError::InvalidResponse(
            "返回的模型列表为空 (账号无 Codex 权限或全部 visibility=hide)".into(),
        ));
    }
    Ok(models)
}

pub async fn fetch_and_cache(
    pool: &SqlitePool,
    client: &reqwest::Client,
    oauth_manager: &Arc<ChatGptOAuthManager>,
    row: &SubscriptionRow,
) -> Result<ModelCache, DiscoveryError> {
    match fetch(client, oauth_manager, row).await {
        Ok(models) => {
            let cache = ModelCache {
                fetched_at: Utc::now(),
                models,
            };
            if let Err(e) = store::save_model_cache(pool, &row.id, &row.endpoint_id, &cache).await {
                warn!(?e, "model cache 持久化失败");
            } else {
                info!(subscription_id = %row.id, "ChatGPT codex models cached");
            }
            Ok(cache)
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_parses_and_filters_hide_models() {
        let body = serde_json::json!({
            "models": [
                {"slug": "gpt-5.5", "display_name": "GPT-5.5", "visibility": "list"},
                {"slug": "gpt-5-internal", "display_name": "internal", "visibility": "hide"},
                {"slug": "gpt-5-codex", "display_name": "Codex", "visibility": "list"},
            ]
        });
        let parsed: CodexModelsEnvelope = serde_json::from_value(body).unwrap();
        let kept: Vec<&str> = parsed
            .models
            .iter()
            .filter(|m| m.visibility.as_deref() == Some("list"))
            .map(|m| m.slug.as_str())
            .collect();
        assert_eq!(kept, vec!["gpt-5.5", "gpt-5-codex"]);
    }

    #[test]
    fn envelope_handles_missing_optional_fields() {
        // 真实响应里 display_name / visibility 都可能缺失或 null, 用 serde(default) 兜住.
        let body = serde_json::json!({
            "models": [
                {"slug": "gpt-5.5"},
                {"slug": "gpt-5", "visibility": null, "display_name": null},
            ]
        });
        let parsed: CodexModelsEnvelope = serde_json::from_value(body).unwrap();
        assert_eq!(parsed.models.len(), 2);
        assert!(parsed.models.iter().all(|m| m.visibility.is_none()));
        assert!(parsed.models.iter().all(|m| m.display_name.is_none()));
    }

    #[test]
    fn envelope_handles_empty_array() {
        // 空数组要能解出来, fetch() 之后再判 is_empty 报 InvalidResponse.
        let body = serde_json::json!({"models": []});
        let parsed: CodexModelsEnvelope = serde_json::from_value(body).unwrap();
        assert!(parsed.models.is_empty());
    }

    #[test]
    fn envelope_ignores_unknown_visibility_values() {
        // 后端如果以后加了 "preview" / "deprecated", 这些 entry 会被过滤掉而不是崩.
        let body = serde_json::json!({
            "models": [
                {"slug": "gpt-future", "visibility": "preview"},
                {"slug": "gpt-5.5", "visibility": "list"},
            ]
        });
        let parsed: CodexModelsEnvelope = serde_json::from_value(body).unwrap();
        let kept: Vec<&str> = parsed
            .models
            .iter()
            .filter(|m| m.visibility.as_deref() == Some("list"))
            .map(|m| m.slug.as_str())
            .collect();
        assert_eq!(kept, vec!["gpt-5.5"]);
    }
}
