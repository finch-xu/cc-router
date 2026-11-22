//! 系统托盘 + 窗口关闭拦截（设计稿 §13.4）。
//!
//! `tauri.conf.json` 的 `app.trayIcon` 字段已经声明了托盘，Tauri 启动时自动注册。
//! 这里只需要挂上菜单与事件回调。
//!
//! macOS 上 Dock 图标随窗口显隐动态切换（Regular ↔ Accessory，见 `reveal_window`
//! 与 `on_window_event`）：窗口收进托盘后 Dock 无图标，托盘是唯一常驻入口，
//! 所以菜单除了开关窗口, 还带状态 (代理 / 订阅)、常用页面快捷方式、复制环境变量、
//! 检查更新与开机自启。文案跟随界面语言 (`strings`), 状态就地刷新 (`menu`)。

mod menu;
mod strings;

use std::time::Duration;

use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
use tauri::{App, AppHandle, Emitter, Listener, Manager, WindowEvent};
use tracing::warn;

use crate::settings::model::SettingsPatch;
use crate::state::AppState;
use menu::{ids, TraySnapshot, TrayState};
pub use strings::TrayLocale;

/// 必须与 `tauri.conf.json::app.trayIcon.id` 一致。
const TRAY_ID: &str = "cc-router-tray";

/// 让前端切到某个页面。用 `emit_to("main", ..)` 只发给桌面主窗口, **刻意不进**
/// `proxy/web/events.rs::BRIDGED_EVENTS` —— 桥过去的话, 开着的网页界面也会跟着跳页。
/// (那边的源码扫描只认 `.emit(` 字面量, 不会把这里当成漏登记。)
const NAVIGATE_EVENT: &str = "tray://navigate";

/// 兜底刷新间隔: 订阅增删 / 限额到期这类变化没有专门的事件, 靠它追上。
const REFRESH_INTERVAL: Duration = Duration::from_secs(15);
/// 「已复制」提示停留多久
const COPIED_FOR: Duration = Duration::from_secs(3);

pub fn setup(app: &mut App, locale: TrayLocale) -> tauri::Result<()> {
    let handle = app.handle().clone();
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        warn!("tray icon 'cc-router-tray' 未自动创建, 请检查 tauri.conf.json");
        return Ok(());
    };

    let state = TrayState::new(locale);
    // 先把托盘挂上, 再 manage: install_menu 内部会 apply, 而 apply 只读自身不查 app.state
    state.install_menu(&handle, locale)?;
    app.manage(state);

    tray.on_menu_event(move |app, event| match event.id.as_ref() {
        ids::SHOW => show_main_window(app),
        ids::OPEN_SUBS => navigate(app, "/subscriptions"),
        ids::OPEN_LIVE => navigate(app, "/live-routing"),
        ids::OPEN_LOGS => navigate(app, "/request-logs"),
        // 更新页看到 check=1 会立刻检查一次 (正在下载 / 等待重启时除外)
        ids::CHECK_UPDATE => navigate(app, "/updates?check=1"),
        ids::COPY_ENV => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move { copy_env(app).await });
        }
        ids::AUTOSTART => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move { toggle_autostart(app).await });
        }
        ids::QUIT => app.exit(0),
        _ => {}
    });

    tray.on_tray_icon_event(|tray, event| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            show_main_window(tray.app_handle());
        }
    });

    // 状态刷新: 订阅状态机每次转移都会发这个事件; 其余变化靠定时兜底
    let on_state = handle.clone();
    handle.listen_any("subscription_state_changed", move |_| refresh(&on_state));
    let ticker = handle.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            refresh(&ticker);
            tokio::time::sleep(REFRESH_INTERVAL).await;
        }
    });

    Ok(())
}

/// 从 `AppState` 汇总一次状态并写到菜单上。可以从任何线程调用, 也可以在托盘装好之前
/// 调用 (那时直接忽略 —— `setup` 装好后会自己刷一次)。
pub fn refresh(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(state) = app.try_state::<AppState>() else { return };
        let snapshot = snapshot(&state).await;
        let autostart = state.settings.read().await.autostart;
        let Some(tray) = menu::tray_state(&app) else { return };
        tray.set_snapshot(snapshot, autostart);
        apply_on_main(&app);
    });
}

/// 前端检测到新版本 (或确认已是最新) 后推过来 (`commands::tray::set_tray_update`)。
pub fn set_update(app: &AppHandle, version: Option<String>) {
    if let Some(tray) = menu::tray_state(app) {
        tray.set_update(version);
        apply_on_main(app);
    }
}

/// 用户在设置里切界面语言后重建托盘菜单（`commands::settings::update_settings` 调用）。
///
/// **只 `set_menu`，绝不能再调 `tray.on_menu_event`**：Tauri 的
/// `TrayIcon::on_menu_event` 是往 app 级 `manager.menu.global_event_listeners`
/// 这个 `Vec` 里 `push`（tauri-2.11.1 `src/tray/mod.rs:467`），不是覆盖注册。
/// 重复注册会让一次点击触发 N 次 —— 切过两次语言后点「退出」就是连着两次
/// `app.exit(0)`。菜单项 id 不变，启动时挂的那份 handler 对新菜单继续有效。
///
/// 失败只 warn 不返回错误：菜单文案没跟上语言是观感问题，不该让设置保存失败。
///
/// 调用方负责把它送上主线程（muda 的 NSMenu 只能主线程碰）。
pub fn rebuild_menu(app: &AppHandle, locale: TrayLocale) {
    let Some(tray) = menu::tray_state(app) else {
        warn!("tray state 不存在, 跳过菜单重建");
        return;
    };
    if let Err(e) = tray.install_menu(app, locale) {
        warn!(error = %e, "failed to rebuild tray menu");
    }
}

