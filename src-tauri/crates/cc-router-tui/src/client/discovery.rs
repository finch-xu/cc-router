//! 找到正在运行的 cc-router: 数据目录规则 + 读 runtime.json。
//!
//! 数据目录必须与 Tauri 对 identifier 的解析结果一致, 由主 crate 的契约测试锁住
//! (`tui_contract::identifier_matches_tauri_conf`)。

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// 必须等于 src-tauri/tauri.conf.json 的 `identifier`。
pub const IDENTIFIER: &str = "com.cc-router.desktop";
pub const RUNTIME_FILE: &str = "runtime.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Linux,
    Windows,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DiscoveryError {
    #[error("无法确定数据目录: 环境变量 {0} 未设置")]
    MissingEnv(&'static str),
    #[error("未找到 {0}")]
    NoRuntimeFile(PathBuf),
    #[error("{0} 已损坏: {1}")]
    Corrupt(PathBuf, String),
    #[error("{0} 里没有可用端口")]
    NoPort(PathBuf),
}

/// `env` 注入是为了测试; 生产传 `|k| std::env::var(k).ok()`。空字符串视同未设置。
pub fn default_data_dir(
    platform: Platform,
    env: impl Fn(&str) -> Option<String>,
) -> Result<PathBuf, DiscoveryError> {
    let get = |k: &'static str| env(k).filter(|v| !v.is_empty());
    let need = |k: &'static str| get(k).ok_or(DiscoveryError::MissingEnv(k));
    let base = match platform {
        Platform::MacOs => PathBuf::from(need("HOME")?).join("Library/Application Support"),
        Platform::Windows => PathBuf::from(need("APPDATA")?),
        Platform::Linux => match get("XDG_DATA_HOME") {
            Some(x) => PathBuf::from(x),
            None => PathBuf::from(need("HOME")?).join(".local/share"),
        },
    };
    Ok(base.join(IDENTIFIER))
}

/// runtime.json 的视图。未知字段忽略, 这样 app 先升级加字段时旧 TUI 不会读不了。
#[derive(Clone, Deserialize, PartialEq, Eq)]
pub struct RuntimeInfo {
    pub pid: u32,
    pub app_version: String,
    pub http_port: Option<u16>,
    pub https_port: Option<u16>,
    pub ca_pem_path: Option<String>,
    pub local_secret: String,
}

/// 手写 Debug: 密钥不能进日志 / panic 信息。
impl std::fmt::Debug for RuntimeInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeInfo")
            .field("pid", &self.pid)
            .field("app_version", &self.app_version)
            .field("http_port", &self.http_port)
            .field("https_port", &self.https_port)
            .field("ca_pem_path", &self.ca_pem_path)
            .field("local_secret", &"<redacted>")
            .finish()
    }
}

impl RuntimeInfo {
    /// 优先 HTTP (不需要处理证书); 只开了 HTTPS 时才走 https。
    pub fn base_url(&self) -> Option<String> {
        match (self.http_port, self.https_port) {
            (Some(p), _) => Some(format!("http://127.0.0.1:{p}")),
            (None, Some(p)) => Some(format!("https://127.0.0.1:{p}")),
            (None, None) => None,
        }
    }
}

pub fn read_runtime(data_dir: &Path) -> Result<RuntimeInfo, DiscoveryError> {
    let path = data_dir.join(RUNTIME_FILE);
    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        Err(_) => return Err(DiscoveryError::NoRuntimeFile(path)),
    };
    let info: RuntimeInfo =
        serde_json::from_str(&raw).map_err(|e| DiscoveryError::Corrupt(path.clone(), e.to_string()))?;
    if info.base_url().is_none() {
        return Err(DiscoveryError::NoPort(path));
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn data_dir_follows_tauri_rules_per_platform() {
        assert_eq!(
            default_data_dir(Platform::MacOs, env_of(&[("HOME", "/Users/u")])).unwrap(),
            PathBuf::from("/Users/u/Library/Application Support/com.cc-router.desktop")
        );
        assert_eq!(
            default_data_dir(Platform::Linux, env_of(&[("HOME", "/home/u")])).unwrap(),
            PathBuf::from("/home/u/.local/share/com.cc-router.desktop")
        );
        assert_eq!(
            default_data_dir(Platform::Linux, env_of(&[("HOME", "/home/u"), ("XDG_DATA_HOME", "/x")])).unwrap(),
            PathBuf::from("/x/com.cc-router.desktop")
        );
        assert_eq!(
            default_data_dir(Platform::Windows, env_of(&[("APPDATA", "C:\\Users\\u\\AppData\\Roaming")])).unwrap(),
            PathBuf::from("C:\\Users\\u\\AppData\\Roaming").join("com.cc-router.desktop")
        );
    }

    #[test]
    fn empty_env_value_counts_as_unset() {
        assert_eq!(
            default_data_dir(Platform::Linux, env_of(&[("HOME", "/home/u"), ("XDG_DATA_HOME", "")])).unwrap(),
            PathBuf::from("/home/u/.local/share/com.cc-router.desktop")
        );
        assert_eq!(
            default_data_dir(Platform::MacOs, env_of(&[("HOME", "")])),
            Err(DiscoveryError::MissingEnv("HOME"))
        );
    }

    const SAMPLE: &str = r#"{"pid":7,"app_version":"5.1.0","http_port":23456,"https_port":null,
        "ca_pem_path":null,"local_secret":"abc","some_future_field":true}"#;

    #[test]
    fn reads_runtime_file_and_ignores_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(RUNTIME_FILE), SAMPLE).unwrap();
        let info = read_runtime(dir.path()).unwrap();
        assert_eq!(info.pid, 7);
        assert_eq!(info.base_url().as_deref(), Some("http://127.0.0.1:23456"));
    }

    #[test]
    fn https_only_runtime_uses_https() {
        let info: RuntimeInfo = serde_json::from_str(
            r#"{"pid":1,"app_version":"x","http_port":null,"https_port":23457,"ca_pem_path":"/d/tls/ca.pem","local_secret":"s"}"#,
        )
        .unwrap();
        assert_eq!(info.base_url().as_deref(), Some("https://127.0.0.1:23457"));
    }

    #[test]
    fn missing_corrupt_and_portless_files_are_distinct_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(RUNTIME_FILE);
        assert_eq!(read_runtime(dir.path()), Err(DiscoveryError::NoRuntimeFile(path.clone())));
        std::fs::write(&path, "{not json").unwrap();
        assert!(matches!(read_runtime(dir.path()), Err(DiscoveryError::Corrupt(..))));
        std::fs::write(&path, r#"{"pid":1,"app_version":"x","http_port":null,"https_port":null,"ca_pem_path":null,"local_secret":"s"}"#).unwrap();
        assert_eq!(read_runtime(dir.path()), Err(DiscoveryError::NoPort(path)));
    }

    #[test]
    fn debug_never_prints_the_secret() {
        let info: RuntimeInfo = serde_json::from_str(SAMPLE).unwrap();
        let shown = format!("{info:?}");
        assert!(!shown.contains("abc"), "{shown}");
        assert!(shown.contains("<redacted>"));
    }
}
