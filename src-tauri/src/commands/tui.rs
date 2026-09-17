//! 终端界面 (cc-router-tui) 相关 command. 一键安装到 PATH 在 P2 加, 见 spec §3.5 / §7.2。

use std::path::{Path, PathBuf};

use serde::Serialize;

/// sidecar 的文件名。Tauri 的 externalBin 打包时会去掉 target triple 后缀,
/// 安装后它与主程序同目录 (macOS: Contents/MacOS/, Windows: 安装目录, deb: /usr/bin/)。
const SIDECAR_NAME: &str = if cfg!(windows) { "cc-router-tui.exe" } else { "cc-router-tui" };

#[derive(Debug, Serialize)]
pub struct TuiLaunchInfo {
    /// sidecar 的绝对路径; 不存在 (dev 构建 / 未打包 sidecar 的版本) 为 None
    pub path: Option<String>,
    /// AppImage 运行时 path 每次启动都变, 前端据此换一套说明
    pub is_appimage: bool,
}

fn sidecar_path_for(exe: &Path) -> Option<PathBuf> {
    exe.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|dir| dir.join(SIDECAR_NAME))
}

fn existing_sidecar(exe: &Path) -> Option<String> {
    sidecar_path_for(exe)
        .filter(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn tui_launch_info() -> TuiLaunchInfo {
    TuiLaunchInfo {
        path: std::env::current_exe().ok().and_then(|exe| existing_sidecar(&exe)),
        is_appimage: crate::commands::app::is_appimage_runtime(),
    }
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
    fn exe_without_parent_yields_none() {
        assert_eq!(sidecar_path_for(Path::new("")), None);
    }

    #[test]
    fn missing_file_is_reported_as_none_existing_file_as_some() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("cc-router");
        assert_eq!(existing_sidecar(&exe), None);
        std::fs::write(dir.path().join(SIDECAR_NAME), b"").unwrap();
        assert_eq!(
            existing_sidecar(&exe),
            Some(dir.path().join(SIDECAR_NAME).to_string_lossy().into_owned())
        );
    }
}
