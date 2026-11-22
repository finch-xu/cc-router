//! 插件 (第一期: 界面主题)。设计: docs/superpowers/specs/2026-11-22-plugin-store-design.md,
//! 实施: docs/superpowers/plans/2026-11-22-plugin-store.md。
//!
//! 分工: **下载与落盘在 Rust, 内容校验在 TS** (`src/plugins/`, 社区仓库 CI 跑同一份)。
//! Rust 这边只管「不会伤到宿主机」的那部分: 路径不越界、文件头与扩展名一致、SVG 不带脚本 / 外链、
//! 体积上限、清单的最小结构。主题值是否合法、能不能拼进 CSS, 由前端在安装确认前和每次应用前校验。
//!
//! 插件只装数据不装代码, 所以「从链接安装」不需要额外门槛 (设计 §6.1)。

pub mod config;
pub mod files;
pub mod github;
pub mod link;
pub mod runtime;
pub mod store;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{AppError, AppResult};

pub const MANIFEST_FILE: &str = "cc-router-plugin.json";
/// 社区目录 (CI 在 cc-router-community 仓库 main 上生成)。
pub const COMMUNITY_INDEX_URL: &str =
    "https://raw.githubusercontent.com/finch-xu/cc-router-community/main/index.json";

pub const MAX_ASSETS: usize = 128;
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
pub const MAX_ENTRY_BYTES: u64 = 1024 * 1024;
pub const MAX_FILE_BYTES: u64 = 30 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 60 * 1024 * 1024;
pub const MAX_INDEX_BYTES: u64 = 4 * 1024 * 1024;

