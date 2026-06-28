use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tauri::State;
use tokio::sync::RwLock;
use tracing::warn;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::provider::model::{AuthHeaderFormat, AuthType, ModelDiscovery};
use crate::state::AppState;
use crate::subscription::{
    balance_discovery,
    model::{
        BalanceSnapshot, ModelCache, ModelInfo, ModelSlots, OAuthMetadata, SubscriptionDto,
        SubscriptionRow, SubscriptionRuntime, CUSTOM_GEMINI_INTERACTIONS_SOURCE_MARKER,
        CUSTOM_GEMINI_SOURCE_MARKER, CUSTOM_OPENAI_CHAT_SOURCE_MARKER, CUSTOM_OPENAI_SOURCE_MARKER,
        CUSTOM_SOURCE_MARKER,
    },
    model_discovery, ping, state_machine, store,
};

/// 自定义订阅的协议家族, 决定 cc-router 用哪条 dispatch 路径.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CustomProtocol {
    #[default]
    Anthropic,
    Gemini,
    /// OpenAI Responses (官方 / 兼容中转), 走 Anthropic ↔ Responses 翻译 + API key 鉴权.
    /// dispatch 走 [`crate::proxy::openai_responses_dispatch`].
    OpenaiResponses,
    /// OpenAI Chat Completions (官方 / DeepSeek / Together / Groq / Ollama / 各类 one-api 中转),
    /// 走 Anthropic ↔ Chat Completions 翻译 + API key 鉴权.
    /// dispatch 走 [`crate::proxy::openai_chat_completions_dispatch`].
    OpenaiChatCompletions,
    /// Google Gemini Interactions API (`/v1beta/interactions`, 新统一接口), 走 Anthropic ↔ Interactions
    /// step_list 翻译 + `auth_type=GeminiInteractionsApiKey` + `__custom_gemini_interactions__` provider_id.
    /// dispatch 走 [`crate::proxy::gemini_interactions_dispatch`].
    GeminiInteractions,
}

/// 创建订阅时的 source 区分: 内置 yaml 模板 vs 用户自定义。
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CreateSource {
    /// 标准路径: 后端从 yaml 模板 snapshot 连接信息进订阅 row。
    FromTemplate {
        provider_id: String,
        endpoint_id: String,
    },
    /// 自定义路径: 用户在表单里填完整连接信息。
    /// `protocol` 缺省 → Anthropic 透传; `Gemini` → `auth_type=GeminiApiKey` + `__custom_gemini__` provider_id.
    Custom {
        provider_display_name: String,
        base_url: String,
        messages_path: String,
        auth_header_name: String,
        auth_header_format: AuthHeaderFormat,
        #[serde(default)]
        protocol: CustomProtocol,
    },
}

#[derive(Debug, Deserialize)]
pub struct CreateSubscriptionInput {
    pub display_name: String,
    pub api_key: String,
    pub model_slots: ModelSlots,
    pub source: CreateSource,
}

#[derive(Debug, Deserialize, Default)]
pub struct SubscriptionPatch {
    pub display_name: Option<String>,
    pub model_slots: Option<ModelSlots>,
    /// 内置订阅: 切到同 provider 的另一个 endpoint, 后端 re-snapshot base_url/messages_path。
    /// 自定义订阅传该字段会被拒绝。
    pub endpoint_id: Option<String>,
    /// 自定义订阅: 改连接信息。内置订阅传该字段会被拒绝。
    pub connection: Option<ConnectionPatch>,
}

