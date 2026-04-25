use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tauri::State;
use tokio::sync::RwLock;
use tracing::warn;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::subscription::{
    model::{
        ModelCache, ModelInfo, ModelSlots, SubscriptionDto, SubscriptionRow, SubscriptionRuntime,
    },
    model_discovery, ping, state_machine, store,
};
use crate::virtual_model::VirtualModelName;

#[derive(Debug, Deserialize)]
pub struct CreateSubscriptionInput {
    pub provider_id: String,
    pub endpoint_id: String,
    pub display_name: String,
    pub api_key: String,
    pub model_slots: ModelSlots,
}

#[derive(Debug, Deserialize, Default)]
pub struct SubscriptionPatch {
    pub endpoint_id: Option<String>,
    pub display_name: Option<String>,
    pub model_slots: Option<ModelSlots>,
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
    // 校验 provider / endpoint 存在
    let provider = state
        .providers
        .get(&input.provider_id)
        .ok_or_else(|| AppError::ProviderNotFound(input.provider_id.clone()))?;
    if provider.endpoint(&input.endpoint_id).is_none() {
        return Err(AppError::EndpointNotFound(input.endpoint_id));
    }

    let id = Uuid::new_v4();
    let now = Utc::now();
    let row = SubscriptionRow {
        id,
        provider_id: input.provider_id,
        endpoint_id: input.endpoint_id,
        display_name: input.display_name,
        api_key: input.api_key,
        model_slots: input.model_slots,
        enabled: true,
        is_auth_failed: false,
        last_error_message: None,
        created_at: now,
        updated_at: now,
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

    {
        let mut guard = rt.write().await;
        if let Some(name) = patch.display_name {
            guard.row.display_name = name;
        }
        if let Some(ep) = patch.endpoint_id {
            let provider = state
                .providers
                .get(&guard.row.provider_id)
                .ok_or_else(|| AppError::ProviderNotFound(guard.row.provider_id.clone()))?;
            if provider.endpoint(&ep).is_none() {
                return Err(AppError::EndpointNotFound(ep));
            }
            guard.row.endpoint_id = ep;
        }
        if let Some(slots) = patch.model_slots {
            guard.row.model_slots = slots;
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

    // 同步更新内存中的 row.api_key
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
        // 触发状态机：auth_failed → healthy
        let _ = state_machine::apply(
            &state.db,
            &state.app_handle,
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
    // 从内存先移除
    {
        let mut subs = state.subscriptions.write().await;
        subs.remove(&id);
    }
    // 从虚拟模型绑定中移除
    {
        let mut vms = state.virtual_models.write().await;
        for vm in vms.values_mut() {
            vm.subscription_ids.retain(|x| *x != id);
        }
    }
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
    store::update_enabled(&state.db, &id, enabled).await?;

    let rt = {
        let subs = state.subscriptions.read().await;
        subs.get(&id)
            .cloned()
            .ok_or_else(|| AppError::SubscriptionNotFound(id.to_string()))?
    };

    let event = if enabled {
        state_machine::Event::UserEnable
    } else {
        state_machine::Event::UserDisable
    };
    let _ = state_machine::apply(&state.db, &state.app_handle, rt, event).await;
    Ok(())
}

/// 测试一条订阅的真实可达性: 用最小 prompt 直接打 messages 端点。
///
/// 设计:
/// - 不打 /models — 避免 URL 拼接歧义和 enabled:false provider 无法测的限制。
/// - 通过 → 触发 `Event::UserManualReset` 一键复活订阅(任意非 Disabled 状态)。
/// - 失败 → 仅回显错误, 不发负面 event 给状态机(避免 false negative 打坏健康订阅;
///   生产真实流量会自然走状态机)。
///
/// 真正的请求构造和发送由 `subscription::ping::probe` 完成,与后台巡检 worker 共用。
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
    let (provider_id, endpoint_id, api_key, slots) = {
        let g = rt.read().await;
        (
            g.row.provider_id.clone(),
            g.row.endpoint_id.clone(),
            g.row.api_key.clone(),
            g.row.model_slots.clone(),
        )
    };
    let provider = state
        .providers
        .get(&provider_id)
        .ok_or_else(|| AppError::ProviderNotFound(provider_id.clone()))?
        .clone();
    let endpoint = provider
        .endpoint(&endpoint_id)
        .ok_or_else(|| AppError::EndpointNotFound(endpoint_id.clone()))?
        .clone();

    let model = match ping::pick_test_model(&slots, &provider.model_discovery.example_models) {
        Some(m) => m,
        None => {
            return Ok(TestConnectionResult {
                ok: false,
                message: "订阅未配置任何 model 槽位, 且 provider 无 example_models, 无法测试".into(),
                http_status: None,
                model_used: None,
                state_reset: false,
            });
        }
    };

    let result = ping::probe(&state.probe_client, &provider, &endpoint, &api_key, &model).await;

    let mut state_reset = false;
    if result.ok {
        match state_machine::apply(
            &state.db,
            &state.app_handle,
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
    let (provider_id, endpoint_id, api_key) = {
        let g = rt.read().await;
        (g.row.provider_id.clone(), g.row.endpoint_id.clone(), g.row.api_key.clone())
    };
    let provider = state
        .providers
        .get(&provider_id)
        .ok_or_else(|| AppError::ProviderNotFound(provider_id.clone()))?
        .clone();

    match model_discovery::fetch_and_cache(
        &state.db,
        &state.http_client,
        &provider,
        &endpoint_id,
        &id,
        &api_key,
    )
    .await
    {
        Ok(cache) => {
            // 更新内存缓存
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

// 保留 _ 避免 unused warning
#[allow(dead_code)]
fn _unused(_: VirtualModelName) {}
