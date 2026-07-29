//! 启动时按显示器可用区自适应窗口几何。
//!
//! `tauri.conf.json` 里的 width/height 只是首帧值 —— 真实尺寸在这里按
//! `Monitor::work_area()` (已扣除 Windows 任务栏 / macOS Dock + 菜单栏, 不是
//! `size()`) 重算, 避免 1920x1080 @150% 这类逻辑分辨率只剩 1280x720 的笔记本上
//! 窗口比屏幕还大。
//!
//! 为什么手写而不用插件: Tauri 2 的 `WindowConfig` 只有绝对的 width/height/
//! min_*/max_* 与 center/maximized, 没有任何按屏幕比例的声明式字段; 官方唯一
//! 相关的 `tauri-plugin-window-state` 只做持久化, 且恢复尺寸时不校验显示器
//! (只对位置做 intersects 判断), 反而会把「窗口大于屏幕」持久化下来。查显示器
//! 再 set_size 是官方推荐路径。
//!
//! Rust 侧调用直连 dispatcher 不经 ACL, 因此 `capabilities/default.json` 无需改动。

use tauri::{LogicalSize, Manager};
use tracing::{info, warn};

/// 理想尺寸下限 (CSS px): 笔记本与小屏上的目标尺寸。
const PREFERRED_W: f64 = 1200.0;
const PREFERRED_H: f64 = 800.0;
/// 理想尺寸上限 (CSS px): 外接大屏放大的封顶, 防止超宽屏出现巨型窗口。
const MAX_W: f64 = 1600.0;
const MAX_H: f64 = 1000.0;
/// 大屏放大时理想尺寸占可用区的比例 (高度比例略高, 维持接近 16:10 的观感)。
const GROW_W: f64 = 0.62;
const GROW_H: f64 = 0.70;
/// 绝对下限 (CSS px): 侧边栏死宽 220px + 多列表格开始挤压的临界点。
const FLOOR_W: f64 = 960.0;
const FLOOR_H: f64 = 560.0;
/// 可用区占比: 留一圈呼吸空间, 让用户看得见桌面边缘。
const WORK_AREA_RATIO: f64 = 0.92;
/// Windows 低缩放下的 webview 放大倍数。
const WINDOWS_UI_ZOOM: f64 = 1.08;
/// OS 缩放低于该值才补偿字号, 见 `resolve_ui_zoom_for`。
const ZOOM_COMPENSATION_BELOW_SCALE: f64 = 1.25;

/// 窗口几何计算结果, 单位均为逻辑像素 (即 CSS px × ui_zoom)。
#[derive(Debug, Clone, Copy, PartialEq)]
struct Geometry {
    width: f64,
    height: f64,
    min_width: f64,
    min_height: f64,
}

/// Windows 在 100% 缩放下微软雅黑 12px 以下会触发劣质 hinting, 字形发虚、笔画粘连;
/// 而 OS 缩放 >=125% 时系统已经把物理像素补上了, 再叠加 zoom 反而过大。
///
/// 这个 scale 感知顺带消解了一处约束冲突: 放大 webview 会压缩 CSS 视口、需要抬高
/// 最小窗口宽度, 而最窄的屏幕恰恰是高缩放屏 (1080p@175% 逻辑宽仅 1097px)。
/// 按 scale 门控后, 两个约束永不同时触发。
///
/// `is_windows` 显式传入而非直接读 `cfg!`, 是为了让阈值逻辑在任何平台都可单测。
fn resolve_ui_zoom_for(is_windows: bool, scale_factor: f64) -> f64 {
    if is_windows && scale_factor < ZOOM_COMPENSATION_BELOW_SCALE {
        WINDOWS_UI_ZOOM
    } else {
        1.0
    }
}

fn resolve_ui_zoom(scale_factor: f64) -> f64 {
    resolve_ui_zoom_for(cfg!(target_os = "windows"), scale_factor)
}

