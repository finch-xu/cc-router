//! 插件在磁盘上的布局与生命周期:
//!
//! ```text
//! app_data_dir/plugins/
//!   <id>/                       已安装: 清单 + 入口 + 资源 (保持作者仓库里的相对路径) + .ccr-install.json
//!   .staging/<uuid>/            暂存: 下载 / 读取完、等用户确认的包
//! ```
//!
//! 暂存表在进程内存里 (1 小时过期); 进程重启后首次用到时清空 `.staging/`。
//! 安装 = 暂存目录整体改名, 与旧版本互换后删旧, 中途失败不会留下半个插件。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::files::{check_asset, data_url, is_safe_path, is_text_asset, package_hash};
use super::{is_valid_id, parse_manifest, ManifestCore, PluginSource, SourceKind, MANIFEST_FILE, MAX_ENTRY_BYTES, MAX_FILE_BYTES, MAX_MANIFEST_BYTES, MAX_TOTAL_BYTES};
use crate::error::{AppError, AppResult};
use cc_router_plugin_spec::check::{CapabilityDiff, CheckResult};
use cc_router_plugin_spec::plugin::{check_system, is_system_type};

const INSTALL_META: &str = ".ccr-install.json";
const STAGING_TTL: Duration = Duration::from_secs(60 * 60);

pub fn plugins_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("plugins")
}

/// 一个包的全部文件 (清单 + 入口 + 资源), 路径 → 内容。
pub type Files = BTreeMap<String, Vec<u8>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetInfo {
    pub path: String,
    pub size: u64,
}

/// 暂存结果, 交给前端做内容校验与确认框。
#[derive(Debug, Clone, Serialize)]
pub struct StagedPlugin {
    pub staging_id: String,
    pub manifest: Value,
    /// 入口文件文本 (主题 = theme.json)
    pub entry: String,
    pub assets: Vec<AssetInfo>,
    /// 文本资源 (html / css) 原文: 前端在确认框里校验与预览
    pub texts: BTreeMap<String, String>,
    pub total_size: u64,
    pub sha256: String,
    pub source: PluginSource,
    /// 本机已装的同 id 插件 (覆盖安装 / 来源不同的提示用)
    pub installed: Option<InstalledPlugin>,
    /// 系统插件 (厂商等) 的 Rust 校验结果: 问题、能力摘要、与已装版本的能力差异。主题 / 小票为 None
    /// (它们由前端校验)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<CheckResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPlugin {
    pub id: String,
    pub plugin_type: String,
    pub version: String,
    pub manifest: Value,
    pub source: PluginSource,
    pub sha256: String,
    pub installed_at: i64,
    pub size: u64,
}

/// `load_plugin` 的结果: 应用主题需要的一切。
#[derive(Debug, Clone, Serialize)]
pub struct PluginContent {
    pub manifest: Value,
    pub entry: String,
    /// 二进制资源路径 → data URL
    pub assets: BTreeMap<String, String>,
    /// 文本资源 (html / css) 路径 → 原文
    pub texts: BTreeMap<String, String>,
}

struct Staged {
    dir: PathBuf,
    core: ManifestCore,
    manifest: Value,
    source: PluginSource,
    sha256: String,
    total: u64,
    created: Instant,
}

static STAGED: LazyLock<Mutex<HashMap<String, Staged>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static STAGING_SWEPT: Mutex<bool> = Mutex::new(false);

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn staged_lock() -> std::sync::MutexGuard<'static, HashMap<String, Staged>> {
    STAGED.lock().unwrap_or_else(|e| e.into_inner())
}

/// 清掉过期暂存; 进程里第一次调用时把整个 `.staging/` 清空 (上次进程留下的)。
fn sweep(root: &Path) {
    let staging = root.join(".staging");
    {
        let mut swept = STAGING_SWEPT.lock().unwrap_or_else(|e| e.into_inner());
        if !*swept {
            let _ = std::fs::remove_dir_all(&staging);
            *swept = true;
        }
    }
    let mut map = staged_lock();
    map.retain(|_, s| {
        let keep = s.created.elapsed() < STAGING_TTL;
        if !keep {
            let _ = std::fs::remove_dir_all(&s.dir);
        }
        keep
    });
}

