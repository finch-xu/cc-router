//! 终端界面 (cc-router-tui) 相关 command: 启动路径 + 一键添加到 PATH (spec §3.4 / §3.5 / §7.2)。
//! 碰系统的逻辑都在 `crate::tui_install`; 这里只负责拼 DTO。
//!
//! `install_tui_command` / `uninstall_tui_command` **只允许从桌面窗口调用**: 它们会在宿主机上弹系统授权框、
//! 改宿主机的 PATH。`proxy/web/api.rs::web_commands!` 里这两条是拒绝桩 (不调用这里的函数), 有源码扫描测试锁住。

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{AppError, AppResult};
use crate::tui_install::{self, InstallBlocked, InstallKind, InstallStatus, Outcome};

/// sidecar 的文件名。Tauri 的 externalBin 打包时会去掉 target triple 后缀,
/// 安装后它与主程序同目录 (macOS: Contents/MacOS/, Windows: 安装目录, deb: /usr/bin/)。
const SIDECAR_NAME: &str = if cfg!(windows) { "cc-router-tui.exe" } else { "cc-router-tui" };

#[derive(Debug, Serialize)]
pub struct TuiLaunchInfo {
    /// sidecar 的绝对路径; 不存在 (dev 构建 / 未打包 sidecar 的版本) 为 None
    pub path: Option<String>,
    /// AppImage 运行时 path 每次启动都变, 前端据此换一套说明
    pub is_appimage: bool,
    /// 这台机器上「添加到 PATH」是哪种做法 (前端据此选说明文字; `system` / `unavailable` 不显示按钮)
    pub install_kind: InstallKind,
    /// 目标位置上已经有指向本 sidecar 的条目
    pub in_path: bool,
    /// 已安装时的位置: macOS 链接路径 / Windows PATH 条目 / Linux 复制目标
    pub installed_at: Option<String>,
    pub can_install: bool,
    /// 不能安装的原因; 文案由前端按值翻译
    pub install_blocked: Option<InstallBlocked>,
    /// 仅 `copy`: `~/.local/bin` 不在 PATH 里, 前端提示用户自己加一行 export
    pub local_bin_off_path: bool,
}

impl TuiLaunchInfo {
    fn new(path: Option<String>, is_appimage: bool, status: InstallStatus) -> Self {
        Self {
            path,
            is_appimage,
            install_kind: status.kind,
            in_path: status.in_path,
            can_install: status.can_install(),
            installed_at: status.installed_at,
            install_blocked: status.blocked,
            local_bin_off_path: status.local_bin_off_path,
        }
    }
}

/// `install_tui_command` / `uninstall_tui_command` 的返回值: 装/卸载操作最新一次做没做,
/// 外加操作完之后重新读到的状态。`cancelled` 只在 macOS 系统授权框被用户点了取消时为 true——
/// 那不是错误, 但前端需要知道「什么都没发生」而不是误以为成功或吞掉不吭声 (fix-1 R6)。
#[derive(Debug, Serialize)]
pub struct TuiInstallOutcome {
    pub info: TuiLaunchInfo,
    pub cancelled: bool,
}

fn sidecar_path_for(exe: &Path) -> Option<PathBuf> {
    exe.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|dir| dir.join(SIDECAR_NAME))
}

fn existing_sidecar(exe: &Path) -> Option<String> {
    sidecar_path_for(exe)
        // len > 0: build.rs 的 0 字节占位不算
        .filter(|p| std::fs::metadata(p).map(|m| m.is_file() && m.len() > 0).unwrap_or(false))
        .map(|p| p.to_string_lossy().into_owned())
}

fn current_sidecar() -> Option<String> {
    std::env::current_exe().ok().and_then(|exe| existing_sidecar(&exe))
}

/// 同步实现, 只从阻塞线程里调用 (`tui_install::status` 在 Windows 上会拉起一次 PowerShell, 是阻塞 IO)。
fn launch_info_blocking() -> TuiLaunchInfo {
    let path = current_sidecar();
    let status = match &path {
        Some(p) => tui_install::status(Path::new(p)),
        None => InstallStatus::unavailable(),
    };
    TuiLaunchInfo::new(path, crate::commands::app::is_appimage_runtime(), status)
}

/// `tui_launch_info` 会在 Windows 上拉起 PowerShell 读注册表, 是阻塞 IO——放进 `spawn_blocking`
/// 避免卡住 async 工作线程 (fix-1 R9)。join 失败 (阻塞线程 panic) 时退化成「不可用」而不是整个 command 报错。
#[tauri::command]
pub async fn tui_launch_info() -> TuiLaunchInfo {
    match tauri::async_runtime::spawn_blocking(launch_info_blocking).await {
        Ok(info) => info,
        Err(_) => TuiLaunchInfo::new(None, crate::commands::app::is_appimage_runtime(), InstallStatus::unavailable()),
    }
}

