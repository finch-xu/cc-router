use tauri::State;

use crate::db::paths;
use crate::error::AppResult;
use crate::settings::model::{Settings, SettingsPatch};
use crate::settings::{generate_token, save};
use crate::state::AppState;

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> AppResult<Settings> {
    Ok(state.settings.read().await.clone())
}

#[tauri::command]
pub async fn update_settings(
    state: State<'_, AppState>,
    patch: SettingsPatch,
) -> AppResult<Settings> {
    let dock_change = patch.hide_dock_icon;
    let mut guard = state.settings.write().await;
    guard.apply_patch(patch);
    let app_data_dir = paths::app_data_dir(&state.app_handle)?;
    save(&app_data_dir, &guard).await?;
    let snapshot = guard.clone();
    drop(guard);

    if let Some(hide) = dock_change {
        crate::apply_dock_visibility(&state.app_handle, hide);
    }

    Ok(snapshot)
}

/// 重新生成 auth_token 并立即持久化。返回新 settings 让前端拿到新 token 显示。
#[tauri::command]
pub async fn generate_new_token(state: State<'_, AppState>) -> AppResult<Settings> {
    let mut guard = state.settings.write().await;
    guard.auth_token = generate_token();
    let app_data_dir = paths::app_data_dir(&state.app_handle)?;
    save(&app_data_dir, &guard).await?;
    Ok(guard.clone())
}
