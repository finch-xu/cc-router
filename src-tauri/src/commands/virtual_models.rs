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
    let mut out = Vec::with_capacity(3);
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

    // 校验：所有 subscription 必须存在
    {
        let subs = state.subscriptions.read().await;
        for id in &ids {
            if !subs.contains_key(id) {
                return Err(AppError::SubscriptionNotFound(id.to_string()));
            }
        }
    }

    // 去重保序
    let mut seen = std::collections::HashSet::new();
    let deduped: Vec<Uuid> = ids
        .into_iter()
        .filter(|id| seen.insert(*id))
        .collect();

    store::save_mode(&state.db, vm_name, input.mode).await?;
    store::save_bindings(&state.db, vm_name, &deduped).await?;

    let mut guard = state.virtual_models.write().await;
    if let Some(cfg) = guard.get_mut(&vm_name) {
        cfg.mode = input.mode;
        cfg.subscription_ids = deduped;
        cfg.last_used_index = 0;
    }
    Ok(())
}
