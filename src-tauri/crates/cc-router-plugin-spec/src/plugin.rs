//! 从插件目录读清单并按类型分派校验 (`ccr-plugin-check` 与 app 共用)。

use std::path::Path;

use crate::check::{CheckResult, Issue, IssueCode};

pub const MANIFEST_FILE: &str = "cc-router-plugin.json";

/// 由 Rust 校验的系统插件类型。与前端 / 主 crate 的 `SUPPORTED_TYPES` 是不同的两张表:
/// 那两张表是「app 能装哪些类型」, 这张是「其中哪些归 Rust 校验」。
pub const SYSTEM_TYPES: &[&str] = &[];
/// 由 TS 校验的类型 (主题 / 小票): Rust 侧只认出来, 不校验内容。
pub const TS_TYPES: &[&str] = &["theme", "receipt"];

pub fn is_system_type(t: &str) -> bool {
    SYSTEM_TYPES.contains(&t)
}

pub enum DirCheck {
    /// 主题 / 小票: 交给 TS 校验器
    Delegated,
    Checked(CheckResult),
    /// 清单读不到或不是 JSON
    Unreadable(String),
}

pub fn check_dir(dir: &Path) -> DirCheck {
    let raw = match std::fs::read(dir.join(MANIFEST_FILE)) {
        Ok(b) => b,
        Err(e) => return DirCheck::Unreadable(format!("cannot read {MANIFEST_FILE}: {e}")),
    };
    let manifest: serde_json::Value = match serde_json::from_slice(&raw) {
        Ok(v) => v,
        Err(e) => return DirCheck::Unreadable(format!("{MANIFEST_FILE} is not valid JSON: {e}")),
    };
    let Some(plugin_type) = manifest.get("type").and_then(|t| t.as_str()) else {
        return DirCheck::Checked(CheckResult::failed(vec![Issue::new("type", IssueCode::Missing)]));
    };
    if TS_TYPES.contains(&plugin_type) {
        return DirCheck::Delegated;
    }
    DirCheck::Checked(CheckResult::failed(vec![
        Issue::new("type", IssueCode::UnsupportedType).detail(plugin_type.to_string())
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir_with(manifest: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("ccr-spec-{}-{}", std::process::id(), manifest.len()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(MANIFEST_FILE), manifest).unwrap();
        d
    }

    #[test]
    fn themes_are_delegated_and_unknown_types_fail() {
        assert!(matches!(check_dir(&dir_with(r#"{"type":"theme"}"#)), DirCheck::Delegated));
        match check_dir(&dir_with(r#"{"type":"wasm-thing"}"#)) {
            DirCheck::Checked(r) => assert_eq!(r.issues[0].code, IssueCode::UnsupportedType),
            _ => panic!("expected a failed check"),
        }
        assert!(matches!(check_dir(Path::new("/nonexistent-ccr")), DirCheck::Unreadable(_)));
    }
}
