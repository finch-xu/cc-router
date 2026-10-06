//! 配置导入导出的 Tauri command。设计稿: docs/superpowers/specs/2026-09-30-config-export-import-design.md

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};
use tokio::sync::RwLock;
use zeroize::Zeroizing;

use crate::backup::crypto::{self, KdfCost};
use crate::backup::export::{build_export, SecretOptions, VirtualModelSnapshot};
use crate::backup::format::parse_file;
use crate::backup::import::{self, ImportPreview, ImportReport, LocalState};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::subscription::model::{SubscriptionRow, SubscriptionRuntime};

/// 同一时刻只允许一个导入在跑 (两个导入交错会在「预览 → 应用」之间互相改掉本机状态)。
static BUSY: AtomicBool = AtomicBool::new(false);

struct BusyGuard;

impl BusyGuard {
    fn acquire() -> Option<Self> {
        BUSY.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).ok().map(|_| Self)
    }
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::SeqCst);
    }
}

#[derive(Debug, Serialize)]
pub struct ExportSummary {
    pub subscriptions: usize,
    pub with_secrets: bool,
}

#[derive(Default, Deserialize)]
pub struct ImportOptions {
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub skip_secrets: bool,
    #[serde(default)]
    pub import_token: bool,
}

// Hand-written so the export password can never end up in a log line via `{:?}`.
impl std::fmt::Debug for ImportOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportOptions")
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("skip_secrets", &self.skip_secrets)
            .field("import_token", &self.import_token)
            .finish()
    }
}

fn is_valid_token(t: &str) -> bool {
    !t.is_empty() && t.len() <= 256 && t.bytes().all(|b| b.is_ascii_graphic())
}

async fn snapshot_rows(state: &AppState) -> Vec<SubscriptionRow> {
    let subs = state.subscriptions.read().await;
    let mut out = Vec::with_capacity(subs.len());
    for rt in subs.values() {
        out.push(rt.read().await.row.clone());
    }
    out
}

async fn snapshot_vms(state: &AppState) -> Vec<VirtualModelSnapshot> {
    let guard = state.virtual_models.read().await;
    guard
        .values()
        .map(|c| VirtualModelSnapshot { name: c.name, mode: c.mode, subscription_ids: c.subscription_ids.clone() })
        .collect()
}

async fn local_state(state: &AppState) -> LocalState {
    let existing_ids: HashSet<_> = state.subscriptions.read().await.keys().copied().collect();
    let bindings: HashMap<_, _> = state
        .virtual_models
        .read()
        .await
        .values()
        .map(|c| (c.name, c.subscription_ids.clone()))
        .collect();
    LocalState { existing_ids, bindings, providers: state.providers.clone() }
}

async fn render_export(state: &AppState, password: Option<Zeroizing<String>>) -> AppResult<(String, usize)> {
    let rows = snapshot_rows(state).await;
    let vms = snapshot_vms(state).await;
    let token = Zeroizing::new(state.settings.read().await.auth_token.clone());
    let count = rows.len();
    // Argon2 (64 MiB, t=3) takes a few hundred ms; keep it off the async workers.
    let text = tokio::task::spawn_blocking(move || -> AppResult<String> {
        let secrets = password.as_ref().map(|p| SecretOptions {
            password: p.as_str(),
            auth_token: token.as_str(),
            cost: KdfCost::DEFAULT,
        });
        let file = build_export(&rows, &vms, secrets, env!("CARGO_PKG_VERSION"), Utc::now())?;
        Ok(serde_json::to_string_pretty(&file)?)
    })
    .await
    .map_err(|e| AppError::internal(format!("导出任务异常退出: {e}")))??;
    Ok((text, count))
}

/// 桌面端导出到用户选的路径。`password` 为 Some 即带密钥模式。网页 / TUI 通道是拒绝桩。
#[tauri::command]
pub async fn export_config(
    state: State<'_, AppState>,
    path: String,
    password: Option<String>,
) -> AppResult<ExportSummary> {
    let password = password.map(Zeroizing::new);
    let with_secrets = password.is_some();
    let (text, subscriptions) = render_export(&state, password).await?;
    tokio::fs::write(&path, text.as_bytes()).await?;
    Ok(ExportSummary { subscriptions, with_secrets })
}

/// 不带密钥的导出, 返回文本 (网页界面由浏览器下载)。
#[tauri::command]
pub async fn export_config_text(state: State<'_, AppState>) -> AppResult<String> {
    Ok(render_export(&state, None).await?.0)
}

