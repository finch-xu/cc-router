//! ChatGPT OAuth 登录流程的 Tauri commands.
//!
//! 前端流程:
//! 1. `start_chatgpt_device_flow` → 拿 user_code 与 verification URL,
//!    前端用 tauri_plugin_shell 打开浏览器并显示 user_code
//! 2. 反复轮询 `poll_chatgpt_device_code(device_code)` 直到拿到 ChatGptAccount
//! 3. 拿 device_code + 用户填的订阅信息调 `create_chatgpt_oauth_subscription`
//!    后端从内存缓存取出整套 token 落盘, refresh_token 永远不进 JS

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::Utc;
use serde::Deserialize;
use tauri::State;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::oauth::chatgpt::{
    build_codex_ua, ChatGptAccount, DeviceFlowStart, OAuthError, CHATGPT_ACCOUNT_ID_HEADER,
    CODEX_ORIGINATOR,
};
use crate::provider::model::AuthType;
use crate::state::AppState;
use crate::subscription::{
    model::{
        ModelSlots, OAuthMetadata, SubscriptionDto, SubscriptionRow, SubscriptionRuntime,
    },
    store,
};

fn map_oauth_err(e: OAuthError) -> AppError {
    match e {
        OAuthError::Storage(msg) => AppError::internal(msg),
        OAuthError::PendingDeviceNotFound => {
            AppError::BadRequest("device_code 不存在或已过期, 请重新发起登录".into())
        }
        OAuthError::DeviceCodeExpired => {
            AppError::BadRequest("登录链接已过期, 请重新发起".into())
        }
        OAuthError::RefreshTokenInvalid => {
            AppError::BadRequest("ChatGPT 登录已失效, 请重新登录".into())
        }
        // 其他都按通用上游错误处理
        other => AppError::internal(other.to_string()),
    }
}

#[tauri::command]
pub async fn start_chatgpt_device_flow(
    state: State<'_, AppState>,
) -> AppResult<DeviceFlowStart> {
    state
        .chatgpt_oauth
        .start_device_flow()
        .await
        .map_err(map_oauth_err)
}

/// 返回值约定:
/// - `Some(account)`: 用户已完成授权, 拿到了账号信息, 完整 token 集已缓存到后端.
/// - `None`: 用户还没在浏览器完成授权, 前端继续轮询.
#[tauri::command]
pub async fn poll_chatgpt_device_code(
    state: State<'_, AppState>,
    device_code: String,
) -> AppResult<Option<ChatGptAccount>> {
    state
        .chatgpt_oauth
        .poll_device_code(&device_code)
        .await
        .map_err(map_oauth_err)
}

#[derive(Debug, Deserialize)]
pub struct CreateChatGptOAuthSubscriptionInput {
    /// 走完 device_flow 拿到的 device_code.
    pub device_code: String,
    /// provider yaml id, 当前只允许 "openai_codex".
    pub provider_id: String,
    pub endpoint_id: String,
    pub display_name: String,
    pub model_slots: ModelSlots,
}

