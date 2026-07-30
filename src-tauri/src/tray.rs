//! 系统托盘 + 窗口关闭拦截（设计稿 §13.4）。
//!
//! `tauri.conf.json` 的 `app.trayIcon` 字段已经声明了托盘，Tauri 启动时自动注册。
//! 这里只需要挂上菜单与事件回调。
//!
//! macOS 上 cc-router 是纯菜单栏 app（activationPolicy=Accessory，见 `lib.rs::run`），
//! Dock 里没有图标，托盘就是唯一常驻入口，所以菜单文案必须跟随用户选的界面语言。

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
use tauri::{App, Manager, WindowEvent};
use tracing::warn;

/// 必须与 `tauri.conf.json::app.trayIcon.id` 一致。
const TRAY_ID: &str = "cc-router-tray";

/// 托盘菜单语言，与前端 `src/i18n/index.tsx` 的 `Locale` 一一对应。
///
/// 只有两条文案，刻意不引 Rust i18n 框架：一张 `match` 常量表比多一个依赖 +
/// 一套资源文件划算得多。加语言时这里加一个 variant + 两条 match 分支，
/// 前端 `src/i18n/locales/` 同步加文件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayLocale {
    Zh,
    En,
    Ja,
}

impl TrayLocale {
    /// `settings.preferred_language`（"system" / "zh" / "en" / "ja"）→ 实际语言。
    pub fn from_pref(pref: &str) -> Self {
        Self::resolve(pref, tauri_plugin_os::locale().as_deref())
    }

    /// `from_pref` 的纯函数版本：系统语言标签显式传入而不是直接读 OS，
    /// 这样映射规则在任何平台的 CI 上都能单测。
    ///
    /// 映射规则必须与前端 `src/i18n/index.tsx::detectSystemLocale()` 逐字一致，
    /// 否则「跟随系统」时托盘和 UI 会显示两种语言：
    /// `zh*` → 中文，`ja*` → 日本語，其余（含取不到 locale）→ English。
    fn resolve(pref: &str, system_tag: Option<&str>) -> Self {
        match pref {
            "zh" => Self::Zh,
            "en" => Self::En,
            "ja" => Self::Ja,
            // "system"、空串、以及任何未知值都走系统探测 —— 前端 resolveLocale 对
            // undefined / "system" 同样落到 detectSystemLocale()。
            _ => match system_tag {
                None => Self::En,
                Some(tag) => {
                    let lower = tag.to_ascii_lowercase();
                    if lower.starts_with("zh") {
                        Self::Zh
                    } else if lower.starts_with("ja") {
                        Self::Ja
                    } else {
                        Self::En
                    }
                }
            },
        }
    }

    fn show_window(self) -> &'static str {
        match self {
            Self::Zh => "显示主窗口",
            Self::En => "Show Main Window",
            Self::Ja => "メインウィンドウを表示",
        }
    }

    fn quit(self) -> &'static str {
        match self {
            Self::Zh => "退出 cc-router",
            Self::En => "Quit cc-router",
            Self::Ja => "cc-router を終了",
        }
    }
}

/// 按语言构造托盘菜单。
///
/// 菜单项 id 恒为 `show` / `quit`，`on_menu_event` 按 id 分发 —— 所以换语言时
/// 只需要 `set_menu` 换掉菜单，handler 不用动（见 `rebuild_menu`）。
fn build_menu<R: tauri::Runtime, M: Manager<R>>(
    manager: &M,
    locale: TrayLocale,
) -> tauri::Result<Menu<R>> {
    let show_item = MenuItem::with_id(manager, "show", locale.show_window(), true, None::<&str>)?;
    let quit_item = MenuItem::with_id(manager, "quit", locale.quit(), true, None::<&str>)?;
    Menu::with_items(manager, &[&show_item, &quit_item])
}

