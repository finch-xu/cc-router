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
use crate::oauth::kiro::{KiroAccount, KiroDeviceFlowStart, KiroImportPreview};
use crate::provider::model::AuthType;
use crate::state::AppState;
use crate::subscription::{
    model::{
        KiroDisguise, KiroOAuthExtras, ModelSlots, OAuthMetadata, SubscriptionDto, SubscriptionRow,
        SubscriptionRuntime,
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
        kiro: None,
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
        balance_discovery: provider.balance_discovery.clone(),
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

// Kiro OAuth commands

#[derive(Debug, serde::Serialize)]
pub struct KiroImportResult {
    /// 凭据 import session id, 前端创建订阅时回传以取走后端缓存的 polled tokens (refresh_token 不进 JS).
    pub session_id: String,
    pub preview: KiroImportPreview,
}

/// 支持 `~` 展开为 $HOME, 调用方可直接传 `~/.aws/sso/cache/kiro-auth-token.json`.
#[tauri::command]
pub async fn import_kiro_credentials_from_file(
    state: State<'_, AppState>,
    path: String,
) -> AppResult<KiroImportResult> {
    let expanded = expand_tilde(&path);
    let json_str = std::fs::read_to_string(&expanded).map_err(|e| {
        AppError::BadRequest(format!(
            "无法读取凭据文件 {} ({}): {e}",
            path,
            expanded.display()
        ))
    })?;
    import_kiro_inner(&state, &json_str).await
}

fn expand_tilde(path: &str) -> std::path::PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            let mut out = std::path::PathBuf::from(home);
            out.push(rest);
            return out;
        }
    }
    if path == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return std::path::PathBuf::from(home);
        }
    }
    std::path::PathBuf::from(path)
}

#[tauri::command]
pub async fn import_kiro_credentials_from_text(
    state: State<'_, AppState>,
    json: String,
) -> AppResult<KiroImportResult> {
    import_kiro_inner(&state, &json).await
}

async fn import_kiro_inner(state: &AppState, json_str: &str) -> AppResult<KiroImportResult> {
    let (polled, preview) = state
        .kiro_oauth
        .parse_credential_json(json_str)
        .map_err(map_oauth_err)?;
    let session_id = Uuid::new_v4().to_string();
    state.kiro_oauth.cache_imported_session(&session_id, polled).await;
    Ok(KiroImportResult { session_id, preview })
}

#[tauri::command]
pub async fn start_kiro_device_flow(
    state: State<'_, AppState>,
    region: Option<String>,
) -> AppResult<KiroDeviceFlowStart> {
    state
        .kiro_oauth
        .start_device_flow(region.as_deref())
        .await
        .map_err(map_oauth_err)
}

#[tauri::command]
pub async fn poll_kiro_device_code(
    state: State<'_, AppState>,
    device_code: String,
) -> AppResult<Option<KiroAccount>> {
    state
        .kiro_oauth
        .poll_device_code(&device_code)
        .await
        .map_err(map_oauth_err)
}

