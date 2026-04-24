use tauri::State;

use crate::db::paths;
use crate::error::AppResult;
use crate::settings::model::{Settings, SettingsPatch};
use crate::settings::save;
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
    let mut guard = state.settings.write().await;
    guard.apply_patch(patch);
    let app_data_dir = paths::app_data_dir(&state.app_handle)?;
    save(&app_data_dir, &guard).await?;
    Ok(guard.clone())
}