/// 收齐的包 → 校验 → 写进暂存目录 → 登记。`files` 必须含清单、入口与清单列出的全部资源, 不多不少。
pub fn stage(app_data_dir: &Path, files: Files, source: PluginSource) -> AppResult<StagedPlugin> {
    let root = plugins_dir(app_data_dir);
    sweep(&root);
    let manifest_bytes = files.get(MANIFEST_FILE).ok_or_else(|| AppError::BadRequest(format!("缺少 {MANIFEST_FILE}")))?;
    let (core, manifest) = parse_manifest(manifest_bytes)?;
    let entry_bytes = files.get(&core.entry).ok_or_else(|| AppError::BadRequest(format!("缺少入口文件 {}", core.entry)))?;
    let entry = String::from_utf8(entry_bytes.clone()).map_err(|_| AppError::BadRequest(format!("{} 不是 UTF-8 文本", core.entry)))?;
    let mut total = 0u64;
    let mut assets = Vec::new();
    let mut texts = BTreeMap::new();
    for (path, bytes) in &files {
        if !is_safe_path(path) {
            return Err(AppError::BadRequest(format!("不合法的文件路径: {path}")));
        }
        let expected = path == MANIFEST_FILE || *path == core.entry || core.assets.contains(path);
        if !expected {
            return Err(AppError::BadRequest(format!("包里有清单没列出的文件: {path}")));
        }
        total += bytes.len() as u64;
        if core.assets.contains(path) {
            check_asset(path, bytes).map_err(AppError::BadRequest)?;
            assets.push(AssetInfo { path: path.clone(), size: bytes.len() as u64 });
            if is_text_asset(path) {
                texts.insert(path.clone(), String::from_utf8_lossy(bytes).into_owned());
            }
        }
    }
    for a in &core.assets {
        if !files.contains_key(a) {
            return Err(AppError::BadRequest(format!("缺少资源文件 {a}")));
        }
    }
    if total > MAX_TOTAL_BYTES {
        return Err(AppError::BadRequest(format!("插件总大小超过 {} MB", MAX_TOTAL_BYTES / 1024 / 1024)));
    }
    let sha256 = package_hash(&files);

    let staging_id = Uuid::new_v4().to_string();
    let dir = root.join(".staging").join(&staging_id);
    for (path, bytes) in &files {
        let target = dir.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(target, bytes)?;
    }
    let installed = list(app_data_dir)?.into_iter().find(|p| p.id == core.id);
    let system = is_system_type(&core.plugin_type).then(|| {
        let (mut result, _) = check_system(&core.plugin_type, &core.id, &entry, &core.assets);
        // 与已装版本比能力 (同类型才比; 旧版本读不出来就不给差异)
        if let (Some(new_caps), Some(old)) = (&result.capabilities, installed.as_ref()) {
            if old.plugin_type == core.plugin_type {
                if let Ok((old_core, old_entry)) = read_entry(app_data_dir, &old.id) {
                    if let (CheckResult { capabilities: Some(old_caps), .. }, _) =
                        check_system(&old_core.plugin_type, &old_core.id, &old_entry, &old_core.assets)
                    {
                        result.diff = Some(CapabilityDiff::between(&old_caps, new_caps));
                    }
                }
            }
        }
        result
    });
    let out = StagedPlugin {
        staging_id: staging_id.clone(),
        manifest: manifest.clone(),
        entry,
        assets,
        texts,
        total_size: total,
        sha256: sha256.clone(),
        source: source.clone(),
        installed,
        system,
    };
    staged_lock().insert(staging_id, Staged { dir, core, manifest, source, sha256, total, created: Instant::now() });
    Ok(out)
}

