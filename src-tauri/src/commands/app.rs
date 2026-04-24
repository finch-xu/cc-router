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

    // 2. 删除 DB / settings 文件
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
