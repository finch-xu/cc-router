//! 插件 command (主题 / 小票 / 系统插件)。
//!
//! `list_plugins` / `load_plugin` / `disable_plugin` 网页界面也能用 (显示已安装的主题; 停用是安全方向);
//! 其余会下载、改宿主机文件或启用会改变 app 行为的插件, 在 `proxy/web/api.rs::web_commands!` 里是
//! `DESKTOP_ONLY` 拒绝桩, 由 `host_mutating_plugin_commands_are_refusal_stubs_here` 扫源码锁住。

use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use tauri::{Emitter, State};

use crate::db::paths;
use crate::error::{AppError, AppResult};
use crate::plugins::store::{self, InstalledPlugin, PluginContent, StagedPlugin};
use crate::plugins::{config, runtime};
use cc_router_plugin_spec::plugin::is_system_type;
use crate::plugins::{github, link, PluginSource, SourceKind, COMMUNITY_INDEX_URL, MANIFEST_FILE, MAX_ENTRY_BYTES, MAX_FILE_BYTES, MAX_INDEX_BYTES, MAX_MANIFEST_BYTES, MAX_TOTAL_BYTES};
use crate::state::AppState;

/// 已安装插件变了 (装 / 卸 / 更新): 各端刷新列表; 正在用的主题被卸载时前端回退到默认主题。
pub const CHANGED_EVENT: &str = "plugins_changed";

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T> {
    tokio::task::spawn_blocking(f).await.map_err(|e| AppError::internal(e.to_string()))?
}

/// 已安装列表的一项: 安装记录 + 生效状态 (系统插件看 `plugin_configs`, 主题 / 小票恒为 true)。
#[derive(Debug, Serialize)]
pub struct InstalledPluginView {
    #[serde(flatten)]
    pub record: InstalledPlugin,
    pub enabled: bool,
}

#[tauri::command]
pub async fn list_plugins(state: State<'_, AppState>) -> AppResult<Vec<InstalledPluginView>> {
    let dir = paths::app_data_dir(&state.app_handle)?;
    let configs = config::load_all(&state.db).await?;
    let list = blocking(move || store::list(&dir)).await?;
    Ok(list
        .into_iter()
        .map(|record| {
            let enabled = !is_system_type(&record.plugin_type) || config::is_enabled(&configs, &record.id, &record.plugin_type);
            InstalledPluginView { record, enabled }
        })
        .collect())
}

/// 系统插件装 / 卸 / 启停之后: 重建运行时 (厂商注册表 + 计价注册表) 并通知各端。
async fn after_change(state: &AppState) {
    if let Err(e) = runtime::rebuild_runtime(state).await {
        tracing::warn!(error = %e, "插件变更后重建系统插件运行时失败");
    }
    let _ = state.app_handle.emit(CHANGED_EVENT, ());
}

async fn installed_system_plugin(state: &AppState, id: &str) -> AppResult<InstalledPlugin> {
    let dir = paths::app_data_dir(&state.app_handle)?;
    let id_owned = id.to_string();
    let rec = blocking(move || store::list(&dir))
        .await?
        .into_iter()
        .find(|p| p.id == id_owned)
        .ok_or_else(|| AppError::BadRequest(format!("没有安装插件 {id}")))?;
    if !is_system_type(&rec.plugin_type) {
        return Err(AppError::BadRequest("这类插件不需要启用, 在主题 / 小票选择里直接使用".into()));
    }
    Ok(rec)
}

/// 启用系统插件。会改变 app 行为, 只能在桌面窗口里调用 (网页界面是拒绝桩)。
#[tauri::command]
pub async fn enable_plugin(state: State<'_, AppState>, id: String) -> AppResult<()> {
    installed_system_plugin(&state, &id).await?;
    config::set_enabled(&state.db, &id, true).await?;
    after_change(&state).await;
    Ok(())
}

/// 停用系统插件 (安全方向, 网页界面也能用)。
#[tauri::command]
pub async fn disable_plugin(state: State<'_, AppState>, id: String) -> AppResult<()> {
    installed_system_plugin(&state, &id).await?;
    config::set_enabled(&state.db, &id, false).await?;
    after_change(&state).await;
    Ok(())
}

#[tauri::command]
pub async fn load_plugin(state: State<'_, AppState>, id: String) -> AppResult<PluginContent> {
    let dir = paths::app_data_dir(&state.app_handle)?;
    blocking(move || store::load(&dir, &id)).await
}