/// 本地文件夹 → `Files`。只读清单列出的文件; 每个文件规范化后必须仍在文件夹里 (挡住符号链接逃逸)。
pub fn read_dir_package(dir: &Path) -> AppResult<Files> {
    let base = dir.canonicalize().map_err(|_| AppError::BadRequest("找不到这个文件夹".into()))?;
    let read = |rel: &str, max: u64| -> AppResult<Vec<u8>> {
        let p = base.join(rel).canonicalize().map_err(|_| AppError::BadRequest(format!("缺少文件 {rel}")))?;
        if !p.starts_with(&base) {
            return Err(AppError::BadRequest(format!("{rel} 指向文件夹之外")));
        }
        if std::fs::metadata(&p)?.len() > max {
            return Err(AppError::BadRequest(format!("{rel} 超过大小上限")));
        }
        Ok(std::fs::read(p)?)
    };
    let mut files = Files::new();
    let manifest = read(MANIFEST_FILE, MAX_MANIFEST_BYTES)?;
    let (core, _) = parse_manifest(&manifest)?;
    files.insert(MANIFEST_FILE.into(), manifest);
    files.insert(core.entry.clone(), read(&core.entry, MAX_ENTRY_BYTES)?);
    for a in &core.assets {
        files.insert(a.clone(), read(a, MAX_FILE_BYTES)?);
    }
    Ok(files)
}

/// 把暂存的包装上。`community_sha256` 给了就必须一致, 来源记为 community。
pub fn install(app_data_dir: &Path, staging_id: &str, community_sha256: Option<&str>) -> AppResult<InstalledPlugin> {
    let staged = staged_lock()
        .remove(staging_id)
        .ok_or_else(|| AppError::BadRequest("这次下载已过期, 请重新获取".into()))?;
    let mut source = staged.source.clone();
    // 系统插件在代理里执行: 不信前端的确认结果, 装之前在 Rust 里再校验一次
    if is_system_type(&staged.core.plugin_type) {
        let entry = std::fs::read_to_string(staged.dir.join(&staged.core.entry))?;
        let (result, _) = check_system(&staged.core.plugin_type, &staged.core.id, &entry, &staged.core.assets);
        if !result.ok {
            let _ = std::fs::remove_dir_all(&staged.dir);
            return Err(AppError::BadRequest(format!("插件内容有 {} 处问题, 无法安装", result.issues.len().max(1))));
        }
    }
    if let Some(expected) = community_sha256 {
        if !expected.eq_ignore_ascii_case(&staged.sha256) {
            let _ = std::fs::remove_dir_all(&staged.dir);
            return Err(AppError::BadRequest("下载到的内容与社区目录记录的不一致, 已取消安装".into()));
        }
        source.kind = SourceKind::Community;
    }
    let root = plugins_dir(app_data_dir);
    let record = InstalledPlugin {
        id: staged.core.id.clone(),
        plugin_type: staged.core.plugin_type.clone(),
        version: staged.core.version.clone(),
        manifest: staged.manifest,
        source,
        sha256: staged.sha256,
        installed_at: now_ms(),
        size: staged.total,
    };
    std::fs::write(staged.dir.join(INSTALL_META), serde_json::to_vec_pretty(&record)?)?;

    let target = root.join(&record.id);
    let old = root.join(format!(".old-{}-{}", record.id, Uuid::new_v4()));
    let had_old = target.exists();
    if had_old {
        std::fs::rename(&target, &old)?;
    }
    if let Err(e) = std::fs::rename(&staged.dir, &target) {
        if had_old {
            let _ = std::fs::rename(&old, &target);
        }
        return Err(e.into());
    }
    if had_old {
        let _ = std::fs::remove_dir_all(&old);
    }
    Ok(record)
}

pub fn discard(staging_id: &str) {
    if let Some(s) = staged_lock().remove(staging_id) {
        let _ = std::fs::remove_dir_all(s.dir);
    }
}