#[tauri::command]
pub async fn preview_config_import(state: State<'_, AppState>, text: String) -> AppResult<ImportPreview> {
    let file = parse_file(&text)?;
    let local = local_state(&state).await;
    Ok(import::preview(&file, &local))
}

/// The whole import runs in a spawned task: on the web / TUI channel the caller's future is dropped
/// when the HTTP client disconnects, and cancelling between the DB commit and the in-memory refresh
/// would leave the two out of sync. The busy guard moves into the task so it covers the entire run.
#[tauri::command]
pub async fn apply_config_import(
    state: State<'_, AppState>,
    text: String,
    options: ImportOptions,
) -> AppResult<ImportReport> {
    let busy = BusyGuard::acquire()
        .ok_or_else(|| AppError::BadRequest("已有一个导入正在进行".into()))?;
    let app = state.app_handle.clone();
    tokio::spawn(async move {
        let _busy = busy;
        let state = app.state::<AppState>();
        run_import(&state, text, options).await
    })
    .await
    .map_err(|e| AppError::internal(format!("导入任务异常退出: {e}")))?
}

async fn run_import(state: &AppState, text: String, options: ImportOptions) -> AppResult<ImportReport> {
    let file = parse_file(&text)?;

    let secrets = match (&file.secrets, options.skip_secrets) {
        (Some(env), false) => {
            let password = Zeroizing::new(options.password.unwrap_or_default());
            if password.is_empty() {
                return Err(AppError::BadRequest("请输入导出时设置的密码".into()));
            }
            let env = env.clone();
            let plain = tokio::task::spawn_blocking(move || crypto::open(&env, &password))
                .await
                .map_err(|e| AppError::internal(format!("解密任务异常退出: {e}")))??;
            Some(import::resolve_secrets(&file, plain)?)
        }
        _ => None,
    };

    let local = local_state(state).await;
    let plan = import::plan(&file, secrets.as_ref(), &local, Utc::now());
    import::apply(&state.db, &plan).await?;

    {
        let mut subs = state.subscriptions.write().await;
        for row in &plan.inserts {
            subs.insert(row.id, Arc::new(RwLock::new(SubscriptionRuntime::from_row(row.clone()))));
        }
    }
    {
        let mut vms = state.virtual_models.write().await;
        for (name, ids) in &plan.bindings {
            if let Some(cfg) = vms.get_mut(name) {
                cfg.subscription_ids = ids.clone();
                cfg.last_used_index = 0;
            }
        }
        for (name, mode) in &plan.modes {
            if let Some(cfg) = vms.get_mut(name) {
                cfg.mode = *mode;
            }
        }
    }

    let mut report = plan.report.clone();
    if options.import_token {
        match secrets.as_ref().and_then(|s| s.auth_token.clone()) {
            Some(token) if is_valid_token(&token) => {
                match crate::commands::settings::replace_auth_token(state, token).await {
                    Ok(_) => {
                        report.token_imported = true;
                        // Let other open clients (web UI / TUI) refetch settings.
                        let _ = state.app_handle.emit("settings_changed", ());
                    }
                    Err(e) => report.token_error = Some(e.to_string()),
                }
            }
            Some(_) => report.token_error = Some("文件里的访问令牌格式不正确, 未导入".into()),
            None => report.token_error = Some("文件里没有访问令牌".into()),
        }
    }

    // Existing event (already bridged): UI / TUI / tray refetch on it. Payload is a subscription id.
    if let Some(first) = plan.inserts.first() {
        let _ = state.app_handle.emit("subscription_state_changed", first.id.to_string());
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_validation() {
        assert!(is_valid_token("3f9a0c"));
        assert!(!is_valid_token(""));
        assert!(!is_valid_token("has space"));
        assert!(!is_valid_token("换行\n"));
        assert!(!is_valid_token(&"a".repeat(257)));
    }

    #[test]
    fn import_options_debug_redacts_password() {
        let opts = ImportOptions {
            password: Some("hunter2-secret".into()),
            skip_secrets: false,
            import_token: true,
        };
        let shown = format!("{:?}", opts);
        assert!(!shown.contains("hunter2-secret"), "{shown}");
        assert!(shown.contains("<redacted>") && shown.contains("import_token: true"), "{shown}");
        assert!(format!("{:?}", ImportOptions::default()).contains("password: None"));
    }

    #[test]
    fn only_one_import_at_a_time() {
        let first = BusyGuard::acquire().expect("第一次应能拿到");
        assert!(BusyGuard::acquire().is_none(), "进行中时第二次拿不到");
        drop(first);
        assert!(BusyGuard::acquire().is_some(), "释放后可再拿");
    }
}
