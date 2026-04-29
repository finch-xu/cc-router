//! 更新器 command:运行时按 Settings::update_source 切换 manifest 源。
//!
//! 不直接用 `@tauri-apps/plugin-updater` 的 JS API,因为 plugin 注册时就锁定了
//! tauri.conf.json::endpoints,运行时无法热切。改用 `app.updater_builder()` 每次
//! 现场构造一次性 builder,把 endpoints 注入进去——切源立即生效,无需重启。

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_updater::UpdaterExt;
use tracing::{info, warn};

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::updater_source::manifest_url_for;

/// 给前端返回的版本元数据,精简自 `tauri_plugin_updater::Update`。
#[derive(Debug, Serialize, Clone)]
pub struct UpdateInfo {
    pub version: String,
    pub current_version: String,
    pub body: Option<String>,
}

/// `updater://progress` 事件 payload。
///
/// 流程:Started(一次) → Progress(N 次) → Finished(一次)。
#[derive(Debug, Serialize, Clone)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum UpdaterProgress {
    Started { content_length: Option<u64> },
    Progress { chunk_length: u64 },
    Finished,
}

const PROGRESS_EVENT: &str = "updater://progress";

/// 根据当前 settings 构造一次性 Updater 实例。每次 check/install 都现造一次。
async fn build_updater(
    app: &AppHandle,
    state: &State<'_, AppState>,
) -> AppResult<tauri_plugin_updater::Updater> {
    let source = state.settings.read().await.update_source.clone();
    let mut builder = app.updater_builder();
    if let Some(url) = manifest_url_for(source.as_deref()) {
        let parsed = url
            .parse()
            .map_err(|e| AppError::internal(format!("invalid manifest URL '{url}': {e}")))?;
        builder = builder
            .endpoints(vec![parsed])
            .map_err(|e| AppError::internal(format!("set updater endpoints: {e}")))?;
        info!(?source, %url, "updater endpoint overridden");
    } else {
        info!("updater endpoint using tauri.conf.json default");
    }
    builder
        .build()
        .map_err(|e| AppError::internal(format!("build updater: {e}")))
}

/// 检查更新。返回 `None` 表示已是最新。
#[tauri::command]
pub async fn check_for_update(
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<Option<UpdateInfo>> {
    let updater = build_updater(&app, &state).await?;
    let result = updater
        .check()
        .await
        .map_err(|e| AppError::internal(format!("check updates: {e}")))?;
    Ok(result.map(|u| UpdateInfo {
        version: u.version.clone(),
        current_version: u.current_version.clone(),
        body: u.body.clone(),
    }))
}

/// 下载并安装更新。
///
/// 进度通过 `updater://progress` 事件流推送。完成后**不**自动重启,
/// 由前端调用现有的 `relaunch_app` command 由用户决定时机
/// (cc-router 是常驻代理,重启会中断 CC 当前会话)。
#[tauri::command]
pub async fn download_install_update(
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let updater = build_updater(&app, &state).await?;
    let Some(update) = updater
        .check()
        .await
        .map_err(|e| AppError::internal(format!("check updates: {e}")))?
    else {
        warn!("download_install_update called but no update available");
        return Ok(());
    };

    let app_started = app.clone();
    let started_once = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let started_flag = started_once.clone();
    let app_progress = app.clone();
    let app_finish = app.clone();

    update
        .download_and_install(
            move |chunk_length, content_length| {
                // 第一次回调时先 emit Started,再 emit 第一个 Progress
                if !started_flag.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    let _ = app_started.emit(
                        PROGRESS_EVENT,
                        UpdaterProgress::Started { content_length },
                    );
                }
                let _ = app_progress.emit(
                    PROGRESS_EVENT,
                    UpdaterProgress::Progress {
                        chunk_length: chunk_length as u64,
                    },
                );
            },
            move || {
                let _ = app_finish.emit(PROGRESS_EVENT, UpdaterProgress::Finished);
            },
        )
        .await
        .map_err(|e| AppError::internal(format!("download/install update: {e}")))?;
    Ok(())
}
