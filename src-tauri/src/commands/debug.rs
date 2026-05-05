//! 调试模式相关命令: 打开 dump 目录 / 清空 dump 目录.
//!
//! 这两个命令仅给 Settings 页"调试"分组的两个按钮使用. 不依赖 debug_mode 当前
//! 是否开启 — 即使关闭, 用户仍可清理之前累积的 dump 文件.

use tauri::AppHandle;
use tracing::info;

use crate::db::paths;
use crate::error::{AppError, AppResult};
use crate::observability::body_dump;

/// 打开系统文件管理器到 debug-dumps 目录. 目录不存在时先创建.
#[tauri::command]
pub async fn open_debug_dump_dir(app: AppHandle) -> AppResult<()> {
    let app_data_dir = paths::app_data_dir(&app)?;
    let dir = body_dump::dump_dir(&app_data_dir);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::internal(format!("创建 debug-dumps 目录失败: {e}")))?;

    let path_str = dir.to_string_lossy().to_string();
    let result = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(&path_str).spawn()
    } else if cfg!(target_os = "windows") {
        std::process::Command::new("explorer").arg(&path_str).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(&path_str).spawn()
    };

    result.map_err(|e| AppError::internal(format!("打开目录失败: {e}")))?;
    info!(path = %path_str, "opened debug-dumps directory");
    Ok(())
}

/// 清空 debug-dumps 目录下的所有内容. 删整个目录再重建, 保持后续 dump 写盘行为不变.
#[tauri::command]
pub async fn clear_debug_dumps(app: AppHandle) -> AppResult<()> {
    let app_data_dir = paths::app_data_dir(&app)?;
    let dir = body_dump::dump_dir(&app_data_dir);
    if dir.exists() {
        tokio::fs::remove_dir_all(&dir)
            .await
            .map_err(|e| AppError::internal(format!("清空 debug-dumps 失败: {e}")))?;
    }
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::internal(format!("重建 debug-dumps 目录失败: {e}")))?;
    info!("cleared debug-dumps directory");
    Ok(())
}
