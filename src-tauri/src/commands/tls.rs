//! TLS 证书管理 Tauri 命令. 前端 Settings 页 HTTPS 区域调用.

use std::path::PathBuf;

use tauri::State;

use crate::db::paths;
use crate::error::AppResult;
use crate::state::AppState;
use crate::tls;
use crate::tls::TlsStatus;

#[tauri::command]
pub async fn tls_get_status(state: State<'_, AppState>) -> AppResult<TlsStatus> {
    let app_data_dir = paths::app_data_dir(&state.app_handle)?;
    tls::read_status(&app_data_dir).await
}

#[tauri::command]
pub async fn tls_get_ca_pem_path(state: State<'_, AppState>) -> AppResult<String> {
    let app_data_dir = paths::app_data_dir(&state.app_handle)?;
    Ok(tls::ca_pem_path(&app_data_dir).to_string_lossy().to_string())
}

#[tauri::command]
pub async fn tls_export_ca_pem(
    state: State<'_, AppState>,
    dest: String,
) -> AppResult<()> {
    let app_data_dir = paths::app_data_dir(&state.app_handle)?;
    let dest_path = PathBuf::from(dest);
    // HTTP-only 模式下用户提前导出 CA, 这里按需生成 (跳过 leaf + ServerConfig 构建).
    tls::ensure_ca(&app_data_dir).await?;
    tls::export_ca_pem(&app_data_dir, &dest_path).await
}

#[tauri::command]
pub async fn tls_regenerate_leaf(state: State<'_, AppState>) -> AppResult<TlsStatus> {
    let app_data_dir = paths::app_data_dir(&state.app_handle)?;
    tls::ensure_ca(&app_data_dir).await?;
    tls::regenerate_leaf(&app_data_dir).await?;
    tls::read_status(&app_data_dir).await
}