pub fn setup(app: &mut App, locale: TrayLocale) -> tauri::Result<()> {
    let menu = build_menu(app, locale)?;

    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        warn!("tray icon 'cc-router-tray' 未自动创建, 请检查 tauri.conf.json");
        return Ok(());
    };

    tray.set_menu(Some(menu))?;
    tray.on_menu_event(move |app, event| match event.id.as_ref() {
        "show" => {
            if let Some(win) = app.get_webview_window("main") {
                reveal_window(&win);
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
                reveal_window(&win);
            }
        }
    });

    Ok(())
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
pub fn rebuild_menu<R: tauri::Runtime>(app: &tauri::AppHandle<R>, locale: TrayLocale) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        warn!("tray icon 'cc-router-tray' 不存在, 跳过菜单重建");
        return;
    };
    if let Err(e) = build_menu(app, locale).and_then(|menu| tray.set_menu(Some(menu))) {
        warn!(error = %e, "failed to rebuild tray menu");
    }
}

/// 把主窗口呼出到前台并抢键盘焦点。
///
/// 顺序很关键: Tauri `WebviewWindow::set_focus` 在 macOS 下透传到 tao
/// `Window::set_focus` (tao 0.35.x src/platform_impl/macos/window.rs),
/// 该实现仅在 `!is_minimized && is_visible` 时才会调用
/// `NSApp.activateIgnoringOtherApps(YES)`. 所以必须先 unminimize、再 show、
/// 最后 set_focus, 否则在 Accessory (Dock 隐藏) 模式下从托盘呼出窗口
/// 可能不抢前台焦点, 用户需要二次点击.
pub(crate) fn reveal_window<R: tauri::Runtime>(win: &tauri::WebviewWindow<R>) {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_pref_overrides_system_locale() {
        assert_eq!(TrayLocale::resolve("zh", Some("en-US")), TrayLocale::Zh);
        assert_eq!(TrayLocale::resolve("en", Some("zh-Hans-CN")), TrayLocale::En);
        assert_eq!(TrayLocale::resolve("ja", Some("zh-CN")), TrayLocale::Ja);
        // 显式选择时根本不该看系统 locale, 取不到也无所谓
        assert_eq!(TrayLocale::resolve("ja", None), TrayLocale::Ja);
    }

    /// 这组断言是与前端 src/i18n/index.tsx::detectSystemLocale() 的契约,
    /// 改任何一条之前先去看那个函数。
    #[test]
    fn system_pref_matches_frontend_detection_rules() {
        assert_eq!(TrayLocale::resolve("system", Some("zh-CN")), TrayLocale::Zh);
        assert_eq!(
            TrayLocale::resolve("system", Some("zh-Hans-CN")),
            TrayLocale::Zh
        );
        assert_eq!(
            TrayLocale::resolve("system", Some("zh-Hant-TW")),
            TrayLocale::Zh
        );
        assert_eq!(TrayLocale::resolve("system", Some("ja-JP")), TrayLocale::Ja);
        assert_eq!(TrayLocale::resolve("system", Some("en-US")), TrayLocale::En);
        assert_eq!(TrayLocale::resolve("system", Some("de-DE")), TrayLocale::En);
        assert_eq!(TrayLocale::resolve("system", Some("ko-KR")), TrayLocale::En);
    }

    #[test]
    fn locale_tag_matching_is_case_insensitive() {
        // sys_locale 在不同 OS 上大小写不统一, 前端 detectSystemLocale 也做了 toLowerCase
        assert_eq!(TrayLocale::resolve("system", Some("ZH-CN")), TrayLocale::Zh);
        assert_eq!(TrayLocale::resolve("system", Some("Ja-jp")), TrayLocale::Ja);
    }

    #[test]
    fn unknown_or_missing_falls_back_to_english() {
        assert_eq!(TrayLocale::resolve("system", None), TrayLocale::En);
        assert_eq!(TrayLocale::resolve("", None), TrayLocale::En);
        // 手改 settings.json 塞了非法值, 不能 panic, 走系统探测
        assert_eq!(TrayLocale::resolve("klingon", Some("zh-CN")), TrayLocale::Zh);
        assert_eq!(TrayLocale::resolve("klingon", None), TrayLocale::En);
    }

    /// 加语言时忘了填某条文案 -> 空串菜单项, 这里兜住。
    #[test]
    fn every_locale_has_both_labels() {
        for locale in [TrayLocale::Zh, TrayLocale::En, TrayLocale::Ja] {
            assert!(
                !locale.show_window().is_empty(),
                "{locale:?} show_window 缺文案"
            );
            assert!(!locale.quit().is_empty(), "{locale:?} quit 缺文案");
        }
    }
}