async fn snapshot(state: &AppState) -> TraySnapshot {
    let running =
        state.http_bound_port.read().await.is_some() || state.https_bound_port.read().await.is_some();
    let address = if running {
        let url = state.local_base_url().await;
        Some(
            url.trim_start_matches("http://")
                .trim_start_matches("https://")
                .to_string(),
        )
    } else {
        None
    };

    let now = chrono::Utc::now();
    let runtimes: Vec<_> = state.subscriptions.read().await.values().cloned().collect();
    let mut snap = TraySnapshot {
        address,
        total: runtimes.len(),
        ..TraySnapshot::default()
    };
    for rt in runtimes {
        let rt = rt.read().await;
        if !rt.row.enabled {
            continue;
        }
        snap.enabled += 1;
        if !rt.is_dispatchable(now) {
            snap.down += 1;
        }
    }
    snap
}

fn apply_on_main(app: &AppHandle) {
    let handle = app.clone();
    if let Err(e) = app.run_on_main_thread(move || {
        if let Some(tray) = menu::tray_state(&handle) {
            tray.apply(&handle);
        }
    }) {
        warn!(error = %e, "failed to dispatch tray menu update");
    }
}

fn show_main_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        reveal_window(&win);
    }
}

/// 呼出主窗口并让前端切到 `path`。窗口隐藏时网页并没销毁, 事件照样送达。
pub(crate) fn navigate(app: &AppHandle, path: &str) {
    show_main_window(app);
    if let Err(e) = app.emit_to("main", NAVIGATE_EVENT, path) {
        warn!(error = %e, path, "tray navigate emit failed");
    }
}

/// 复制 Claude Code 环境变量: macOS / Linux 是 `export` 行, Windows 是 PowerShell `$env:` 行。
async fn copy_env(app: AppHandle) {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    let Some(state) = app.try_state::<AppState>() else { return };
    let base_url = state.local_base_url().await;
    let token = state.settings.read().await.auth_token.clone();
    let pairs = crate::commands::proxy::claude_code_env(&base_url, &token);
    let text = if cfg!(windows) {
        crate::commands::proxy::render_env_powershell(&pairs)
    } else {
        crate::commands::proxy::render_env_export(&pairs)
    };
    if let Err(e) = app.clipboard().write_text(text) {
        warn!(error = %e, "tray copy env failed");
        return;
    }
    let Some(tray) = menu::tray_state(&app) else { return };
    tray.set_copied(true);
    apply_on_main(&app);
    tokio::time::sleep(COPIED_FOR).await;
    tray.set_copied(false);
    apply_on_main(&app);
}

/// 勾选框点击: 与设置页的开关走同一条路径 (`apply_settings_patch`: 先改系统登录项,
/// 成功才落 settings.json), 再通知前端刷新设置缓存。无论成败最后都按 settings 真值
/// 重设勾选状态 —— 菜单项被点击时平台已经自己翻转了勾选, 失败时要翻回去。
async fn toggle_autostart(app: AppHandle) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let want = !state.settings.read().await.autostart;
    let patch = SettingsPatch {
        autostart: Some(want),
        ..SettingsPatch::default()
    };
    match crate::commands::settings::apply_settings_patch(&state, patch).await {
        Ok(_) => {
            let _ = app.emit("settings_changed", ());
        }
        Err(e) => warn!(error = %e, want, "tray autostart toggle failed"),
    }
    refresh(&app);
}

/// 把主窗口呼出到前台并抢键盘焦点。
///
/// 顺序很关键, 且 macOS 下 policy 必须排最先:
/// 1. 先把 activation policy 升回 Regular (Dock 图标出现)。tao 的 `set_focus`
///    走 `NSApp.activateIgnoringOtherApps`, Accessory 进程调用它常被
///    WindowServer 忽略 (Accessory 语义即「不参与前台激活」), 先升 Regular
///    才能保证抢到前台。
/// 2. 再 unminimize → show → set_focus: Tauri `WebviewWindow::set_focus` 在
///    macOS 下透传到 tao `Window::set_focus` (tao 0.35.x
///    src/platform_impl/macos/window.rs), 该实现仅在
///    `!is_minimized && is_visible` 时才会调用
///    `NSApp.activateIgnoringOtherApps(YES)`, 乱序会导致用户需要二次点击。
pub(crate) fn reveal_window<R: tauri::Runtime>(win: &tauri::WebviewWindow<R>) {
    #[cfg(target_os = "macos")]
    if let Err(e) = win
        .app_handle()
        .set_activation_policy(tauri::ActivationPolicy::Regular)
    {
        warn!(error = %e, "set_activation_policy(Regular) failed");
    }
    let _ = win.unminimize();
    let _ = win.show();
    let _ = win.set_focus();
}

/// 主窗口关闭时：阻止关闭，改为隐藏，交给托盘保活。
///
/// 只拦 `main`：这个 handler 是 app 级的，将来若出现别的 window（OAuth 回调窗、
/// 独立日志窗），它们的 close 必须能真正关掉，否则会变成关不掉的幽灵窗口。
/// 当前只有 main 一个窗口，这是纯防御。
pub fn on_window_event(window: &tauri::Window, event: &WindowEvent) {
    if window.label() != "main" {
        return;
    }
    if let WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let _ = window.hide();
        // macOS: 窗口收进托盘后降为 Accessory —— Dock 图标消失, 退出 Cmd+Tab。
        // 下次 reveal_window 会升回 Regular。
        #[cfg(target_os = "macos")]
        if let Err(e) = window
            .app_handle()
            .set_activation_policy(tauri::ActivationPolicy::Accessory)
        {
            warn!(error = %e, "set_activation_policy(Accessory) failed");
        }
    }
}

