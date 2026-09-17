//! `app_data_dir/runtime.json`: 本次进程的运行事实, 给同机的 cc-router-tui 读。
//!
//! 与 settings.json 分开: 那个是用户配置, 这个每次启动覆盖重写。
//! `local_secret` 只存在于进程内存和这个文件, 见 docs/superpowers/specs/2026-09-17-tui-design.md §3.2。

use std::path::Path;

use serde::Serialize;

use crate::error::AppResult;

pub const FILE_NAME: &str = "runtime.json";

#[derive(Clone, Serialize)]
pub struct RuntimeFile {
    pub pid: u32,
    pub app_version: String,
    pub http_port: Option<u16>,
    pub https_port: Option<u16>,
    pub ca_pem_path: Option<String>,
    pub local_secret: String,
}

impl std::fmt::Debug for RuntimeFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeFile")
            .field("pid", &self.pid)
            .field("app_version", &self.app_version)
            .field("http_port", &self.http_port)
            .field("https_port", &self.https_port)
            .field("ca_pem_path", &self.ca_pem_path)
            .field("local_secret", &"<redacted>")
            .finish()
    }
}

/// 32 字节随机数的 base64url (无 padding)。
///
/// 刻意不引 `rand`: 两个 UUIDv4 拼起来是 32 字节, 其中每个 UUID 有 6 个固定的版本 / 变体位,
/// 实际熵 244 bit, 对一个只在回环上校验的密钥绰绰有余。uuid 内部走 getrandom (OS CSPRNG)。
pub fn generate_secret() -> String {
    use base64::Engine;
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

impl RuntimeFile {
    /// `ca_pem_path` 只在 HTTPS listener 起来时给: TUI 只有走 https 才需要信任本地 CA。
    pub fn new(
        app_data_dir: &Path,
        http_port: Option<u16>,
        https_port: Option<u16>,
        local_secret: &str,
    ) -> Self {
        Self {
            pid: std::process::id(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            http_port,
            https_port,
            ca_pem_path: https_port
                .map(|_| crate::tls::ca_pem_path(app_data_dir).to_string_lossy().into_owned()),
            local_secret: local_secret.to_string(),
        }
    }
}

/// 先以 0600 建临时文件写完, 再 rename 覆盖。
/// 直接 `fs::write` 再 chmod 会有一个「文件已含密钥但权限还是默认值」的窗口;
/// 而且覆盖一个已存在的宽权限文件时 `fs::write` 不会收紧权限。
pub fn write(app_data_dir: &Path, file: &RuntimeFile) -> AppResult<()> {
    use std::io::Write;

    let final_path = app_data_dir.join(FILE_NAME);
    let tmp_path = app_data_dir.join(format!("{FILE_NAME}.{}.tmp", std::process::id()));
    let body = serde_json::to_vec_pretty(file)?;

    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let result = (|| -> AppResult<()> {
        let mut f = opts.open(&tmp_path)?;
        f.write_all(&body)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp_path, &final_path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_is_43_chars_of_base64url_and_never_repeats() {
        let a = generate_secret();
        let b = generate_secret();
        // 32 字节 → base64url 无 padding = ceil(32 * 4 / 3) = 43 字符
        assert_eq!(a.len(), 43);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'), "{a}");
        assert_ne!(a, b);
    }

    #[test]
    fn ca_path_is_present_only_when_https_listener_is_up() {
        let dir = std::path::Path::new("/data");
        let http_only = RuntimeFile::new(dir, Some(23456), None, "s");
        assert_eq!(http_only.ca_pem_path, None);
        let https = RuntimeFile::new(dir, None, Some(23457), "s");
        assert_eq!(
            https.ca_pem_path.as_deref(),
            Some(crate::tls::ca_pem_path(dir).to_string_lossy().as_ref())
        );
        assert_eq!(https.http_port, None);
        assert_eq!(https.local_secret, "s");
        assert_eq!(https.app_version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn write_produces_parseable_json_and_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), &RuntimeFile::new(dir.path(), Some(1), None, "first")).unwrap();
        write(dir.path(), &RuntimeFile::new(dir.path(), Some(2), None, "second")).unwrap();
        let raw = std::fs::read_to_string(dir.path().join(FILE_NAME)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["http_port"], 2);
        assert_eq!(v["local_secret"], "second");
        assert!(v["https_port"].is_null());
        // 临时文件不应残留
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != FILE_NAME)
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[cfg(unix)]
    #[test]
    fn file_is_owner_only_even_when_replacing_a_world_readable_one() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        // 模拟一个权限过宽的旧文件 (例如被用户手动 chmod 过)
        std::fs::write(&path, "{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        write(dir.path(), &RuntimeFile::new(dir.path(), Some(1), None, "s")).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{mode:o}");
    }

    /// 字面量 `local_secret` 只允许出现在这几个文件里。这是一道「字段名」层面的栅栏:
    /// 它拦得住有人在 commands/ 或 DTO 里直接读 `state.local_secret`, 但拦不住先在白名单文件里
    /// 加一个不同名的访问器再从别处调用 —— 加访问器本身就应该在 review 里被质疑。
    #[test]
    fn local_secret_is_only_touched_by_allowlisted_files() {
        const ALLOWED: &[&str] = &[
            "runtime_file.rs",
            "state.rs",
            "lib.rs",
            "proxy/server.rs",
            "proxy/web/gate.rs",
            "proxy/web/auth.rs",
        ];
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().filter_map(Result::ok) {
                let p = entry.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|e| e == "rs") {
                    out.push(p);
                }
            }
        }
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk(&src, &mut files);
        let offenders: Vec<String> = files
            .iter()
            .filter(|p| std::fs::read_to_string(p).unwrap().contains("local_secret"))
            .map(|p| p.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/"))
            .filter(|rel| !ALLOWED.contains(&rel.as_str()))
            .collect();
        assert!(offenders.is_empty(), "local_secret 出现在白名单之外: {offenders:?}");
    }

    #[test]
    fn settings_serialization_never_contains_the_secret_field() {
        let raw = serde_json::to_string(&crate::settings::model::Settings::default()).unwrap();
        assert!(!raw.contains("local_secret"), "{raw}");
    }

    #[test]
    fn debug_output_redacts_the_secret() {
        let f = RuntimeFile::new(std::path::Path::new("/data"), Some(1), None, "super-secret-value");
        let shown = format!("{f:?}");
        assert!(!shown.contains("super-secret-value"), "{shown}");
        assert!(shown.contains("<redacted>"), "{shown}");
        assert!(shown.contains("http_port"), "{shown}");
    }

    #[test]
    fn write_into_a_missing_directory_errors_and_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        let f = RuntimeFile::new(&missing, Some(1), None, "s");
        assert!(write(&missing, &f).is_err());
        assert!(!missing.exists());
    }
}