/// 安装 / 卸载共用: 在同一个阻塞线程闭包里做完「读状态 (可选 can_install 前置检查) → 执行 → 再读状态」,
/// 避免每次单独 `spawn_blocking` 各发一趟阻塞 IO (对 install 来说是一读一读, 不是三读, 见 fix-1 R9)。
async fn change_install(
    op: fn(&Path) -> tui_install::InstallResult,
    require_can_install: bool,
    err_prefix: &'static str,
) -> AppResult<TuiInstallOutcome> {
    let sidecar = current_sidecar().ok_or_else(|| AppError::BadRequest("此版本未包含终端界面程序".into()))?;
    let path = PathBuf::from(sidecar);
    let (outcome, info) = tauri::async_runtime::spawn_blocking(move || -> AppResult<(Outcome, TuiLaunchInfo)> {
        if require_can_install && !launch_info_blocking().can_install {
            return Err(AppError::BadRequest("当前无法添加到 PATH".into()));
        }
        let outcome = op(&path).map_err(|raw| AppError::Internal(format!("{err_prefix}: {raw}")))?;
        Ok((outcome, launch_info_blocking()))
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))??;
    Ok(TuiInstallOutcome { info, cancelled: outcome == Outcome::Cancelled })
}

#[tauri::command]
pub async fn install_tui_command() -> AppResult<TuiInstallOutcome> {
    change_install(tui_install::install, true, "添加到 PATH 失败").await
}

#[tauri::command]
pub async fn uninstall_tui_command() -> AppResult<TuiInstallOutcome> {
    change_install(tui_install::uninstall, false, "从 PATH 移除失败").await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_sits_next_to_the_main_executable() {
        let exe = Path::new("/Applications/cc-router.app/Contents/MacOS/cc-router");
        assert_eq!(
            sidecar_path_for(exe),
            Some(PathBuf::from("/Applications/cc-router.app/Contents/MacOS").join(SIDECAR_NAME))
        );
    }

    #[test]
    fn dto_flattens_the_install_status() {
        let status = InstallStatus {
            kind: InstallKind::Symlink,
            in_path: true,
            installed_at: Some("/usr/local/bin/cc-router-tui".into()),
            blocked: None,
            local_bin_off_path: false,
        };
        let json = serde_json::to_value(TuiLaunchInfo::new(Some("/a/cc-router-tui".into()), false, status)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "path": "/a/cc-router-tui",
                "is_appimage": false,
                "install_kind": "symlink",
                "in_path": true,
                "installed_at": "/usr/local/bin/cc-router-tui",
                "can_install": true,
                "install_blocked": null,
                "local_bin_off_path": false,
            })
        );
        let none = serde_json::to_value(TuiLaunchInfo::new(None, false, InstallStatus::unavailable())).unwrap();
        assert_eq!((&none["install_kind"], &none["can_install"]), (&serde_json::json!("unavailable"), &serde_json::json!(false)));
    }

    #[test]
    fn install_outcome_dto_wraps_info_with_a_cancelled_flag() {
        let status = InstallStatus::unavailable();
        let info = TuiLaunchInfo::new(None, false, status);
        let json = serde_json::to_value(TuiInstallOutcome { info, cancelled: true }).unwrap();
        assert_eq!(json["cancelled"], serde_json::json!(true));
        assert_eq!(json["info"]["install_kind"], serde_json::json!("unavailable"));
    }

    #[test]
    fn exe_without_parent_yields_none() {
        assert_eq!(sidecar_path_for(Path::new("")), None);
    }

    #[test]
    fn missing_or_placeholder_is_none_real_file_is_some() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("cc-router");
        assert_eq!(existing_sidecar(&exe), None, "文件不存在");

        // build.rs 在没跑过打包钩子时放的 0 字节占位: 不能当成「已包含 TUI」
        std::fs::write(dir.path().join(SIDECAR_NAME), b"").unwrap();
        assert_eq!(existing_sidecar(&exe), None, "0 字节占位");

        std::fs::write(dir.path().join(SIDECAR_NAME), b"\x7fELF").unwrap();
        assert_eq!(
            existing_sidecar(&exe),
            Some(dir.path().join(SIDECAR_NAME).to_string_lossy().into_owned())
        );
    }
}