pub fn list(app_data_dir: &Path) -> AppResult<Vec<InstalledPlugin>> {
    let root = plugins_dir(app_data_dir);
    let Ok(rd) = std::fs::read_dir(&root) else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !is_valid_id(&name) {
            continue;
        }
        let Ok(bytes) = std::fs::read(entry.path().join(INSTALL_META)) else { continue };
        match serde_json::from_slice::<InstalledPlugin>(&bytes) {
            Ok(rec) if rec.id == name => out.push(rec),
            _ => tracing::warn!(plugin = %name, "插件安装记录损坏, 已跳过"),
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

fn installed_dir(app_data_dir: &Path, id: &str) -> AppResult<PathBuf> {
    if !is_valid_id(id) {
        return Err(AppError::BadRequest("插件 id 不合法".into()));
    }
    let dir = plugins_dir(app_data_dir).join(id);
    if !dir.join(INSTALL_META).exists() {
        return Err(AppError::BadRequest(format!("没有安装插件 {id}")));
    }
    Ok(dir)
}

/// 读出一个已安装插件 (每次都按清单重新检查路径与文件头, 不信任磁盘)。
pub fn load(app_data_dir: &Path, id: &str) -> AppResult<PluginContent> {
    let dir = installed_dir(app_data_dir, id)?;
    let (core, manifest) = parse_manifest(&std::fs::read(dir.join(MANIFEST_FILE))?)?;
    if core.id != id {
        return Err(AppError::BadRequest("插件清单与安装目录不一致".into()));
    }
    let entry = std::fs::read_to_string(dir.join(&core.entry))?;
    let mut assets = BTreeMap::new();
    let mut texts = BTreeMap::new();
    for a in &core.assets {
        let bytes = std::fs::read(dir.join(a))?;
        check_asset(a, &bytes).map_err(AppError::BadRequest)?;
        if is_text_asset(a) {
            texts.insert(a.clone(), String::from_utf8_lossy(&bytes).into_owned());
        } else if let Some(url) = data_url(a, &bytes) {
            assets.insert(a.clone(), url);
        }
    }
    Ok(PluginContent { manifest, entry, assets, texts })
}

/// 读出已安装插件的清单核心字段与入口文本 (系统插件重建运行时用; 每次都重新解析清单)。
pub fn read_entry(app_data_dir: &Path, id: &str) -> AppResult<(ManifestCore, String)> {
    let dir = installed_dir(app_data_dir, id)?;
    let (core, _) = parse_manifest(&std::fs::read(dir.join(MANIFEST_FILE))?)?;
    if core.id != id {
        return Err(AppError::BadRequest("插件清单与安装目录不一致".into()));
    }
    let bytes = std::fs::read(dir.join(&core.entry))?;
    if bytes.len() as u64 > MAX_ENTRY_BYTES {
        return Err(AppError::BadRequest("入口文件过大".into()));
    }
    let entry = String::from_utf8(bytes).map_err(|_| AppError::BadRequest(format!("{} 不是 UTF-8 文本", core.entry)))?;
    Ok((core, entry))
}

pub fn uninstall(app_data_dir: &Path, id: &str) -> AppResult<()> {
    let dir = installed_dir(app_data_dir, id)?;
    std::fs::remove_dir_all(dir)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(id: &str, extra: &[(&str, &[u8])]) -> Files {
        let assets: Vec<String> = extra.iter().map(|(p, _)| format!("\"{p}\"")).collect();
        let manifest = format!(
            r#"{{"manifest":1,"id":"{id}","type":"theme","version":"1.0.0","name":"x","author":"a","entry":"theme.json","assets":[{}]}}"#,
            assets.join(",")
        );
        let mut f = Files::new();
        f.insert(MANIFEST_FILE.into(), manifest.into_bytes());
        f.insert("theme.json".into(), br#"{"themeApi":1}"#.to_vec());
        for (p, b) in extra {
            f.insert((*p).into(), b.to_vec());
        }
        f
    }

    fn local(dir: &Path) -> PluginSource {
        PluginSource { kind: SourceKind::Local, location: dir.display().to_string(), path: None, commit: None }
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrest";

    #[test]
    fn stage_install_reinstall_load_uninstall() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path();
        let s = stage(app, pkg("demo-a", &[("assets/p.png", PNG)]), local(app)).unwrap();
        assert_eq!(s.assets.len(), 1);
        assert!(s.installed.is_none());
        let rec = install(app, &s.staging_id, None).unwrap();
        assert_eq!(rec.id, "demo-a");
        assert_eq!(list(app).unwrap().len(), 1);

        let content = load(app, "demo-a").unwrap();
        assert!(content.assets["assets/p.png"].starts_with("data:image/png;base64,"));
        assert!(content.texts.is_empty());

        // 覆盖安装: 暂存时能看到已装的版本, 装完仍只有一份
        let s2 = stage(app, pkg("demo-a", &[]), local(app)).unwrap();
        assert!(s2.installed.is_some());
        install(app, &s2.staging_id, None).unwrap();
        assert_eq!(list(app).unwrap().len(), 1);
        assert!(load(app, "demo-a").unwrap().assets.is_empty());

        uninstall(app, "demo-a").unwrap();
        assert!(list(app).unwrap().is_empty());
        assert!(load(app, "demo-a").is_err());
    }

    fn provider_pkg(id: &str, base_url: &str) -> Files {
        let manifest = format!(
            r#"{{"manifest":1,"id":"{id}","type":"provider","version":"1.0.0","name":"x","author":"a","entry":"provider.yaml","assets":[]}}"#
        );
        let yaml = format!(
            "display_name: Relay\ncategory: aggregator\ncompatibility: untested\nauth: {{ type: api_key, header_name: Authorization, header_format: bearer }}\nendpoints:\n  - {{ id: main, label: Main, base_url: '{base_url}', messages_path: /v1/messages }}\nmodel_discovery: {{ enabled: false }}\n"
        );
        let mut f = Files::new();
        f.insert(MANIFEST_FILE.into(), manifest.into_bytes());
        f.insert("provider.yaml".into(), yaml.into_bytes());
        f
    }

    #[test]
    fn provider_plugins_are_checked_in_rust_and_reach_the_registry() {
        use crate::plugins::runtime::installed_provider_plugins;
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path();

        let s = stage(app, provider_pkg("relay", "https://api.example.com"), local(app)).unwrap();
        let sys = s.system.as_ref().expect("系统插件带 Rust 校验结果");
        assert!(sys.ok, "{:?}", sys.issues);
        assert!(sys.diff.is_none(), "首次安装没有差异");
        install(app, &s.staging_id, None).unwrap();

        let plugins = installed_provider_plugins(app, &Default::default());
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].0.id, "plugin:relay");
        assert!(plugins[0].1.enabled, "厂商插件默认生效");

        // 更新换了域名: 暂存时给出能力差异
        let s2 = stage(app, provider_pkg("relay", "https://other.example.com"), local(app)).unwrap();
        let diff = s2.system.unwrap().diff.expect("与已装版本比较");
        assert_eq!(diff.added_hosts, vec!["other.example.com"]);
        assert_eq!(diff.removed_hosts, vec!["api.example.com"]);
        discard(&s2.staging_id);

        // 内容有问题: 暂存时报出来, 安装被 Rust 拒绝 (不信前端)
        let bad = stage(app, provider_pkg("relay", "http://api.example.com"), local(app)).unwrap();
        assert!(!bad.system.as_ref().unwrap().ok);
        assert!(install(app, &bad.staging_id, None).is_err());
        assert_eq!(installed_provider_plugins(app, &Default::default())[0].0.endpoints[0].base_url, "https://api.example.com");
    }

    #[test]
    fn rules_plugin_stages_installs_and_reports_expanded_actions_on_update() {
        use crate::plugins::config::PluginConfig;
        use crate::plugins::runtime::installed_rules_plugins;
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path();
        let mk = |rules_json: &str| {
            let mut f = Files::new();
            f.insert(
                MANIFEST_FILE.into(),
                br#"{"manifest":1,"id":"side-tasks","type":"rules","version":"1.0.0","name":"x","author":"a","entry":"rules.json","assets":[]}"#.to_vec(),
            );
            f.insert("rules.json".into(), rules_json.as_bytes().to_vec());
            f
        };
        let v1 = r#"{"rulesApi":1,"params":[{"id":"vm","type":"virtual_model","label":"VM"}],"rules":[{"id":"r1","hook":"route","do":[{"use_virtual_model":"{{params.vm}}"}]}]}"#;
        let s = stage(app, mk(v1), local(app)).unwrap();
        assert!(s.system.as_ref().unwrap().ok);
        install(app, &s.staging_id, None).unwrap();

        // 默认停用: 不会进入规则注册表
        assert!(installed_rules_plugins(app, &Default::default()).is_empty());
        // 启用但缺必填参数: 跳过
        let mut cfgs = HashMap::new();
        cfgs.insert("side-tasks".into(), PluginConfig { enabled: true, priority: 0, params: "{}".into() });
        assert!(installed_rules_plugins(app, &cfgs).is_empty());
        // 填齐参数并启用: 加载成功
        cfgs.insert("side-tasks".into(), PluginConfig { enabled: true, priority: 0, params: r#"{"vm":"haiku"}"#.into() });
        let loaded = installed_rules_plugins(app, &cfgs);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].0.params["vm"], "haiku");

        // 更新新增 reject 动作: 能力差异报告 expands = true
        let v2 = r#"{"rulesApi":1,"params":[{"id":"vm","type":"virtual_model","label":"VM"}],"rules":[{"id":"r1","hook":"route","do":[{"use_virtual_model":"{{params.vm}}"}]},{"id":"r2","hook":"request","do":[{"reject":{"message":"no"}}]}]}"#;
        let s2 = stage(app, mk(v2), local(app)).unwrap();
        let diff = s2.system.unwrap().diff.expect("有能力差异");
        assert!(diff.expands());
        assert!(diff.added_actions.iter().any(|a| a == "reject"));
        discard(&s2.staging_id);
    }

    #[test]
    fn text_assets_come_back_as_text() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path();
        let mut f = pkg("demo-r", &[("t.html", "<div>{{receipt.no}}</div>".as_bytes()), ("s.css", b".a{color:red}")]);
        let m = String::from_utf8(f[MANIFEST_FILE].clone()).unwrap().replace(r#""type":"theme""#, r#""type":"receipt""#);
        f.insert(MANIFEST_FILE.into(), m.into_bytes());
        let s = stage(app, f, local(app)).unwrap();
        assert_eq!(s.texts["t.html"], "<div>{{receipt.no}}</div>");
        install(app, &s.staging_id, None).unwrap();
        let c = load(app, "demo-r").unwrap();
        assert_eq!(c.texts["s.css"], ".a{color:red}");
        assert!(c.assets.is_empty(), "文本资源不转 data URL");
    }

    #[test]
    fn community_hash_must_match() {
        let tmp = tempfile::tempdir().unwrap();
        let s = stage(tmp.path(), pkg("demo-b", &[]), local(tmp.path())).unwrap();
        assert!(install(tmp.path(), &s.staging_id, Some("00")).is_err());
        assert!(list(tmp.path()).unwrap().is_empty());
        // 已消费的暂存不能再用
        assert!(install(tmp.path(), &s.staging_id, None).is_err());

        let s = stage(tmp.path(), pkg("demo-b", &[]), local(tmp.path())).unwrap();
        let rec = install(tmp.path(), &s.staging_id, Some(&s.sha256.to_uppercase())).unwrap();
        assert_eq!(rec.source.kind, SourceKind::Community);
    }

    #[test]
    fn rejects_bad_packages() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path();
        // 清单没列出的文件
        let mut extra = pkg("demo-c", &[]);
        extra.insert("evil.png".into(), PNG.to_vec());
        assert!(stage(app, extra, local(app)).is_err());
        // 列出了但缺文件
        let mut missing = pkg("demo-c", &[("a.png", PNG)]);
        missing.remove("a.png");
        assert!(stage(app, missing, local(app)).is_err());
        // 文件头不符
        assert!(stage(app, pkg("demo-c", &[("a.png", b"GIF89a")]), local(app)).is_err());
        // 不合法 id 不能 load / uninstall
        assert!(load(app, "../x").is_err());
        assert!(uninstall(app, "..").is_err());
    }

    #[test]
    fn reads_local_folder_and_blocks_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        for (p, b) in pkg("demo-d", &[("assets/p.png", PNG)]) {
            std::fs::write(dir.join(p), b).unwrap();
        }
        let files = read_dir_package(&dir).unwrap();
        assert_eq!(files.len(), 3);

        #[cfg(unix)]
        {
            std::fs::write(tmp.path().join("secret.png"), PNG).unwrap();
            std::fs::remove_file(dir.join("assets/p.png")).unwrap();
            std::os::unix::fs::symlink(tmp.path().join("secret.png"), dir.join("assets/p.png")).unwrap();
            assert!(read_dir_package(&dir).is_err(), "符号链接指向文件夹外必须拒绝");
        }
    }
}