#[derive(Debug, Deserialize)]
pub struct ConnectionPatch {
    pub base_url: Option<String>,
    pub messages_path: Option<String>,
    pub auth_header_name: Option<String>,
    pub auth_header_format: Option<AuthHeaderFormat>,
    pub provider_display_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TestConnectionResult {
    pub ok: bool,
    pub message: String,
    /// 上游 HTTP 状态码; 网络错误时为 None。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    /// 实际用于测试的 model 名(从 slots 或 example_models 兜底选出)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_used: Option<String>,
    /// 测试通过且触发了状态机复活 → true。
    pub state_reset: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RefreshModelListResult {
    Auto {
        models: Vec<ModelInfo>,
        fetched_at: i64,
    },
    ManualFallback {
        reason: String,
    },
}

async fn collect_references(state: &AppState) -> std::collections::HashMap<Uuid, Vec<String>> {
    let mut out: std::collections::HashMap<Uuid, Vec<String>> = std::collections::HashMap::new();
    let vms = state.virtual_models.read().await;
    for vm in vms.values() {
        for sub_id in &vm.subscription_ids {
            out.entry(*sub_id)
                .or_default()
                .push(vm.name.as_str().to_string());
        }
    }
    out
}

#[tauri::command]
pub async fn list_subscriptions(state: State<'_, AppState>) -> AppResult<Vec<SubscriptionDto>> {
    let refs = collect_references(&state).await;
    let subs = state.subscriptions.read().await;
    let mut out = Vec::with_capacity(subs.len());
    for (id, rt) in subs.iter() {
        let guard = rt.read().await;
        let referenced = refs.get(id).cloned().unwrap_or_default();
        out.push(SubscriptionDto::from_runtime(&guard, referenced));
    }
    out.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    Ok(out)
}

#[tauri::command]
pub async fn get_subscription(
    state: State<'_, AppState>,
    id: String,
) -> AppResult<SubscriptionDto> {
    let id = Uuid::parse_str(&id).map_err(|_| AppError::BadRequest("无效 id".into()))?;
    let refs = collect_references(&state).await;
    let subs = state.subscriptions.read().await;
    let rt = subs
        .get(&id)
        .ok_or_else(|| AppError::SubscriptionNotFound(id.to_string()))?;
    let guard = rt.read().await;
    let referenced = refs.get(&id).cloned().unwrap_or_default();
    Ok(SubscriptionDto::from_runtime(&guard, referenced))
}

#[tauri::command]
pub async fn create_subscription(
    state: State<'_, AppState>,
    input: CreateSubscriptionInput,
) -> AppResult<SubscriptionDto> {
    let id = Uuid::new_v4();
    let now = Utc::now();

    // 根据 source 拼出 row 的 snapshot 字段
    let row = match input.source {
        CreateSource::FromTemplate {
            provider_id,
            endpoint_id,
        } => {
            let provider = state
                .providers
                .get(&provider_id)
                .ok_or_else(|| AppError::ProviderNotFound(provider_id.clone()))?;
            let endpoint = provider
                .endpoint(&endpoint_id)
                .ok_or_else(|| AppError::EndpointNotFound(endpoint_id.clone()))?;
            SubscriptionRow {
                id,
                provider_id: provider_id.clone(),
                endpoint_id: endpoint_id.clone(),
                display_name: input.display_name,
                api_key: input.api_key,
                auth_type: provider.auth.auth_type,
                oauth_metadata: OAuthMetadata::default(),
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
                required_headers: provider.required_headers.clone(),
                forward_headers: provider.forward_headers.clone(),
                model_discovery: provider.model_discovery.clone(),
                balance_discovery: provider.balance_discovery.clone(),
                provider_display_name: provider.display_name.clone(),
                provider_icon: provider.icon.clone().unwrap_or_default(),
                is_user_defined: false,
            }
        }
        CreateSource::Custom {
            provider_display_name,
            base_url,
            messages_path,
            auth_header_name,
            auth_header_format,
            protocol,
        } => {
            validate_base_url(&base_url)?;
            validate_messages_path(&messages_path)?;
            let is_gemini = protocol == CustomProtocol::Gemini;
            let is_openai = protocol == CustomProtocol::OpenaiResponses;
            let is_openai_chat = protocol == CustomProtocol::OpenaiChatCompletions;
            let is_gemini_interactions = protocol == CustomProtocol::GeminiInteractions;
            if is_gemini && !messages_path.contains("{model}") {
                return Err(AppError::BadRequest(
                    "Gemini 兼容订阅的 messages_path 必须包含 {model} 占位符".into(),
                ));
            }
            // Interactions API 的 model 在 body 里, messages_path 是固定 /v1beta/interactions,
            // 不需要 (也不应有) {model} 占位符 — 与旧 generateContent 的 Gemini 分支区别。
            let (provider_id, endpoint_id, auth_type_choice, icon, discovery) = if is_gemini {
                (
                    CUSTOM_GEMINI_SOURCE_MARKER.to_string(),
                    CUSTOM_GEMINI_SOURCE_MARKER.to_string(),
                    AuthType::GeminiApiKey,
                    "google".to_string(),
                    // Gemini 端点通常都有 /v1beta/models, 默认启用自动发现; 失败时前端 manual fallback.
                    ModelDiscovery {
                        enabled: true,
                        path: "/v1beta/models".into(),
                        ..ModelDiscovery::default()
                    },
                )
            } else if is_openai {
                (
                    CUSTOM_OPENAI_SOURCE_MARKER.to_string(),
                    CUSTOM_OPENAI_SOURCE_MARKER.to_string(),
                    AuthType::OpenaiResponsesApiKey,
                    "openai".to_string(),
                    // OpenAI 兼容 endpoint 普遍提供 /v1/models, 默认启用自动发现
                    ModelDiscovery {
                        enabled: true,
                        path: "/v1/models".into(),
                        ..ModelDiscovery::default()
                    },
                )
            } else if is_openai_chat {
                (
                    CUSTOM_OPENAI_CHAT_SOURCE_MARKER.to_string(),
                    CUSTOM_OPENAI_CHAT_SOURCE_MARKER.to_string(),
                    AuthType::OpenaiChatCompletionsApiKey,
                    "openai".to_string(),
                    // OpenAI Chat Completions 兼容生态 (DeepSeek/Together/Groq 等) 普遍提供 /v1/models, 默认启用
                    ModelDiscovery {
                        enabled: true,
                        path: "/v1/models".into(),
                        ..ModelDiscovery::default()
                    },
                )
            } else if is_gemini_interactions {
                (
                    CUSTOM_GEMINI_INTERACTIONS_SOURCE_MARKER.to_string(),
                    CUSTOM_GEMINI_INTERACTIONS_SOURCE_MARKER.to_string(),
                    AuthType::GeminiInteractionsApiKey,
                    "google".to_string(),
                    // Gemini 端点 (含 Interactions) 都在 generativelanguage.googleapis.com, 复用 /v1beta/models 自动发现.
                    ModelDiscovery {
                        enabled: true,
                        path: "/v1beta/models".into(),
                        ..ModelDiscovery::default()
                    },
                )
            } else {
                (
                    CUSTOM_SOURCE_MARKER.to_string(),
                    CUSTOM_SOURCE_MARKER.to_string(),
                    AuthType::ApiKey,
                    "custom".to_string(),
                    // 自定义 Anthropic 订阅默认 disable model_discovery, 走 manual fallback
                    ModelDiscovery {
                        enabled: false,
                        ..ModelDiscovery::default()
                    },
                )
            };
            SubscriptionRow {
                id,
                provider_id,
                endpoint_id,
                display_name: input.display_name,
                api_key: input.api_key,
                auth_type: auth_type_choice,
                oauth_metadata: OAuthMetadata::default(),
                model_slots: input.model_slots,
                enabled: true,
                is_auth_failed: false,
                last_error_message: None,
                created_at: now,
                updated_at: now,
                base_url,
                messages_path,
                auth_header_name,
                auth_header_format,
                required_headers: BTreeMap::new(),
                forward_headers: Vec::new(),
                model_discovery: discovery,
                balance_discovery: None,
                provider_display_name,
                provider_icon: icon,
                is_user_defined: true,
            }
        }
    };

    store::insert(&state.db, &row).await?;

    let rt = Arc::new(RwLock::new(SubscriptionRuntime::from_row(row)));
    {
        let mut subs = state.subscriptions.write().await;
        subs.insert(id, rt.clone());
    }

    let guard = rt.read().await;
    Ok(SubscriptionDto::from_runtime(&guard, vec![]))
}

#[tauri::command]
pub async fn update_subscription(
    state: State<'_, AppState>,
    id: String,
    patch: SubscriptionPatch,
) -> AppResult<SubscriptionDto> {
    let id = Uuid::parse_str(&id).map_err(|_| AppError::BadRequest("无效 id".into()))?;
    let rt = {
        let subs = state.subscriptions.read().await;
        subs.get(&id)
            .cloned()
            .ok_or_else(|| AppError::SubscriptionNotFound(id.to_string()))?
    };

    // 先做所有校验/反查 (不持锁), 失败时不会留下半应用的内存修改。
    if patch.endpoint_id.is_some() && patch.connection.is_some() {
        return Err(AppError::BadRequest(
            "endpoint_id 与 connection patch 不能同时存在".into(),
        ));
    }
    let endpoint_resnapshot = if let Some(new_endpoint_id) = patch.endpoint_id.as_ref() {
        let is_user_defined = rt.read().await.row.is_user_defined;
        if is_user_defined {
            return Err(AppError::BadRequest(
                "自定义订阅不支持切 endpoint, 请用 connection patch 改连接信息".into(),
            ));
        }
        let provider_id = rt.read().await.row.provider_id.clone();
        let provider = state
            .providers
            .get(&provider_id)
            .ok_or_else(|| AppError::ProviderNotFound(provider_id.clone()))?;
        let endpoint = provider
            .endpoint(new_endpoint_id)
            .ok_or_else(|| AppError::EndpointNotFound(new_endpoint_id.clone()))?;
        Some((
            new_endpoint_id.clone(),
            endpoint.base_url.clone(),
            endpoint.messages_path.clone(),
        ))
    } else {
        None
    };
    if let Some(conn) = patch.connection.as_ref() {
        let is_user_defined = rt.read().await.row.is_user_defined;
        if !is_user_defined {
            return Err(AppError::BadRequest(
                "内置订阅不能改连接信息, 请用 endpoint_id 切换 endpoint".into(),
            ));
        }
        if let Some(v) = conn.base_url.as_deref() {
            validate_base_url(v)?;
        }
        if let Some(v) = conn.messages_path.as_deref() {
            validate_messages_path(v)?;
        }
    }

    {
        let mut guard = rt.write().await;
        if let Some(name) = patch.display_name {
            guard.row.display_name = name;
        }
        if let Some(slots) = patch.model_slots {
            guard.row.model_slots = slots;
        }
        if let Some((eid, base, path)) = endpoint_resnapshot {
            guard.row.endpoint_id = eid;
            guard.row.base_url = base;
            guard.row.messages_path = path;
        }
        if let Some(conn) = patch.connection {
            if let Some(v) = conn.base_url {
                guard.row.base_url = v;
            }
            if let Some(v) = conn.messages_path {
                guard.row.messages_path = v;
            }
            if let Some(v) = conn.auth_header_name {
                guard.row.auth_header_name = v;
            }
            if let Some(v) = conn.auth_header_format {
                guard.row.auth_header_format = v;
            }
            if let Some(v) = conn.provider_display_name {
                guard.row.provider_display_name = v;
            }
        }

        guard.row.updated_at = Utc::now();
        store::update_row(&state.db, &guard.row).await?;
    }

    let refs = collect_references(&state).await;
    let guard = rt.read().await;
    let referenced = refs.get(&id).cloned().unwrap_or_default();
    Ok(SubscriptionDto::from_runtime(&guard, referenced))
}

#[tauri::command]
pub async fn update_subscription_key(
    state: State<'_, AppState>,
    id: String,
    new_key: String,
) -> AppResult<()> {
    let id = Uuid::parse_str(&id).map_err(|_| AppError::BadRequest("无效 id".into()))?;

    store::update_api_key(&state.db, &id, &new_key).await?;

    let rt = {
        let subs = state.subscriptions.read().await;
        subs.get(&id).cloned()
    };
    if let Some(rt) = rt {
        {
            let mut guard = rt.write().await;
            guard.row.api_key = new_key;
            guard.row.is_auth_failed = false;
            guard.row.last_error_message = None;
            guard.last_error_message = None;
            guard.row.updated_at = Utc::now();
        }
        let _ = state_machine::apply(
            &state.db,
            &state.app_handle,
            &state.event_log_tx,
            rt,
            state_machine::Event::UserUpdateKey,
        )
        .await;
    }
    Ok(())
}

#[tauri::command]
pub async fn delete_subscription(state: State<'_, AppState>, id: String) -> AppResult<()> {
    let id = Uuid::parse_str(&id).map_err(|_| AppError::BadRequest("无效 id".into()))?;
    {
        let mut subs = state.subscriptions.write().await;
        subs.remove(&id);
    }
    {
        let mut vms = state.virtual_models.write().await;
        for vm in vms.values_mut() {
            vm.subscription_ids.retain(|x| *x != id);
        }
    }
    state.chatgpt_oauth.forget(id).await;
    store::delete(&state.db, &id).await?;
    Ok(())
}

#[tauri::command]
pub async fn set_subscription_enabled(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> AppResult<()> {
    let id = Uuid::parse_str(&id).map_err(|_| AppError::BadRequest("无效 id".into()))?;

    let rt = {
        let subs = state.subscriptions.read().await;
        subs.get(&id)
            .cloned()
            .ok_or_else(|| AppError::SubscriptionNotFound(id.to_string()))?
    };

    store::update_enabled(&state.db, &id, enabled).await?;
    {
        let mut guard = rt.write().await;
        guard.row.enabled = enabled;
        guard.row.updated_at = Utc::now();
    }

    let event = if enabled {
        state_machine::Event::UserEnable
    } else {
        state_machine::Event::UserDisable
    };
    let _ = state_machine::apply(&state.db, &state.app_handle, &state.event_log_tx, rt, event).await;
    Ok(())
}

/// 测试一条订阅的真实可达性: 用最小 prompt 直接打 messages 端点。
///
/// snapshot 模型: 全部连接信息从订阅 row 自身字段读, 不再回查 state.providers。
#[tauri::command]
pub async fn test_connection(
    state: State<'_, AppState>,
    id: String,
) -> AppResult<TestConnectionResult> {
    let id = Uuid::parse_str(&id).map_err(|_| AppError::BadRequest("无效 id".into()))?;
    let rt = {
        let subs = state.subscriptions.read().await;
        subs.get(&id)
            .cloned()
            .ok_or_else(|| AppError::SubscriptionNotFound(id.to_string()))?
    };
    let row = {
        let g = rt.read().await;
        g.row.clone()
    };

    let model = match ping::pick_test_model(&row) {
        Some(m) => m,
        None => {
            return Ok(TestConnectionResult {
                ok: false,
                message: "订阅未配置任何 model 槽位, 且未提供 example_models, 无法测试".into(),
                http_status: None,
                model_used: None,
                state_reset: false,
            });
        }
    };

    let result = ping::probe_subscription(&state, &row, &model).await;

    let mut state_reset = false;
    if result.ok {
        match state_machine::apply(
            &state.db,
            &state.app_handle,
            &state.event_log_tx,
            rt.clone(),
            state_machine::Event::UserManualReset,
        )
        .await
        {
            Ok(_) => state_reset = true,
            Err(e) => warn!(?e, "UserManualReset apply 失败, 复活效果未生效"),
        }
    }
    Ok(TestConnectionResult {
        ok: result.ok,
        message: result.message,
        http_status: result.http_status,
        model_used: Some(model),
        state_reset,
    })
}

#[tauri::command]
pub async fn refresh_model_list(
    state: State<'_, AppState>,
    id: String,
) -> AppResult<RefreshModelListResult> {
    let id = Uuid::parse_str(&id).map_err(|_| AppError::BadRequest("无效 id".into()))?;
    let rt = {
        let subs = state.subscriptions.read().await;
        subs.get(&id)
            .cloned()
            .ok_or_else(|| AppError::SubscriptionNotFound(id.to_string()))?
    };
    let row = {
        let g = rt.read().await;
        g.row.clone()
    };

    // OAuth 订阅走独立的 chatgpt_models 路径 (端点 / 鉴权 / 响应格式都不同),
    // 其他 provider 走通用的 OpenAI /v1/models envelope.
    let result = if matches!(row.auth_type, AuthType::ChatgptOauth) {
        crate::oauth::chatgpt_models::fetch_and_cache(
            &state.db,
            &state.http_client,
            &state.chatgpt_oauth,
            &row,
        )
        .await
    } else {
        model_discovery::fetch_and_cache(&state.db, &state.http_client, &row).await
    };

    match result {
        Ok(cache) => {
            let mut guard = rt.write().await;
            guard.model_cache = Some(ModelCache {
                fetched_at: cache.fetched_at,
                models: cache.models.clone(),
            });
            Ok(RefreshModelListResult::Auto {
                models: cache.models,
                fetched_at: cache.fetched_at.timestamp_millis(),
            })
        }
        Err(e) => Ok(RefreshModelListResult::ManualFallback {
            reason: e.to_string(),
        }),
    }
}

/// 余额刷新结果. 前端按 kind 分发渲染.
///
/// - `success`: 成功拉到余额, 数据已写 DB + runtime, UI 同步更新
/// - `failed`: 网络/HTTP/解析失败, UI 显示 reason, 旧缓存值仍可见 (不擦)
/// - `unsupported`: provider yaml 没声明 balance_discovery, UI 不应展示余额卡片
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RefreshBalanceResult {
    Success {
        snapshot: BalanceSnapshot,
        fetched_at: i64,
    },
    Failed {
        reason: String,
    },
    Unsupported,
}

#[tauri::command]
pub async fn refresh_subscription_balance(
    state: State<'_, AppState>,
    id: String,
) -> AppResult<RefreshBalanceResult> {
    let id = Uuid::parse_str(&id).map_err(|_| AppError::BadRequest("无效 id".into()))?;
    let rt = {
        let subs = state.subscriptions.read().await;
        subs.get(&id)
            .cloned()
            .ok_or_else(|| AppError::SubscriptionNotFound(id.to_string()))?
    };
    let row = {
        let g = rt.read().await;
        g.row.clone()
    };

    // 早返 Unsupported, 避免发出空请求
    if row.balance_discovery.is_none() {
        return Ok(RefreshBalanceResult::Unsupported);
    }

    match balance_discovery::fetch_and_cache(&state.db, &state.http_client, &row).await {
        Ok(snapshot) => {
            let fetched_at = snapshot.fetched_at.timestamp_millis();
            // 写回 runtime, 下一次前端 list_subscriptions 就拿到新值
            {
                let mut guard = rt.write().await;
                guard.balance_cache = Some(snapshot.clone());
            }
            Ok(RefreshBalanceResult::Success {
                snapshot,
                fetched_at,
            })
        }
        Err(e) => Ok(RefreshBalanceResult::Failed {
            reason: e.to_string(),
        }),
    }
}

fn validate_base_url(s: &str) -> AppResult<()> {
    if !(s.starts_with("http://") || s.starts_with("https://")) {
        return Err(AppError::BadRequest(
            "base_url 必须以 http:// 或 https:// 开头".into(),
        ));
    }
    Ok(())
}

fn validate_messages_path(s: &str) -> AppResult<()> {
    if !s.starts_with('/') {
        return Err(AppError::BadRequest("messages_path 必须以 / 开头".into()));
    }
    Ok(())
}