/// 纯计算, 无 Tauri 依赖。
///
/// `avail_*` 是显示器可用区的逻辑像素尺寸; `ui_zoom` 是 webview 页面缩放倍数 ——
/// 视口被 zoom 压缩, 所以窗口像素要按 zoom 放大才能维持同样的 CSS px 布局宽度。
fn compute_geometry(avail_w: f64, avail_h: f64, ui_zoom: f64) -> Geometry {
    // 理想尺寸: 笔记本上恒为 1200x800; 外接大屏按可用区比例增长, 封顶 1600x1000
    let pref_w = (avail_w * GROW_W).clamp(PREFERRED_W, MAX_W);
    let pref_h = (avail_h * GROW_H).clamp(PREFERRED_H, MAX_H);

    // 可用区比下限还小时把下限压到可用区, 保证窗口永远不会大于屏幕
    let min_width = (FLOOR_W * ui_zoom).min(avail_w);
    let min_height = (FLOOR_H * ui_zoom).min(avail_h);
    // .max(min_*) 保证 clamp 的 min <= max, 否则极端尺寸下会 panic
    let max_w = (pref_w * ui_zoom).max(min_width);
    let max_h = (pref_h * ui_zoom).max(min_height);

    Geometry {
        width: (avail_w * WORK_AREA_RATIO).clamp(min_width, max_w),
        height: (avail_h * WORK_AREA_RATIO).clamp(min_height, max_h),
        min_width,
        min_height,
    }
}

/// 按当前显示器可用区重设窗口尺寸。尽力而为 —— 任何一步失败都只 warn, 让调用方
/// 继续执行 center/show, 大不了退回 `tauri.conf.json` 里的静态尺寸。
fn resize_to_work_area<R: tauri::Runtime>(win: &tauri::WebviewWindow<R>) {
    // 窗口刚创建时 current_monitor 即它落位的那块屏; 拿不到就退到主显示器
    let monitor = win
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| win.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        warn!("无法解析显示器信息, 保留 tauri.conf.json 的窗口尺寸");
        return;
    };

    let scale = monitor.scale_factor();
    if !scale.is_finite() || scale <= 0.0 {
        warn!(scale, "显示器 scale_factor 非法, 跳过窗口自适应");
        return;
    }

    let work = monitor.work_area();
    let avail_w = f64::from(work.size.width) / scale;
    let avail_h = f64::from(work.size.height) / scale;
    if avail_w <= 0.0 || avail_h <= 0.0 {
        warn!(avail_w, avail_h, "显示器可用区为空, 跳过窗口自适应");
        return;
    }

    let ui_zoom = resolve_ui_zoom(scale);
    let geo = compute_geometry(avail_w, avail_h, ui_zoom);

    // 顺序很关键: tauri.conf.json 的 minWidth/minHeight 会钳制 set_size,
    // 必须先把下限放开, 否则小屏上根本缩不下去。
    if let Err(e) = win.set_min_size(Some(LogicalSize::new(geo.min_width, geo.min_height))) {
        warn!(error = %e, "set_min_size 失败");
    }
    if let Err(e) = win.set_size(LogicalSize::new(geo.width, geo.height)) {
        warn!(error = %e, "set_size 失败");
    }
    if ui_zoom != 1.0 {
        // 页面级缩放 (WebView2 ZoomFactor), 不是 CSS zoom —— 它会重算 CSS 视口,
        // 所以 100vh 仍等于窗口高度、position:fixed 仍贴边。
        if let Err(e) = win.set_zoom(ui_zoom) {
            warn!(error = %e, "set_zoom 失败");
        }
    }

    info!(
        avail_w,
        avail_h,
        scale,
        ui_zoom,
        width = geo.width,
        height = geo.height,
        "窗口几何已按显示器可用区自适应"
    );
}

