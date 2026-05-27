//! 应用级命令（factory reset 等）。

use tauri::{AppHandle, State};
use tracing::{info, warn};

use crate::db::paths;
use crate::error::AppResult;
use crate::state::AppState;

/// 恢复出厂设置：删除 config.db + settings.json，然后重启 app。
/// API Key 明文存储在 config.db 里，删库即删 key。
/// 下次启动时会重新走 migration 和 onboarding 引导。
#[tauri::command]
pub async fn factory_reset(state: State<'_, AppState>, app: AppHandle) -> AppResult<()> {
    info!("factory reset initiated");

    // 1. 关闭 DB pool（让 wal checkpoint 和连接释放）
    state.db.close().await;

    // 2. 顺便禁用 OS 级登录项 (LaunchAgent / Registry / .desktop), 否则 settings.json 已删
    //    但下次开机仍会自启动, 与 UI 显示的 false 背离.
    {
        use tauri_plugin_autostart::ManagerExt;
        if let Err(e) = app.autolaunch().disable() {
            warn!(?e, "factory reset: failed to disable autostart");
        }
    }

    // 3. 删除 DB / settings 文件
    let app_data_dir = paths::app_data_dir(&app)?;
    for name in ["config.db", "config.db-wal", "config.db-shm", "settings.json"] {
        let path = app_data_dir.join(name);
        if path.exists() {
            if let Err(e) = tokio::fs::remove_file(&path).await {
                warn!(?e, ?path, "failed to remove");
            }
        }
    }

    info!("factory reset finished, restarting app");
    app.restart();
}

/// 当前 Linux 进程是否运行在 AppImage 中。
/// AppImage 启动时会向自身导出 `APPIMAGE` 环境变量,Tauri updater 也用同样方式判断。
/// 前端用此命令决定 Linux 平台是走自动更新流程,还是引导用户去 GitHub release 页手动下载 .deb。
#[tauri::command]
pub fn is_appimage_runtime() -> bool {
    std::env::var("APPIMAGE").is_ok()
}

/// 用户在 About 页确认"立即重启"后调用,等同 factory reset 里的 app.restart()。
/// 单独暴露是为了避免前端引入 tauri-plugin-process 依赖——保持依赖最小化。
#[tauri::command]
pub fn relaunch_app(app: AppHandle) {
    info!("relaunch requested by user (post-update)");
    app.restart();
}