/// 用 device_code 取出后端缓存的完整 token, 落盘成一条新的 chatgpt_oauth 订阅,
/// 同时 seed access_token 进内存缓存, 这样首次请求不会触发 refresh.
#[tauri::command]
pub async fn create_chatgpt_oauth_subscription(
    state: State<'_, AppState>,
    input: CreateChatGptOAuthSubscriptionInput,
) -> AppResult<SubscriptionDto> {
    let provider = state
        .providers
        .get(&input.provider_id)
        .ok_or_else(|| AppError::ProviderNotFound(input.provider_id.clone()))?;

    if !matches!(provider.auth.auth_type, AuthType::ChatgptOauth) {
        return Err(AppError::BadRequest(format!(
            "Provider {} 不是 chatgpt_oauth 类型",
            input.provider_id
        )));
    }

    let endpoint = provider
        .endpoint(&input.endpoint_id)
        .ok_or_else(|| AppError::EndpointNotFound(input.endpoint_id.clone()))?;

    // 取出 device_code 对应的整套 token (一次性消费)
    let polled = state
        .chatgpt_oauth
        .consume_completed_poll(&input.device_code)
        .await
        .map_err(map_oauth_err)?;

    let id = Uuid::new_v4();
    let now = Utc::now();
    let metadata = OAuthMetadata {
        account_id: polled.account_id.clone(),
        email: polled.email.clone(),
        refresh_token: polled.refresh_token.clone(),
        authenticated_at: polled.authenticated_at_ms,
    };

    // required_headers 中的 ChatGPT-Account-Id 由 pipeline 在 OAuth 分支里动态注入,
    // 不写死进订阅 snapshot, 因为账户切换时不用重建订阅.
    let mut required_headers: BTreeMap<String, String> = provider.required_headers.clone();
    required_headers.remove(CHATGPT_ACCOUNT_ID_HEADER);

    let row = SubscriptionRow {
        id,
        provider_id: input.provider_id.clone(),
        endpoint_id: input.endpoint_id.clone(),
        display_name: input.display_name,
        api_key: String::new(), // OAuth 订阅不存 api_key
        auth_type: AuthType::ChatgptOauth,
        oauth_metadata: metadata,
        model_slots: input.model_slots,
        enabled: true,
        is_auth_failed: false,
        last_error_message: None,
        created_at: now,
        updated_at: now,
        base_url: endpoint.base_url.clone(),
        messages_path: endpoint.messages_path.clone(),
        auth_header_name: provider.auth.header_name.clone(),
        auth_header_format: provider.auth.header_format.clone(),
        required_headers,
        forward_headers: provider.forward_headers.clone(),
        model_discovery: provider.model_discovery.clone(),
        provider_display_name: provider.display_name.clone(),
        provider_icon: provider.icon.clone().unwrap_or_default(),
        is_user_defined: false,
    };

    store::insert(&state.db, &row).await?;

    // seed access_token 进内存, 这样第一次请求不需要 refresh
    state
        .chatgpt_oauth
        .seed_cache(id, polled.access_token, polled.access_token_expires_at_ms)
        .await;

    let rt = Arc::new(RwLock::new(SubscriptionRuntime::from_row(row)));
    {
        let mut subs = state.subscriptions.write().await;
        subs.insert(id, rt.clone());
    }

    let guard = rt.read().await;
    Ok(SubscriptionDto::from_runtime(&guard, vec![]))
}

/// 撤销订阅的 OAuth 缓存 (前端在 disconnect / 删除订阅时调).
#[tauri::command]
pub async fn forget_chatgpt_oauth_cache(
    state: State<'_, AppState>,
    subscription_id: String,
) -> AppResult<()> {
    let id = Uuid::parse_str(&subscription_id)
        .map_err(|_| AppError::BadRequest("无效 id".into()))?;
    state.chatgpt_oauth.forget(id).await;
    Ok(())
}

#[derive(Debug, serde::Serialize)]
pub struct ChatGptUsageDto {
    /// 计数 / 余额 / 周期等信息. 服务端协议未公开, 此处直接把 wham/usage 的原始 JSON 透传给前端,
    /// 前端按需读取字段渲染. 字段集合可能随 OpenAI 改动.
    pub raw: serde_json::Value,
    /// 拉取时刻 (ms), 前端用来显示「最后更新于 ...」.
    pub fetched_at: i64,
}

/// 查询某个 ChatGPT OAuth 订阅的额度. 出于「未公开协议易变」原则,
/// 后端不解析字段, 直接把 chatgpt.com/backend-api/wham/usage 的原始 JSON 透传给前端.
#[tauri::command]
pub async fn get_chatgpt_oauth_usage(
    state: State<'_, AppState>,
    subscription_id: String,
) -> AppResult<ChatGptUsageDto> {
    let id = Uuid::parse_str(&subscription_id)
        .map_err(|_| AppError::BadRequest("无效 id".into()))?;
    let rt = state
        .subscriptions
        .read()
        .await
        .get(&id)
        .cloned()
        .ok_or_else(|| AppError::SubscriptionNotFound(id.to_string()))?;
    let (refresh_token, account_id) = {
        let g = rt.read().await;
        if !matches!(g.row.auth_type, AuthType::ChatgptOauth) {
            return Err(AppError::BadRequest(
                "该订阅不是 ChatGPT OAuth 类型".into(),
            ));
        }
        (
            g.row.oauth_metadata.refresh_token.clone(),
            g.row.oauth_metadata.account_id.clone(),
        )
    };
    let access_token = state
        .chatgpt_oauth
        .get_valid_access_token(id, &refresh_token)
        .await
        .map_err(map_oauth_err)?;

    let raw: serde_json::Value = state
        .probe_client
        .get("https://chatgpt.com/backend-api/wham/usage")
        .bearer_auth(&access_token)
        .header(CHATGPT_ACCOUNT_ID_HEADER, &account_id)
        .header("User-Agent", build_codex_ua())
        .header("originator", CODEX_ORIGINATOR)
        .send()
        .await
        .map_err(|e| AppError::internal(format!("usage 请求失败: {e}")))?
        .json()
        .await
        .map_err(|e| AppError::internal(format!("usage 响应解析失败: {e}")))?;

    Ok(ChatGptUsageDto {
        raw,
        fetched_at: chrono::Utc::now().timestamp_millis(),
    })
}