#[derive(Debug, Deserialize)]
pub struct CreateKiroSubscriptionInput {
    /// 凭据来源二选一: JSON import 走 `session_id`, OIDC device flow 走 `device_code`.
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub device_code: Option<String>,
    pub provider_id: String,
    pub endpoint_id: String,
    pub display_name: String,
    pub model_slots: ModelSlots,
    /// 用户在 UI 编辑过的伪装字段. None 时用内置默认值.
    #[serde(default)]
    pub disguise: Option<KiroDisguise>,
    #[serde(default)]
    pub profile_arn_override: Option<String>,
}
#[tauri::command]
pub async fn create_kiro_subscription(
    state: State<'_, AppState>,
    input: CreateKiroSubscriptionInput,
) -> AppResult<SubscriptionDto> {
    let provider = state
        .providers
        .get(&input.provider_id)
        .ok_or_else(|| AppError::ProviderNotFound(input.provider_id.clone()))?;
    if !matches!(provider.auth.auth_type, AuthType::KiroOauth) {
        return Err(AppError::BadRequest(format!(
            "Provider {} 不是 kiro_oauth 类型",
            input.provider_id
        )));
    }
    let endpoint = provider
        .endpoint(&input.endpoint_id)
        .ok_or_else(|| AppError::EndpointNotFound(input.endpoint_id.clone()))?;

    let mut polled = match (input.session_id.as_deref(), input.device_code.as_deref()) {
        (Some(sid), _) => state
            .kiro_oauth
            .consume_imported_session(sid)
            .await
            .map_err(map_oauth_err)?,
        (None, Some(dc)) => state
            .kiro_oauth
            .consume_completed_poll(dc)
            .await
            .map_err(map_oauth_err)?,
        (None, None) => {
            return Err(AppError::BadRequest(
                "必须传 session_id 或 device_code".into(),
            ));
        }
    };
    if let Some(arn) = input.profile_arn_override {
        if !arn.is_empty() {
            polled.profile_arn = Some(arn);
        }
    }

    let metadata = state.kiro_oauth.polled_to_metadata(&polled, input.disguise);
    let id = Uuid::new_v4();
    let now = Utc::now();
    let required_headers: BTreeMap<String, String> = provider.required_headers.clone();

    let row = SubscriptionRow {
        id,
        provider_id: input.provider_id.clone(),
        endpoint_id: input.endpoint_id.clone(),
        display_name: input.display_name,
        api_key: String::new(),
        auth_type: AuthType::KiroOauth,
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
        balance_discovery: provider.balance_discovery.clone(),
        provider_display_name: provider.display_name.clone(),
        provider_icon: provider.icon.clone().unwrap_or_default(),
        is_user_defined: false,
    };

    store::insert(&state.db, &row).await?;

    // seed access_token 进内存 (若导入的凭据带了 access_token 就跳过首次 refresh)
    state
        .kiro_oauth
        .seed_cache(
            id,
            polled.access_token.clone(),
            polled.access_token_expires_at_ms,
        )
        .await;

    let rt = Arc::new(RwLock::new(SubscriptionRuntime::from_row(row)));
    {
        let mut subs = state.subscriptions.write().await;
        subs.insert(id, rt.clone());
    }
    let guard = rt.read().await;
    Ok(SubscriptionDto::from_runtime(&guard, vec![]))
}

#[tauri::command]
pub async fn forget_kiro_oauth_cache(
    state: State<'_, AppState>,
    subscription_id: String,
) -> AppResult<()> {
    let id = Uuid::parse_str(&subscription_id)
        .map_err(|_| AppError::BadRequest("无效 id".into()))?;
    state.kiro_oauth.forget(id).await;
    Ok(())
}

/// 修改一个 Kiro 订阅的伪装字段 (machineId / kiroVersion / systemVersion / nodeVersion).
#[tauri::command]
pub async fn update_kiro_disguise_fields(
    state: State<'_, AppState>,
    subscription_id: String,
    disguise: KiroDisguise,
) -> AppResult<()> {
    let id = Uuid::parse_str(&subscription_id)
        .map_err(|_| AppError::BadRequest("无效 id".into()))?;
    let rt = state
        .subscriptions
        .read()
        .await
        .get(&id)
        .cloned()
        .ok_or_else(|| AppError::SubscriptionNotFound(id.to_string()))?;

    // 取出当前 oauth_metadata + 修改 disguise + 落盘 + 同步 runtime
    let new_metadata = {
        let g = rt.read().await;
        if !matches!(g.row.auth_type, AuthType::KiroOauth) {
            return Err(AppError::BadRequest("该订阅不是 Kiro OAuth 类型".into()));
        }
        let mut meta = g.row.oauth_metadata.clone();
        match meta.kiro.as_mut() {
            Some(extras) => extras.disguise = disguise,
            None => {
                meta.kiro = Some(KiroOAuthExtras {
                    auth_method: crate::subscription::model::KiroAuthMethod::Social,
                    region: crate::oauth::kiro::DEFAULT_KIRO_REGION.to_string(),
                    profile_arn: None,
                    client_id: None,
                    client_secret: None,
                    disguise,
                });
            }
        }
        meta
    };
    store::update_oauth_metadata(&state.db, &id, &new_metadata).await?;
    {
        let mut g = rt.write().await;
        g.row.oauth_metadata = new_metadata;
        g.row.updated_at = Utc::now();
    }
    Ok(())
}
