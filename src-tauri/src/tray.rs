//! 系统托盘 + 窗口关闭拦截（设计稿 §13.4）。
//!
//! `tauri.conf.json` 的 `app.trayIcon` 字段已经声明了托盘，Tauri 启动时自动注册。
//! 这里只需要挂上菜单与事件回调。

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
use tauri::{App, Manager, WindowEvent};
use tracing::warn;

pub fn setup(app: &mut App) -> tauri::Result<()> {
    let show_item = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出 cc-router", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

    let Some(tray) = app.tray_by_id("cc-router-tray") else {
        warn!("tray icon 'cc-router-tray' 未自动创建, 请检查 tauri.conf.json");
        return Ok(());
    };

    tray.set_menu(Some(menu))?;
    tray.on_menu_event(move |app, event| match event.id.as_ref() {
        "show" => {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.set_focus();
                let _ = win.unminimize();
            }
        }
        "quit" => {
            app.exit(0);
        }
        _ => {}
    });

    tray.on_tray_icon_event(|tray, event| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            if let Some(win) = tray.app_handle().get_webview_window("main") {
                let _ = win.show();
                let _ = win.set_focus();
                let _ = win.unminimize();
            }
        }
    });

    Ok(())
}

/// 主窗口关闭时：阻止关闭，改为隐藏，交给托盘保活。
pub fn on_window_event(window: &tauri::Window, event: &WindowEvent) {
    if let WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let _ = window.hide();
    }
}