/// 启动入口: 定几何 → 居中 → 显示。
///
/// 必须在 `bootstrap()` 之前调用 —— 几何计算是同步且极快的, 而 bootstrap 要跑
/// DB migration / provider 加载 / TLS。先定几何再显示, 用户看到的第一帧就是
/// 尺寸正确的窗口, 而不是错位窗口或白屏。
pub fn apply_startup_geometry<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let Some(win) = app.get_webview_window("main") else {
        warn!("主窗口 'main' 不存在, 跳过窗口几何自适应");
        return;
    };

    resize_to_work_area(&win);
    let _ = win.center();
    // tauri.conf.json 里 visible=false, 几何就位后才显示, 避免"先弹默认尺寸再跳变"
    let _ = win.show();
    let _ = win.set_focus();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 逻辑像素允许亚像素误差, 只要落在同一整数像素即可。
    fn assert_close(actual: f64, expected: f64, what: &str) {
        assert!(
            (actual - expected).abs() < 0.01,
            "{what}: 期望 {expected}, 实际 {actual}"
        );
    }

    fn check(label: &str, avail: (f64, f64), zoom: f64, want: (f64, f64)) {
        let geo = compute_geometry(avail.0, avail.1, zoom);
        assert_close(geo.width, want.0, &format!("{label} width"));
        assert_close(geo.height, want.1, &format!("{label} height"));
        // 窗口永远不得大于可用区, 这是本模块存在的全部理由
        assert!(
            geo.width <= avail.0 && geo.height <= avail.1,
            "{label}: 窗口 {}x{} 超出可用区 {}x{}",
            geo.width,
            geo.height,
            avail.0,
            avail.1
        );
    }

    #[test]
    fn shrinks_on_small_screens() {
        // 1080p@175%: 逻辑仅 1097x617, 旧配置的 minHeight=640 在这里根本装不下
        check("1080p@175%", (1097.0, 590.0), 1.0, (1009.24, 560.0));
        // 1080p@150%: 纯 WORK_AREA_RATIO 收缩
        check("1080p@150%", (1280.0, 680.0), 1.0, (1177.6, 625.6));
        // MacBook Air 13": 逻辑 1280x832 扣掉菜单栏与 Dock, 旧配置的 800 高会溢出
        check("MBA13", (1280.0, 737.0), 1.0, (1177.6, 678.04));
    }

    #[test]
    fn caps_at_preferred_on_laptops() {
        // MacBook Pro 14": 可用区够大但未达大屏阈值, 恒定理想尺寸
        check("MBP14", (1512.0, 887.0), 1.0, (1200.0, 800.0));
    }

    #[test]
    fn grows_on_large_monitors() {
        // 5K 27": GROW 生效但未触 MAX
        check("5K27", (2560.0, 1345.0), 1.0, (1587.2, 941.5));
        // 4K 32": 宽高双双触 MAX 封顶
        check("4K32", (3072.0, 1688.0), 1.0, (1600.0, 1000.0));
    }

    #[test]
    fn accounts_for_windows_zoom() {
        // zoom 把窗口像素放大, 使 CSS 视口回到 1200x800
        check("1080p@100% Win", (1920.0, 1040.0), 1.08, (1296.0, 864.0));
        let geo = compute_geometry(1920.0, 1040.0, 1.08);
        assert_close(geo.width / 1.08, 1200.0, "CSS 视口宽");
        assert_close(geo.height / 1.08, 800.0, "CSS 视口高");

        // zoom 与可用区收缩并存
        check("1366x768 Win", (1366.0, 728.0), 1.08, (1256.72, 669.76));
        // 超宽屏: MAX 与 zoom 复合
        check("ultrawide Win", (3440.0, 1400.0), 1.08, (1728.0, 1058.4));

        // 下限也要跟着 zoom 抬高, 否则 CSS 视口会跌破 FLOOR_W 的布局底线
        let geo = compute_geometry(1920.0, 1040.0, 1.08);
        assert_close(geo.min_width, FLOOR_W * 1.08, "min_width");
        assert_close(geo.min_height, FLOOR_H * 1.08, "min_height");
    }

    #[test]
    fn never_exceeds_tiny_work_area() {
        // 可用区小于 FLOOR 时下限被压到可用区, clamp 的 min<=max 仍成立, 不 panic
        check("极端小屏", (800.0, 500.0), 1.0, (800.0, 500.0));
        let geo = compute_geometry(800.0, 500.0, 1.0);
        assert_close(geo.min_width, 800.0, "min_width 被压到可用区");
        assert_close(geo.min_height, 500.0, "min_height 被压到可用区");
    }

    #[test]
    fn zoom_only_on_low_scale_windows() {
        // Windows 100% / 未知低缩放 → 补偿
        assert_eq!(resolve_ui_zoom_for(true, 1.0), WINDOWS_UI_ZOOM);
        assert_eq!(resolve_ui_zoom_for(true, 1.24), WINDOWS_UI_ZOOM);
        // Windows 125% 起 OS 已补足物理像素 → 不再叠加
        assert_eq!(resolve_ui_zoom_for(true, 1.25), 1.0);
        assert_eq!(resolve_ui_zoom_for(true, 1.5), 1.0);
        assert_eq!(resolve_ui_zoom_for(true, 1.75), 1.0);
        // 非 Windows 一律不动, 保证 macOS / Linux 像素级不变
        assert_eq!(resolve_ui_zoom_for(false, 1.0), 1.0);
        assert_eq!(resolve_ui_zoom_for(false, 2.0), 1.0);
    }
}