/// 从 GitHub 链接下载到暂存区。不安装: 前端校验内容、用户确认后再调 `install_staged_plugin`。
#[tauri::command]
pub async fn stage_plugin_from_github(state: State<'_, AppState>, link: String) -> AppResult<StagedPlugin> {
    let r = link::parse(&link).map_err(AppError::BadRequest)?;
    let client = &state.http_client;
    let sha = github::resolve_commit(client, &r).await?;

    let manifest = github::fetch_file(client, &r, &sha, MANIFEST_FILE, MAX_MANIFEST_BYTES)
        .await?
        .ok_or_else(|| AppError::BadRequest(format!("这个位置没有 {MANIFEST_FILE}, 不是 cc-router 插件")))?;
    let (core, _) = crate::plugins::parse_manifest(&manifest)?;
    let mut files = store::Files::new();
    files.insert(MANIFEST_FILE.into(), manifest);
    let need = |path: &str, bytes: Option<Vec<u8>>| bytes.ok_or_else(|| AppError::BadRequest(format!("仓库里缺少清单列出的文件 {path}")));
    let entry = github::fetch_file(client, &r, &sha, &core.entry, MAX_ENTRY_BYTES).await?;
    files.insert(core.entry.clone(), need(&core.entry, entry)?);
    let mut total: u64 = files.values().map(|b| b.len() as u64).sum();
    for a in &core.assets {
        let bytes = need(a, github::fetch_file(client, &r, &sha, a, MAX_FILE_BYTES).await?)?;
        total += bytes.len() as u64;
        if total > MAX_TOTAL_BYTES {
            return Err(AppError::BadRequest(format!("插件总大小超过 {} MB", MAX_TOTAL_BYTES / 1024 / 1024)));
        }
        files.insert(a.clone(), bytes);
    }

    let source = PluginSource { kind: SourceKind::Github, location: format!("{}/{}", r.owner, r.repo), path: r.path.clone(), commit: Some(sha) };
    let dir = paths::app_data_dir(&state.app_handle)?;
    blocking(move || store::stage(&dir, files, source)).await
}

/// 从本地文件夹读到暂存区 (作者调试用)。
#[tauri::command]
pub async fn stage_plugin_from_dir(state: State<'_, AppState>, path: String) -> AppResult<StagedPlugin> {
    let dir = paths::app_data_dir(&state.app_handle)?;
    blocking(move || {
        let folder = Path::new(&path);
        if !folder.is_absolute() {
            return Err(AppError::BadRequest("请选择一个文件夹".into()));
        }
        let files = store::read_dir_package(folder)?;
        let source = PluginSource { kind: SourceKind::Local, location: folder.display().to_string(), path: None, commit: None };
        store::stage(&dir, files, source)
    })
    .await
}

/// `community_sha256`: 从社区目录安装时传目录里记录的包哈希, 不一致即拒绝。
#[tauri::command]
pub async fn install_staged_plugin(
    state: State<'_, AppState>,
    staging_id: String,
    community_sha256: Option<String>,
) -> AppResult<InstalledPlugin> {
    let dir = paths::app_data_dir(&state.app_handle)?;
    let rec = blocking(move || store::install(&dir, &staging_id, community_sha256.as_deref())).await?;
    after_change(&state).await;
    Ok(rec)
}

#[tauri::command]
pub async fn discard_staged_plugin(staging_id: String) -> AppResult<()> {
    blocking(move || {
        store::discard(&staging_id);
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn uninstall_plugin(state: State<'_, AppState>, id: String) -> AppResult<()> {
    let dir = paths::app_data_dir(&state.app_handle)?;
    let id_owned = id.clone();
    blocking(move || store::uninstall(&dir, &id_owned)).await?;
    config::delete(&state.db, &id).await?;
    after_change(&state).await;
    Ok(())
}

/// 社区目录 `index.json` 原样返回 (前端宽松解析: 不认识的条目跳过)。
#[tauri::command]
pub async fn fetch_community_index(state: State<'_, AppState>) -> AppResult<Value> {
    let bytes = github::fetch_limited(&state.http_client, COMMUNITY_INDEX_URL, MAX_INDEX_BYTES)
        .await?
        .ok_or_else(|| AppError::BadGateway("社区目录暂时不可用".into()))?;
    serde_json::from_slice(&bytes).map_err(|_| AppError::BadGateway("社区目录格式不对".into()))
}
