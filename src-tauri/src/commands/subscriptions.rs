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
        BalanceSnapshot, ModelCache, ModelInfo, ModelSlots, OAuthMetadata, SlotEfforts,
        SubscriptionDto, SubscriptionRow, SubscriptionRuntime, CUSTOM_GEMINI_INTERACTIONS_SOURCE_MARKER,
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
    /// step_list 翻译 + `auth_type=GeminiInteractionsApiKey` + `custom-gemini-interactions` provider_id.
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
    /// `protocol` 缺省 → Anthropic 透传; `Gemini` → `auth_type=GeminiApiKey` + `custom-gemini` provider_id.
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
    // 刻意不含 slot_efforts: 新建订阅一律全 auto (DB 列 DEFAULT '{}'), 用户创建后进编辑页再设。
    // 新建向导是两步非事务流程, 不往里再加表单字段。
}

/// 订阅槽位级 effort 允许的档位. 刻意不含 `minimal`:
/// minimal 是 OpenAI 系专有档 (Anthropic 官方 effort 只有 low/medium/high/xhigh/max),
/// 而槽位 effort 是跨全部 dispatch 路径的统一设置 —— 暴露一个在 Anthropic 透传 /
/// Gemini interactions 上必须偷偷降级的档位等于假装支持。需要 minimal 的 OpenAI 用户仍可走
/// provider yaml `default_reasoning_effort` 或客户端 body `extra_body.reasoning_effort`。
///
/// 与 `providers/_schema.json` 里 6 值 enum 的不对称是有意的: 那个 enum 约束 provider yaml
/// 默认值 (provider 作者清楚自家协议), 本白名单约束用户在 UI 选的跨协议统一值。
const ALLOWED_SLOT_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// 校验 patch 里的槽位 effort 都在白名单内 (空/缺失 = auto, 合法)。
/// 显式列四个槽位而不是遍历: 将来给 ModelSlots 加槽位时这里会因缺字段而被注意到。
fn validate_slot_efforts(e: &SlotEfforts) -> AppResult<()> {
    for (slot, v) in [
        ("fable", &e.fable),
        ("opus", &e.opus),
        ("sonnet", &e.sonnet),
        ("haiku", &e.haiku),
    ] {
        let Some(level) = v.as_deref().filter(|s| !s.is_empty()) else {
            continue;
        };
        if !ALLOWED_SLOT_EFFORTS.contains(&level) {
            return Err(AppError::BadRequest(format!(
                "槽位 {slot} 的思考档位 \"{level}\" 无效, 可选: auto / {}",
                ALLOWED_SLOT_EFFORTS.join(" / ")
            )));
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize, Default)]
pub struct SubscriptionPatch {
    pub display_name: Option<String>,
    pub model_slots: Option<ModelSlots>,
    /// 每槽位 reasoning effort 覆盖. 整块替换 (与 model_slots 一致, 无 per-slot patch)。
    /// 字段缺失 = auto, 即透传客户端请求携带的 effort。
    pub slot_efforts: Option<SlotEfforts>,
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
    /// 额外出站 header。整块替换 (与 model_slots / slot_efforts 一致):
    /// Some(map) = 覆盖为 map (空 map = 清空), None = 不改。
    pub required_headers: Option<BTreeMap<String, String>>,
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
                slot_efforts: SlotEfforts::default(),
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
                slot_efforts: SlotEfforts::default(),
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
        if let Some(hs) = conn.required_headers.as_ref() {
            // 针对 patch 后生效的鉴权头名校验: conn 新值优先, 否则现有 row 值。
            let effective_auth_name = match conn.auth_header_name.clone() {
                Some(v) => v,
                None => rt.read().await.row.auth_header_name.clone(),
            };
            validate_required_headers(hs, &effective_auth_name)?;
        }
    }
    if let Some(efforts) = patch.slot_efforts.as_ref() {
        validate_slot_efforts(efforts)?;
    }

    {
        let mut guard = rt.write().await;
        if let Some(name) = patch.display_name {
            guard.row.display_name = name;
        }
        if let Some(slots) = patch.model_slots {
            guard.row.model_slots = slots;
        }
        if let Some(efforts) = patch.slot_efforts {
            guard.row.slot_efforts = efforts;
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
            if let Some(v) = conn.required_headers {
                guard.row.required_headers = v;
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

    let (model, slot) = match ping::pick_test_model(&row) {
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

    let result = ping::probe_subscription(&state, &row, &model, slot).await;

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

/// 禁止用户自定义的保留头: 由 HTTP 栈/dispatch 层自动管理, 允许设置只会产生
/// 被覆盖的假象或破坏连接语义。以小写形式比较。
const RESERVED_HEADER_NAMES: &[&str] = &[
    "content-type",
    "content-length",
    "host",
    "transfer-encoding",
    "connection",
];
/// 额外 header 条数上限, 纯防御性 (真实网关场景 1~3 条)。
const MAX_REQUIRED_HEADERS: usize = 20;

/// 校验自定义订阅的额外出站 header。`auth_header_name` 须传「patch 后生效的」鉴权头名。
/// 用 reqwest::header::{HeaderName, HeaderValue} 解析 —— 与 dispatch 层七处注入点同类型,
/// 保证校验通过的 header 不会在出站时被静默跳过 (dispatch 对非法项是 if let Ok 静默 skip)。
fn validate_required_headers(
    headers: &BTreeMap<String, String>,
    auth_header_name: &str,
) -> AppResult<()> {
    use reqwest::header::{HeaderName, HeaderValue};

    if headers.len() > MAX_REQUIRED_HEADERS {
        return Err(AppError::BadRequest(format!(
            "额外 header 最多 {MAX_REQUIRED_HEADERS} 条 (当前 {} 条)",
            headers.len()
        )));
    }
    // dispatch 层空 auth 名回退 Authorization, 比较口径保持一致
    let auth_lower = if auth_header_name.is_empty() {
        "authorization".to_string()
    } else {
        auth_header_name.to_ascii_lowercase()
    };
    let mut seen = std::collections::HashSet::new();
    for (k, v) in headers {
        if k.trim().is_empty() {
            return Err(AppError::BadRequest("header 名不能为空".into()));
        }
        if HeaderName::try_from(k.as_str()).is_err() {
            return Err(AppError::BadRequest(format!(
                "header 名 \"{k}\" 含非法字符 (只允许字母、数字、连字符等 HTTP token 字符)"
            )));
        }
        let lower = k.to_ascii_lowercase();
        if RESERVED_HEADER_NAMES.contains(&lower.as_str()) {
            return Err(AppError::BadRequest(format!(
                "header \"{k}\" 是保留头, 由系统自动设置, 不能自定义"
            )));
        }
        if lower == auth_lower {
            return Err(AppError::BadRequest(format!(
                "header \"{k}\" 与鉴权 header 同名, 会覆盖鉴权信息, 请改用其他名称"
            )));
        }
        if !seen.insert(lower) {
            return Err(AppError::BadRequest(format!(
                "header \"{k}\" 与另一条仅大小写不同, 发送时会互相覆盖, 请合并为一条"
            )));
        }
        if v.trim().is_empty() {
            return Err(AppError::BadRequest(format!("header \"{k}\" 的值不能为空")));
        }
        if !v.is_ascii() {
            return Err(AppError::BadRequest(format!(
                "header \"{k}\" 的值含非 ASCII 字符 (如中文), HTTP header 值只能用可见 ASCII"
            )));
        }
        if HeaderValue::from_str(v).is_err() {
            return Err(AppError::BadRequest(format!(
                "header \"{k}\" 的值含非法字符 (不允许换行等控制字符)"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdrs(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn empty_map_is_ok() {
        // 清空场景合法 (整块替换语义下空 map = 清空)
        assert!(validate_required_headers(&BTreeMap::new(), "Authorization").is_ok());
    }

    #[test]
    fn issue_35_case_x_dst_is_ok() {
        assert!(validate_required_headers(&hdrs(&[("X-DST", "eastus2")]), "Authorization").is_ok());
    }

    #[test]
    fn more_than_20_entries_rejected() {
        let m: BTreeMap<String, String> = (0..21)
            .map(|i| (format!("X-H-{i}"), "v".to_string()))
            .collect();
        let err = validate_required_headers(&m, "Authorization").unwrap_err();
        assert!(err.to_string().contains("20"));
    }

    #[test]
    fn empty_key_rejected() {
        assert!(validate_required_headers(&hdrs(&[("", "v")]), "Authorization").is_err());
        assert!(validate_required_headers(&hdrs(&[("  ", "v")]), "Authorization").is_err());
    }

    #[test]
    fn key_with_space_rejected() {
        assert!(validate_required_headers(&hdrs(&[("X DST", "v")]), "Authorization").is_err());
    }

    #[test]
    fn key_with_non_ascii_rejected() {
        assert!(validate_required_headers(&hdrs(&[("头名", "v")]), "Authorization").is_err());
    }

    #[test]
    fn reserved_header_rejected_case_insensitive() {
        assert!(
            validate_required_headers(&hdrs(&[("Content-Type", "v")]), "Authorization").is_err()
        );
        assert!(validate_required_headers(&hdrs(&[("HOST", "v")]), "Authorization").is_err());
    }

    #[test]
    fn same_name_as_auth_header_rejected() {
        let err = validate_required_headers(&hdrs(&[("authorization", "v")]), "Authorization")
            .unwrap_err();
        assert!(err.to_string().contains("鉴权"));
    }

    #[test]
    fn empty_auth_name_falls_back_to_authorization() {
        // dispatch 层空 auth 名回退 Authorization (openai_responses_dispatch.rs:128-132), 校验口径一致
        assert!(validate_required_headers(&hdrs(&[("Authorization", "v")]), "").is_err());
    }

    #[test]
    fn case_insensitive_duplicate_keys_rejected() {
        // BTreeMap 允许并存, 但 HeaderMap insert 大小写不敏感会互相覆盖
        assert!(
            validate_required_headers(&hdrs(&[("X-DST", "a"), ("x-dst", "b")]), "Authorization")
                .is_err()
        );
    }

    #[test]
    fn empty_value_rejected() {
        assert!(validate_required_headers(&hdrs(&[("X-DST", "")]), "Authorization").is_err());
        assert!(validate_required_headers(&hdrs(&[("X-DST", "  ")]), "Authorization").is_err());
    }

    #[test]
    fn non_ascii_value_rejected() {
        // 注意: HeaderValue::from_str 对 0x80+ 字节其实放行 (按 opaque bytes 发送),
        // 所以必须显式 is_ascii() 检查, 不能只靠解析
        assert!(validate_required_headers(&hdrs(&[("X-DST", "东区")]), "Authorization").is_err());
    }

    #[test]
    fn control_char_value_rejected() {
        assert!(validate_required_headers(&hdrs(&[("X-DST", "a\nb")]), "Authorization").is_err());
    }

    #[test]
    fn value_with_surrounding_spaces_ok() {
        // trim 仅用于判空, 非空值带空格放行、存原样
        assert!(
            validate_required_headers(&hdrs(&[("X-DST", " eastus2 ")]), "Authorization").is_ok()
        );
    }
}
