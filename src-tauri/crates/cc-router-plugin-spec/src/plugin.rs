//! 从插件目录读清单并按类型分派校验 (`ccr-plugin-check` 与 app 共用)。

use std::path::Path;

use crate::check::{Capabilities, CheckResult, Issue, IssueCode};
use crate::provider_plugin::{self, ProviderPlugin};

pub const MANIFEST_FILE: &str = "cc-router-plugin.json";

/// 由 Rust 校验的系统插件类型。与前端 / 主 crate 的 `SUPPORTED_TYPES` 是不同的两张表:
/// 那两张表是「app 能装哪些类型」, 这张是「其中哪些归 Rust 校验」。
pub const SYSTEM_TYPES: &[&str] = &["provider"];
/// 由 TS 校验的类型 (主题 / 小票): Rust 侧只认出来, 不校验内容。
pub const TS_TYPES: &[&str] = &["theme", "receipt"];

pub fn is_system_type(t: &str) -> bool {
    SYSTEM_TYPES.contains(&t)
}

/// 校验通过时可以直接拿去用的解析结果 (app 用来装进注册表)。
#[derive(Debug, Clone)]
pub enum SystemPlugin {
    Provider(ProviderPlugin),
}

/// 校验一个系统插件的入口文件。`assets` 是清单里列出的资源 (系统插件第一期都不允许带资源)。
pub fn check_system(plugin_type: &str, plugin_id: &str, entry: &str, assets: &[String]) -> (CheckResult, Option<SystemPlugin>) {
    if !assets.is_empty() {
        return (CheckResult::failed(vec![Issue::new("assets", IssueCode::TooMany).detail("system plugins cannot ship assets")]), None);
    }
    match plugin_type {
        "provider" => match provider_plugin::parse(entry, plugin_id) {
            Ok(p) => (CheckResult::passed(Capabilities::Provider(p.capabilities.clone())), Some(SystemPlugin::Provider(p))),
            Err(issues) => (CheckResult::failed(issues), None),
        },
        other => (CheckResult::failed(vec![Issue::new("type", IssueCode::UnsupportedType).detail(other.to_string())]), None),
    }
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
    if !is_system_type(plugin_type) {
        return DirCheck::Checked(CheckResult::failed(vec![
            Issue::new("type", IssueCode::UnsupportedType).detail(plugin_type.to_string())
        ]));
    }
    let str_field = |k: &str| manifest.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let (Some(id), Some(entry)) = (str_field("id"), str_field("entry")) else {
        return DirCheck::Checked(CheckResult::failed(vec![Issue::new("id / entry", IssueCode::Missing)]));
    };
    // 清单本身的完整规则 (id 格式、路径规则、minApp) 由 TS 校验器与 app 负责; 这里只防路径逃逸
    if entry.contains("..") || entry.starts_with('/') || entry.contains('\\') {
        return DirCheck::Checked(CheckResult::failed(vec![Issue::new("entry", IssueCode::BadPath)]));
    }
    let assets: Vec<String> = manifest
        .get("assets")
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let text = match std::fs::read_to_string(dir.join(&entry)) {
        Ok(t) => t,
        Err(e) => return DirCheck::Checked(CheckResult::failed(vec![Issue::new("entry", IssueCode::Missing).detail(e.to_string())])),
    };
    DirCheck::Checked(check_system(plugin_type, &id, &text, &assets).0)
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

    /// 仓库里的范例插件 (`examples/plugins/*`) 中由 Rust 校验的那些必须零问题。
    #[test]
    fn example_system_plugins_pass() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../examples/plugins");
        let mut checked = 0;
        for entry in std::fs::read_dir(&root).expect("examples/plugins exists") {
            let dir = entry.unwrap().path();
            match check_dir(&dir) {
                DirCheck::Delegated => {}
                DirCheck::Checked(r) => {
                    assert!(r.ok, "{}: {:?}", dir.display(), r.issues);
                    checked += 1;
                }
                DirCheck::Unreadable(m) => panic!("{}: {m}", dir.display()),
            }
        }
        assert!(checked >= 1, "至少有一个系统插件范例");
    }
}
