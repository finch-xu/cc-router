use std::path::PathBuf;

use tauri::Manager;

use crate::error::{AppError, AppResult};

pub fn app_data_dir(app: &tauri::AppHandle) -> AppResult<PathBuf> {
    app.path()
        .app_data_dir()
        .map_err(|e| AppError::internal(format!("无法获取 app_data_dir: {e}")))
}

pub fn resource_dir(app: &tauri::AppHandle) -> AppResult<PathBuf> {
    app.path()
        .resource_dir()
        .map_err(|e| AppError::internal(format!("无法获取 resource_dir: {e}")))
}
