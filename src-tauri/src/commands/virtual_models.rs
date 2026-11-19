use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::virtual_model::model::{RoutingMode, VirtualModelName};
use crate::virtual_model::store;

#[derive(Debug, Serialize)]
pub struct VirtualModelDto {
    pub name: String,
    pub mode: RoutingMode,
    pub subscription_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateVirtualModelInput {
    pub mode: RoutingMode,
    pub subscription_ids: Vec<String>,
}

#[tauri::command]
pub async fn list_virtual_models(state: State<'_, AppState>) -> AppResult<Vec<VirtualModelDto>> {
    let guard = state.virtual_models.read().await;
    let mut out = Vec::with_capacity(VirtualModelName::all().len());
    for name in VirtualModelName::all() {
        if let Some(cfg) = guard.get(&name) {
            out.push(VirtualModelDto {
                name: cfg.name.as_str().to_string(),
                mode: cfg.mode,
                subscription_ids: cfg.subscription_ids.iter().map(|id| id.to_string()).collect(),
            });
        }
    }
    Ok(out)
}

#[tauri::command]
pub async fn update_virtual_model(
    state: State<'_, AppState>,
    name: String,
    input: UpdateVirtualModelInput,
) -> AppResult<()> {
    let vm_name = VirtualModelName::parse(&name)
        .ok_or_else(|| AppError::UnknownVirtualModel(name.clone()))?;

    let ids: Result<Vec<Uuid>, _> = input
        .subscription_ids
        .iter()
        .map(|s| Uuid::parse_str(s))
        .collect();
    let ids = ids.map_err(|e| AppError::BadRequest(format!("invalid uuid: {e}")))?;

    let _edit = state.route_edits.lock().await;

    // 校验: 所有 subscription 必须存在, 且端点协议与虚拟模型匹配 (model-jev ↔ systemone)。
    // 桌面 / 网页 / TUI 共用本命令, 前端过滤只是体验, 这里才是隔离边界。
    {
        let subs = state.subscriptions.read().await;
        for id in &ids {
            let rt = subs
                .get(id)
                .ok_or_else(|| AppError::SubscriptionNotFound(id.to_string()))?;
            let g = rt.read().await;
            if let Some(reason) = vm_name.binding_rejection(g.row.endpoint_protocol, &g.row.display_name) {
                return Err(AppError::BadRequest(reason));
            }
        }
    }

    // 去重保序
    let mut seen = std::collections::HashSet::new();
    let deduped: Vec<Uuid> = ids
        .into_iter()
        .filter(|id| seen.insert(*id))
        .collect();

    let mut tx = state.db.begin().await?;
    store::save_mode_on(&mut tx, &vm_name, input.mode).await?;
    store::save_bindings_on(&mut tx, &vm_name, &deduped).await?;
    tx.commit().await?;

    let mut guard = state.virtual_models.write().await;
    if let Some(cfg) = guard.get_mut(&vm_name) {
        cfg.mode = input.mode;
        cfg.subscription_ids = deduped;
        cfg.last_used_index = 0;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomTarget {
    pub subscription_id: Uuid,
    pub upstream_model_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CustomVirtualModelInput {
    pub mode: RoutingMode,
    pub targets: Vec<CustomTarget>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CustomVirtualModelDto {
    pub name: String,
    #[serde(flatten)]
    pub input: CustomVirtualModelInput,
}

#[tauri::command]
pub async fn list_custom_virtual_models(state: State<'_, AppState>) -> AppResult<Vec<CustomVirtualModelDto>> {
    let models = state.virtual_models.read().await;
    let mut result: Vec<_> = models.values().filter(|model| model.name.is_custom()).map(|model| {
        CustomVirtualModelDto {
            name: model.name.as_str().to_owned(),
            input: CustomVirtualModelInput {
                mode: model.mode,
                targets: model.subscription_ids.iter().filter_map(|id| {
                    model.upstream_models.get(id).map(|upstream| CustomTarget {
                        subscription_id: *id,
                        upstream_model_id: upstream.clone(),
                    })
                }).collect(),
            },
        }
    }).collect();
    result.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(result)
}

#[tauri::command]
pub async fn create_custom_virtual_model(
    state: State<'_, AppState>,
    model: CustomVirtualModelDto,
) -> AppResult<()> {
    save_custom_model(&state, &model.name, model.input, true).await
}

#[tauri::command]
pub async fn update_custom_virtual_model(
    state: State<'_, AppState>,
    name: String,
    input: CustomVirtualModelInput,
) -> AppResult<()> {
    save_custom_model(&state, &name, input, false).await
}

async fn save_custom_model(
    state: &AppState,
    name: &str,
    input: CustomVirtualModelInput,
    create: bool,
) -> AppResult<()> {
    use crate::virtual_model::model::VirtualModelConfig;

    let name = VirtualModelName::custom(name)
        .ok_or_else(|| AppError::BadRequest("Invalid or reserved custom model name".into()))?;
    if input.targets.is_empty() {
        return Err(AppError::BadRequest("Add at least one target".into()));
    }
    let _edit = state.route_edits.lock().await;
    let subscriptions = state.subscriptions.read().await;
    let mut ids = Vec::with_capacity(input.targets.len());
    let mut upstream_models = std::collections::HashMap::new();
    for target in input.targets {
        let id = target.subscription_id;
        let sub = subscriptions.get(&id)
            .ok_or_else(|| AppError::SubscriptionNotFound(id.to_string()))?;
        let sub = sub.read().await;
        if let Some(reason) = name.binding_rejection(sub.row.endpoint_protocol, &sub.row.display_name) {
            return Err(AppError::BadRequest(reason));
        }
        let upstream = target.upstream_model_id.trim();
        if upstream.is_empty() || upstream.chars().count() > 160 {
            return Err(AppError::BadRequest("Enter an upstream model ID of 1–160 characters".into()));
        }
        if upstream_models.insert(id, upstream.to_owned()).is_some() {
            return Err(AppError::BadRequest("A subscription may appear only once per route".into()));
        }
        ids.push(id);
    }
    drop(subscriptions);

    let mut models = state.virtual_models.write().await;
    if models.contains_key(&name) == create {
        return Err(if create {
            AppError::BadRequest("Custom model name is already in use".into())
        } else {
            AppError::UnknownVirtualModel(name.as_str().to_owned())
        });
    }
    let mut tx = state.db.begin().await?;
    store::save_mode_on(&mut tx, &name, input.mode).await?;
    store::save_targets_on(&mut tx, &name, &ids, &upstream_models).await?;
    tx.commit().await?;
    models.insert(name.clone(), VirtualModelConfig {
        name,
        mode: input.mode,
        subscription_ids: ids,
        upstream_models,
        last_used_index: 0,
    });
    Ok(())
}

#[tauri::command]
pub async fn delete_custom_virtual_model(state: State<'_, AppState>, name: String) -> AppResult<()> {
    let vm = VirtualModelName::custom(&name).ok_or_else(|| AppError::UnknownVirtualModel(name))?;
    let _edit = state.route_edits.lock().await;
    let mut models = state.virtual_models.write().await;
    if !models.contains_key(&vm) {
        return Err(AppError::UnknownVirtualModel(vm.as_str().to_owned()));
    }
    let mut tx = state.db.begin().await?;
    store::save_bindings_on(&mut tx, &vm, &[]).await?;
    sqlx::query("DELETE FROM virtual_model_config WHERE virtual_model_name = ?")
        .bind(vm.as_str()).execute(&mut *tx).await?;
    tx.commit().await?;
    models.remove(&vm);
    state.session_affinity.lock().unwrap_or_else(|e| e.into_inner()).clear_model(&vm);
    Ok(())
}
