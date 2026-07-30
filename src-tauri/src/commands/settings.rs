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
    let autostart_change = patch.autostart;

    // autostart 副作用必须先成功才 apply, 否则 UI 显示「已启用」但 LaunchAgent 没注册.
    // 这里涉及文件系统 / Registry, 失败可能性真实存在, 所以是先决条件而非事后通知.
    if let Some(want) = autostart_change {
        use tauri_plugin_autostart::ManagerExt;
        let manager = state.app_handle.autolaunch();
        let outcome = if want { manager.enable() } else { manager.disable() };
        if let Err(e) = outcome {
            return Err(crate::error::AppError::internal(format!(
                "autostart toggle failed: {e}"
            )));
        }
    }

    let mut guard = state.settings.write().await;
    // 比对前后值而不是直接看 patch.preferred_language.is_some():
    // 前端每次点 select 都会发同一个值, 没变就别去动 NSMenu.
    let prev_language = guard.preferred_language.clone();
    guard.apply_patch(patch);
    let language_changed = guard.preferred_language != prev_language;
    let app_data_dir = paths::app_data_dir(&state.app_handle)?;
    save(&app_data_dir, &guard).await?;
    let snapshot = guard.clone();
    drop(guard);

    // 托盘菜单文案跟随 UI 语言. 与 autostart 相反, 这是事后副作用:
    // 菜单没跟上语言只是观感问题, 不该让设置保存失败.
    if language_changed {
        let handle = state.app_handle.clone();
        let locale = crate::tray::TrayLocale::from_pref(&snapshot.preferred_language);
        // 整段派到主线程一次: muda 的 NSMenu 只能主线程碰, 而 tauri 的每个菜单 API
        // (MenuItem::with_id / Menu::new / append_items / set_menu) 都各做一次
        // run_on_main_thread + mpsc::recv 同步等待. 在这里一次性跨过去, 内层调用
        // 因为已经在主线程上会同步短路 (tauri-runtime-wry send_user_message 的主线程分支),
        // 既不在 tokio worker 上阻塞, 也把 5 次跨线程往返压成 1 次.
        if let Err(e) = state.app_handle.run_on_main_thread(move || {
            crate::tray::rebuild_menu(&handle, locale);
        }) {
            tracing::warn!(error = %e, "failed to dispatch tray menu rebuild");
        }
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