/// 本版 app 能加载的插件类型 (与 `src/plugins/manifest.ts::SUPPORTED_TYPES` 一致)。
pub const SUPPORTED_TYPES: &[&str] = &["theme", "receipt", "provider"];
/// 不能当插件 id 的名字 (与 `src/plugins/manifest.ts::RESERVED_IDS` 一致, `pnpm check:plugins` 比对两边)。
pub const RESERVED_IDS: &[&str] = &["classic", "sketch", "win2000", "win2k", "quiet", "sparrow", "plain", "cc-router"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// 从 GitHub 链接安装 (未经社区审核)
    Github,
    /// 从社区目录安装 (包哈希与目录一致)
    Community,
    /// 从本地文件夹加载
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginSource {
    pub kind: SourceKind,
    /// `owner/repo` (GitHub / 社区) 或本地文件夹的绝对路径
    pub location: String,
    /// 仓库内子目录 (GitHub / 社区)
    #[serde(default)]
    pub path: Option<String>,
    /// 40 位提交号 (GitHub / 社区)
    #[serde(default)]
    pub commit: Option<String>,
}

/// Rust 需要的那几个清单字段; 完整校验在 TS。原文以 `Value` 原样存下交给前端。
#[derive(Debug, Clone)]
pub struct ManifestCore {
    pub id: String,
    pub plugin_type: String,
    pub version: String,
    pub entry: String,
    pub assets: Vec<String>,
}

pub fn is_valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    (2..=40).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// 比较 a.b.c (忽略预发布后缀); 解析不了的段按 0。
fn version_lt(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> [u64; 3] {
        let mut out = [0; 3];
        for (i, p) in s.split('-').next().unwrap_or("").split('.').take(3).enumerate() {
            out[i] = p.parse().unwrap_or(0);
        }
        out
    };
    parse(a) < parse(b)
}

pub fn parse_manifest(bytes: &[u8]) -> AppResult<(ManifestCore, Value)> {
    let bad = |m: &str| AppError::BadRequest(format!("插件清单 {MANIFEST_FILE} 不合法: {m}"));
    let raw: Value = serde_json::from_slice(bytes).map_err(|e| bad(&format!("不是合法 JSON ({e})")))?;
    let obj = raw.as_object().ok_or_else(|| bad("顶层必须是对象"))?;
    if obj.get("manifest").and_then(Value::as_u64) != Some(1) {
        return Err(bad("manifest 必须是 1 (可能需要更新 cc-router)"));
    }
    let s = |k: &str| obj.get(k).and_then(Value::as_str).map(str::to_string);
    let id = s("id").filter(|i| is_valid_id(i)).ok_or_else(|| bad("id 缺失或格式不对"))?;
    if RESERVED_IDS.contains(&id.as_str()) {
        return Err(bad("id 与内置主题重名"));
    }
    let plugin_type = s("type").ok_or_else(|| bad("缺少 type"))?;
    if !SUPPORTED_TYPES.contains(&plugin_type.as_str()) {
        return Err(AppError::BadRequest(format!("当前版本的 cc-router 不支持「{plugin_type}」类型的插件")));
    }
    let version = s("version").ok_or_else(|| bad("缺少 version"))?;
    let entry = s("entry").filter(|e| files::is_safe_path(e)).ok_or_else(|| bad("entry 缺失或路径不合法"))?;
    let mut assets: Vec<String> = Vec::new();
    if let Some(list) = obj.get("assets") {
        let list = list.as_array().ok_or_else(|| bad("assets 必须是数组"))?;
        if list.len() > MAX_ASSETS {
            return Err(bad(&format!("assets 最多 {MAX_ASSETS} 个")));
        }
        for a in list {
            let a = a.as_str().filter(|a| files::is_safe_path(a)).ok_or_else(|| bad("assets 里有不合法的路径"))?;
            if files::mime_of(a).is_none() {
                return Err(bad(&format!("不支持的资源类型: {a}")));
            }
            if a == entry || assets.iter().any(|x| x == a) {
                return Err(bad(&format!("资源重复: {a}")));
            }
            assets.push(a.to_string());
        }
    }
    if let Some(min) = s("minApp") {
        if version_lt(env!("CARGO_PKG_VERSION"), &min) {
            return Err(AppError::BadRequest(format!("这个插件需要 cc-router {min} 或更新版本")));
        }
    }
    Ok((ManifestCore { id, plugin_type, version, entry, assets }, raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(extra: &str) -> Vec<u8> {
        format!(
            r#"{{"manifest":1,"id":"demo-x","type":"theme","version":"1.0.0","name":"Demo","author":"a","entry":"theme.json"{extra}}}"#
        )
        .into_bytes()
    }

    #[test]
    fn parses_minimal_manifest() {
        let (m, _) = parse_manifest(&manifest(r#","assets":["a.png","f/x.woff2"]"#)).unwrap();
        assert_eq!(m.id, "demo-x");
        assert_eq!(m.assets, vec!["a.png", "f/x.woff2"]);
    }

    #[test]
    fn rejects_bad_manifests() {
        for extra in [
            r#","assets":["../a.png"]"#,
            r#","assets":["a.exe"]"#,
            r#","assets":["a.png","a.png"]"#,
            r#","minApp":"999.0.0""#,
        ] {
            assert!(parse_manifest(&manifest(extra)).is_err(), "{extra}");
        }
        let swap = |from: &str, to: &str| String::from_utf8(manifest("")).unwrap().replace(from, to).into_bytes();
        assert!(parse_manifest(&swap(r#""id":"demo-x""#, r#""id":"quiet""#)).is_err());
        assert!(parse_manifest(&swap(r#""id":"demo-x""#, r#""id":"Demo""#)).is_err());
        assert!(parse_manifest(&swap(r#""type":"theme""#, r#""type":"system""#)).is_err());
        assert!(parse_manifest(&swap(r#""manifest":1"#, r#""manifest":2"#)).is_err());
        assert!(parse_manifest(&swap(r#""entry":"theme.json""#, r#""entry":"/etc/passwd""#)).is_err());
    }

    #[test]
    fn version_compare() {
        assert!(version_lt("6.7.0", "6.8.0"));
        assert!(!version_lt("6.8.0", "6.8.0"));
        assert!(!version_lt("10.0.0", "9.9.9"));
    }
}
