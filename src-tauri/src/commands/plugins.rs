//! 插件 command (主题 / 小票 / 系统插件)。
//!
//! `list_plugins` / `load_plugin` / `disable_plugin` 网页界面也能用 (显示已安装的主题; 停用是安全方向);
//! 其余会下载、改宿主机文件或启用会改变 app 行为的插件, 在 `proxy/web/api.rs::web_commands!` 里是
//! `DESKTOP_ONLY` 拒绝桩, 由 `host_mutating_plugin_commands_are_refusal_stubs_here` 扫源码锁住。

use std::collections::HashMap;
use std::path::Path;

use cc_router_plugin_spec::check::{Capabilities, CapabilityDiff};
use cc_router_plugin_spec::notifier_plugin::{
    check_notifier_param_value, resolve_notifier_params, NotifierParamDecl, NotifierParamKind, NotifierPlugin,
};
use cc_router_plugin_spec::plugin::{check_system, is_system_type, SystemPlugin};
use cc_router_plugin_spec::rules_engine::{dry_run_plugin, Facts, LoadedRules, PartialBodyFacts};
use cc_router_plugin_spec::rules_plugin::{check_param_value, resolve_params, ParamDecl, RulesPlugin};
use chrono::{Datelike, Local, TimeZone, Timelike, Utc};
use serde::Serialize;
use serde_json::{Map, Value};
use sqlx::Row;
use tauri::{Emitter, State};

use crate::db::paths;
use crate::error::{AppError, AppResult};
use crate::notify::{DeliveryRecord, NotifyEvent};
use crate::plugins::notifier::LoadedNotifier;
use crate::plugins::store::{self, InstalledPlugin, PluginContent, StagedPlugin};
use crate::plugins::{config, runtime};
use crate::plugins::{github, link, PluginSource, SourceKind, COMMUNITY_INDEX_URL, MANIFEST_FILE, MAX_ENTRY_BYTES, MAX_FILE_BYTES, MAX_INDEX_BYTES, MAX_MANIFEST_BYTES, MAX_TOTAL_BYTES};
use crate::state::AppState;
use crate::tray::TrayLocale;

/// 已安装插件变了 (装 / 卸 / 更新): 各端刷新列表; 正在用的主题被卸载时前端回退到默认主题。
pub const CHANGED_EVENT: &str = "plugins_changed";

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T> {
    tokio::task::spawn_blocking(f).await.map_err(|e| AppError::internal(e.to_string()))?
}

/// 已安装列表的一项: 安装记录 + 生效状态 (系统插件看 `plugin_configs`, 主题 / 小票恒为 true)。
/// 规则 / 通知插件额外带出参数声明表、用户已保存参数与缺少的必填参数 id (方便界面生成设置对话框与启用前提示)。
/// 通知插件的 `secret` 类型参数**永不**在 `params` 里回传明文, 改为只把已填值的 id 放进 `secret_params_set`。
#[derive(Debug, Serialize)]
pub struct InstalledPluginView {
    #[serde(flatten)]
    pub record: InstalledPlugin,
    pub enabled: bool,
    pub priority: i64,
    pub params: Map<String, Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub param_decls: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing_params: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub secret_params_set: Vec<String>,
}

fn read_rules_plugin(app_data_dir: &Path, id: &str) -> AppResult<RulesPlugin> {
    let (core, entry) = store::read_entry(app_data_dir, id)?;
    match check_system(&core.plugin_type, &core.id, &entry, &core.assets) {
        (_, Some(SystemPlugin::Rules(p))) => Ok(p),
        (res, _) => Err(AppError::BadRequest(format!("规则插件内容有 {} 处问题, 无法读取", res.issues.len().max(1)))),
    }
}

fn read_notifier_plugin(app_data_dir: &Path, id: &str) -> AppResult<NotifierPlugin> {
    let (core, entry) = store::read_entry(app_data_dir, id)?;
    match check_system(&core.plugin_type, &core.id, &entry, &core.assets) {
        (_, Some(SystemPlugin::Notifier(p))) => Ok(p),
        (res, _) => Err(AppError::BadRequest(format!("通知插件内容有 {} 处问题, 无法读取", res.issues.len().max(1)))),
    }
}

fn read_system_caps(app_data_dir: &Path, id: &str) -> Option<Capabilities> {
    let (core, entry) = store::read_entry(app_data_dir, id).ok()?;
    if !is_system_type(&core.plugin_type) {
        return None;
    }
    check_system(&core.plugin_type, &core.id, &entry, &core.assets).0.capabilities
}

/// 校验用户要保存的规则插件参数表: 拒绝未知参数 id 与类型不符的值; 值为 `null` 表示清空 (回退默认值)。
/// 若插件正在启用中, 清空或少填必填参数也会被拒绝。
pub(crate) fn validate_params_update(
    decls: &[ParamDecl],
    incoming: &Map<String, Value>,
    currently_enabled: bool,
) -> AppResult<Map<String, Value>> {
    let mut cleaned = Map::new();
    for (k, v) in incoming {
        let Some(decl) = decls.iter().find(|d| d.id == *k) else {
            return Err(AppError::BadRequest(format!("未知参数: {k}")));
        };
        if v.is_null() {
            continue;
        }
        if let Err(expect) = check_param_value(&decl.kind, v) {
            return Err(AppError::BadRequest(format!("参数 {k} 格式不对: 应为 {expect}")));
        }
        cleaned.insert(k.clone(), v.clone());
    }
    if currently_enabled {
        validate_rules_enable(decls, &cleaned)?;
    }
    Ok(cleaned)
}

/// 规则插件启用前检查: 所有必填参数必须填好且合法 (有默认值的自动补齐)。
pub(crate) fn validate_rules_enable(decls: &[ParamDecl], user: &Map<String, Value>) -> AppResult<Map<String, Value>> {
    resolve_params(decls, user).map_err(|issues| {
        let names: Vec<&str> = issues.iter().map(|i| i.path.as_str()).collect();
        AppError::BadRequest(format!("请先配置必填参数后再启用: {}", names.join(", ")))
    })
}

/// 通知插件启用 / 测试前检查: 所有必填参数 (含 secret 与 base_url) 必须填齐且合法。
pub(crate) fn validate_notifier_enable(
    decls: &[NotifierParamDecl],
    user: &Map<String, Value>,
) -> AppResult<Map<String, Value>> {
    resolve_notifier_params(decls, user).map_err(|issues| {
        let names: Vec<&str> = issues.iter().map(|i| i.path.as_str()).collect();
        AppError::BadRequest(format!("请先配置必填参数后再启用: {}", names.join(", ")))
    })
}

/// 校验通知插件参数更新:
/// - `secret` 参数在 `incoming` 里省略或为空串 `""` 时**保留**库里已有值 (界面看不到原密文, 修改其它字段不能误清空);
/// - 显式传 `null` 才清空;
/// - 传非空值则校验并覆盖。
pub(crate) fn validate_notifier_params_update(
    decls: &[NotifierParamDecl],
    existing: &Map<String, Value>,
    incoming: &Map<String, Value>,
    currently_enabled: bool,
) -> AppResult<Map<String, Value>> {
    for k in incoming.keys() {
        if !decls.iter().any(|d| d.id == *k) {
            return Err(AppError::BadRequest(format!("未知参数: {k}")));
        }
    }
    let mut cleaned = Map::new();
    for decl in decls {
        match decl.kind {
            NotifierParamKind::Secret => match incoming.get(&decl.id) {
                None => {
                    if let Some(prev) = existing.get(&decl.id) {
                        if prev.as_str().is_some_and(|s| !s.is_empty()) {
                            cleaned.insert(decl.id.clone(), prev.clone());
                        }
                    }
                }
                Some(Value::Null) => {}
                Some(Value::String(s)) if s.is_empty() => {
                    if let Some(prev) = existing.get(&decl.id) {
                        if prev.as_str().is_some_and(|p| !p.is_empty()) {
                            cleaned.insert(decl.id.clone(), prev.clone());
                        }
                    }
                }
                Some(v) => {
                    if let Err(expect) = check_notifier_param_value(&decl.kind, v) {
                        return Err(AppError::BadRequest(format!("参数 {} 格式不对: 应为 {expect}", decl.id)));
                    }
                    cleaned.insert(decl.id.clone(), v.clone());
                }
            },
            _ => {
                if let Some(v) = incoming.get(&decl.id) {
                    if v.is_null() {
                        continue;
                    }
                    if let Err(expect) = check_notifier_param_value(&decl.kind, v) {
                        return Err(AppError::BadRequest(format!("参数 {} 格式不对: 应为 {expect}", decl.id)));
                    }
                    cleaned.insert(decl.id.clone(), v.clone());
                }
            }
        }
    }
    if currently_enabled {
        validate_notifier_enable(decls, &cleaned)?;
    }
    Ok(cleaned)
}

/// 剥离通知插件的 `secret` 明文, 返回 `(可安全回传给前端的 params, 已填值的 secret 参数 id 列表)`。
pub(crate) fn redact_notifier_params(
    decls: &[NotifierParamDecl],
    raw: &Map<String, Value>,
) -> (Map<String, Value>, Vec<String>) {
    let mut safe = Map::new();
    let mut secrets_set = Vec::new();
    for (k, v) in raw {
        let is_secret = decls
            .iter()
            .find(|d| d.id == *k)
            .is_some_and(|d| matches!(d.kind, NotifierParamKind::Secret));
        if is_secret {
            if v.as_str().is_some_and(|s| !s.is_empty()) {
                secrets_set.push(k.clone());
            }
        } else {
            safe.insert(k.clone(), v.clone());
        }
    }
    secrets_set.sort();
    (safe, secrets_set)
}

enum LoadedDecls {
    Rules(Vec<ParamDecl>),
    Notifier(Vec<NotifierParamDecl>),
}

#[tauri::command]
pub async fn list_plugins(state: State<'_, AppState>) -> AppResult<Vec<InstalledPluginView>> {
    let dir = paths::app_data_dir(&state.app_handle)?;
    let configs = config::load_all(&state.db).await?;
    let (list, decls_by_id) = blocking(move || {
        let list = store::list(&dir)?;
        let mut decls: HashMap<String, LoadedDecls> = HashMap::new();
        for rec in &list {
            if rec.plugin_type == "rules" {
                if let Ok(p) = read_rules_plugin(&dir, &rec.id) {
                    decls.insert(rec.id.clone(), LoadedDecls::Rules(p.params));
                }
            } else if rec.plugin_type == "notifier" {
                if let Ok(p) = read_notifier_plugin(&dir, &rec.id) {
                    decls.insert(rec.id.clone(), LoadedDecls::Notifier(p.params));
                }
            }
        }
        Ok((list, decls))
    })
    .await?;
    let mut views: Vec<InstalledPluginView> = list
        .into_iter()
        .map(|record| {
            let enabled = !is_system_type(&record.plugin_type) || config::is_enabled(&configs, &record.id, &record.plugin_type);
            let priority = configs.get(&record.id).map(|c| c.priority).unwrap_or(0);
            let raw_params = configs.get(&record.id).map(|c| config::parse_params(&c.params)).unwrap_or_default();
            match decls_by_id.get(&record.id) {
                Some(LoadedDecls::Rules(decls)) => {
                    let missing_params = match resolve_params(decls, &raw_params) {
                        Ok(_) => Vec::new(),
                        Err(issues) => issues.into_iter().map(|i| i.path).collect(),
                    };
                    let param_decls = decls.iter().filter_map(|d| serde_json::to_value(d).ok()).collect();
                    InstalledPluginView {
                        record,
                        enabled,
                        priority,
                        params: raw_params,
                        param_decls,
                        missing_params,
                        secret_params_set: Vec::new(),
                    }
                }
                Some(LoadedDecls::Notifier(decls)) => {
                    let missing_params = match resolve_notifier_params(decls, &raw_params) {
                        Ok(_) => Vec::new(),
                        Err(issues) => issues.into_iter().map(|i| i.path).collect(),
                    };
                    let (params, secret_params_set) = redact_notifier_params(decls, &raw_params);
                    let param_decls = decls.iter().filter_map(|d| serde_json::to_value(d).ok()).collect();
                    InstalledPluginView {
                        record,
                        enabled,
                        priority,
                        params,
                        param_decls,
                        missing_params,
                        secret_params_set,
                    }
                }
                None => {
                    // 通知插件文件损坏时绝不把原始 params (可能含 secret) 回传给前端
                    let params = if record.plugin_type == "notifier" { Map::new() } else { raw_params };
                    InstalledPluginView {
                        record,
                        enabled,
                        priority,
                        params,
                        param_decls: Vec::new(),
                        missing_params: Vec::new(),
                        secret_params_set: Vec::new(),
                    }
                }
            }
        })
        .collect();
    // 优先级高的排在前面 (与 PricingRegistry / RulesRegistry 的匹配顺序一致)
    views.sort_by(|a, b| b.priority.cmp(&a.priority).then_with(|| a.record.id.cmp(&b.record.id)));
    Ok(views)
}

/// 系统插件装 / 卸 / 启停 / 改参数之后: 重建运行时 (厂商 + 计价 + 规则 + 通知注册表) 并通知各端。
async fn after_change(state: &AppState) {
    if let Err(e) = runtime::rebuild_runtime(state).await {
        tracing::warn!(error = %e, "插件变更后重建系统插件运行时失败");
    }
    let _ = state.app_handle.emit(CHANGED_EVENT, ());
}

async fn installed_system_plugin(state: &AppState, id: &str) -> AppResult<InstalledPlugin> {
    let dir = paths::app_data_dir(&state.app_handle)?;
    let id_owned = id.to_string();
    let rec = blocking(move || store::list(&dir))
        .await?
        .into_iter()
        .find(|p| p.id == id_owned)
        .ok_or_else(|| AppError::BadRequest(format!("没有安装插件 {id}")))?;
    if !is_system_type(&rec.plugin_type) {
        return Err(AppError::BadRequest("这类插件不需要启用, 在主题 / 小票选择里直接使用".into()));
    }
    Ok(rec)
}

/// 启用系统插件。会改变 app 行为, 只能在桌面窗口里调用 (网页界面是拒绝桩)。
/// 规则 / 通知插件启用前必须填齐全部必填参数。
#[tauri::command]
pub async fn enable_plugin(state: State<'_, AppState>, id: String) -> AppResult<()> {
    let rec = installed_system_plugin(&state, &id).await?;
    if rec.plugin_type == "rules" {
        let dir = paths::app_data_dir(&state.app_handle)?;
        let id_owned = id.clone();
        let plugin = blocking(move || read_rules_plugin(&dir, &id_owned)).await?;
        let configs = config::load_all(&state.db).await?;
        let user = configs.get(&id).map(|c| config::parse_params(&c.params)).unwrap_or_default();
        validate_rules_enable(&plugin.params, &user)?;
    } else if rec.plugin_type == "notifier" {
        let dir = paths::app_data_dir(&state.app_handle)?;
        let id_owned = id.clone();
        let plugin = blocking(move || read_notifier_plugin(&dir, &id_owned)).await?;
        let configs = config::load_all(&state.db).await?;
        let user = configs.get(&id).map(|c| config::parse_params(&c.params)).unwrap_or_default();
        validate_notifier_enable(&plugin.params, &user)?;
    }
    config::set_enabled(&state.db, &id, true).await?;
    after_change(&state).await;
    Ok(())
}

/// 停用系统插件 (安全方向, 网页界面也能用)。
#[tauri::command]
pub async fn disable_plugin(state: State<'_, AppState>, id: String) -> AppResult<()> {
    installed_system_plugin(&state, &id).await?;
    config::set_enabled(&state.db, &id, false).await?;
    after_change(&state).await;
    Ok(())
}

/// 保存系统插件的用户参数 (网页界面也开放: 改参数本身不启用插件)。
#[tauri::command]
pub async fn set_plugin_params(
    state: State<'_, AppState>,
    id: String,
    params: Map<String, Value>,
) -> AppResult<()> {
    let rec = installed_system_plugin(&state, &id).await?;
    let dir = paths::app_data_dir(&state.app_handle)?;
    let configs = config::load_all(&state.db).await?;
    let enabled = config::is_enabled(&configs, &id, &rec.plugin_type);
    let cleaned = match rec.plugin_type.as_str() {
        "rules" => {
            let id_owned = id.clone();
            let plugin = blocking(move || read_rules_plugin(&dir, &id_owned)).await?;
            validate_params_update(&plugin.params, &params, enabled)?
        }
        "notifier" => {
            let id_owned = id.clone();
            let plugin = blocking(move || read_notifier_plugin(&dir, &id_owned)).await?;
            let existing = configs.get(&id).map(|c| config::parse_params(&c.params)).unwrap_or_default();
            validate_notifier_params_update(&plugin.params, &existing, &params, enabled)?
        }
        _ => return Err(AppError::BadRequest("这类插件没有可设置的参数".into())),
    };
    config::set_params(&state.db, &id, &rec.plugin_type, &cleaned).await?;
    after_change(&state).await;
    Ok(())
}

/// 向一个已安装的通知插件发一次测试消息 (即使尚未启用也可以先测), 记录并返回投递结果。
/// 会发起出站 HTTP 请求, 只能在桌面窗口调用 (网页界面是拒绝桩)。
#[tauri::command]
pub async fn test_notifier_plugin(state: State<'_, AppState>, id: String) -> AppResult<DeliveryRecord> {
    let rec = installed_system_plugin(&state, &id).await?;
    if rec.plugin_type != "notifier" {
        return Err(AppError::BadRequest("只有通知插件支持测试推送".into()));
    }
    let dir = paths::app_data_dir(&state.app_handle)?;
    let id_owned = id.clone();
    let plugin = blocking(move || read_notifier_plugin(&dir, &id_owned)).await?;
    let configs = config::load_all(&state.db).await?;
    let user = configs.get(&id).map(|c| config::parse_params(&c.params)).unwrap_or_default();
    let resolved = validate_notifier_enable(&plugin.params, &user)?;
    let loaded = LoadedNotifier {
        plugin_id: id.clone(),
        version: rec.version,
        plugin,
        params: resolved,
    };
    let locale = TrayLocale::from_pref(&state.settings.read().await.preferred_language);
    let ctx = NotifyEvent::Test.template_context(locale);
    let delivery = crate::notify::send_to_plugin(&state.http_client, &loaded, "test", &ctx).await;
    state.notify.record_delivery(&id, delivery.clone());
    Ok(delivery)
}

/// 查一个通知插件最近的发送记录 (内存最近 20 条, 不含请求体)。网页界面可读。
#[tauri::command]
pub async fn list_notifier_deliveries(state: State<'_, AppState>, id: String) -> AppResult<Vec<DeliveryRecord>> {
    Ok(state.notify.deliveries(&id))
}

/// 发一条内置系统桌面测试通知 (验证系统通知权限)。只能在桌面窗口调用。
#[tauri::command]
pub async fn test_desktop_notification(state: State<'_, AppState>) -> AppResult<()> {
    let locale = TrayLocale::from_pref(&state.settings.read().await.preferred_language);
    let (title, body) = NotifyEvent::Test.render_default(locale);
    crate::notify::send_desktop(&state.app_handle, &title, &body, Some("/settings")).map_err(AppError::Internal)
}

/// 调整同类系统插件 (`pricing` / `rules`) 的优先级顺序: `ordered_ids[0]` 优先级最高。
/// 会改变计价覆盖或规则执行顺序, 仅桌面端开放 (网页端是拒绝桩)。
#[tauri::command]
pub async fn reorder_plugins(
    state: State<'_, AppState>,
    plugin_type: String,
    ordered_ids: Vec<String>,
) -> AppResult<()> {
    if !matches!(plugin_type.as_str(), "pricing" | "rules") {
        return Err(AppError::BadRequest("只有计价与规则插件支持调整优先级".into()));
    }
    let dir = paths::app_data_dir(&state.app_handle)?;
    let installed = blocking(move || store::list(&dir)).await?;
    let valid_ids: std::collections::HashSet<String> = installed
        .into_iter()
        .filter(|p| p.plugin_type == plugin_type)
        .map(|p| p.id)
        .collect();
    let mut seen = std::collections::HashSet::new();
    for id in &ordered_ids {
        if !valid_ids.contains(id) {
            return Err(AppError::BadRequest(format!("插件 {id} 不存在或类型不匹配")));
        }
        if !seen.insert(id.clone()) {
            return Err(AppError::BadRequest(format!("重复的插件 id: {id}")));
        }
    }
    let total = ordered_ids.len() as i64;
    let items: Vec<(String, String, i64)> = ordered_ids
        .into_iter()
        .enumerate()
        .map(|(idx, id)| (id, plugin_type.clone(), (total - idx as i64) * 10))
        .collect();
    config::set_priorities(&state.db, &items).await?;
    after_change(&state).await;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct RuleDryRunCount {
    pub rule_id: String,
    pub matched: usize,
    pub unknown: usize,
}

#[derive(Debug, Serialize)]
pub struct DryRunSample {
    pub request_id: String,
    pub timestamp_ms: i64,
    pub virtual_model: String,
    pub purpose: String,
    pub client_tool: String,
    /// `"matched"` 或 `"unknown"` (至少一条规则命中或因缺少 body 条件无法判断)
    pub status: &'static str,
    pub matched_rules: Vec<String>,
    pub unknown_rules: Vec<String>,
    pub unknown_fields: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect_to: Option<String>,
    pub rejected: bool,
}

#[derive(Debug, Serialize)]
pub struct RulesDryRunResult {
    pub total_checked: usize,
    pub matched_count: usize,
    pub unknown_count: usize,
    pub rule_counts: Vec<RuleDryRunCount>,
    pub samples: Vec<DryRunSample>,
}

/// 规则插件「试运行」(设计 §6.7): 抽最近 N 条请求日志, 按日志已有列模拟匹配并标注依赖 body 的「无法判断」条件。
/// 纯只读, 网页界面与未启用插件均可调用; `params` 传入时优先用表单草稿, 省略则用已保存参数。
#[tauri::command]
pub async fn dry_run_rules_plugin(
    state: State<'_, AppState>,
    id: String,
    params: Option<Map<String, Value>>,
    limit: Option<u32>,
) -> AppResult<RulesDryRunResult> {
    let rec = installed_system_plugin(&state, &id).await?;
    if rec.plugin_type != "rules" {
        return Err(AppError::BadRequest("只有规则插件支持试运行".into()));
    }
    let dir = paths::app_data_dir(&state.app_handle)?;
    let id_owned = id.clone();
    let plugin = blocking(move || read_rules_plugin(&dir, &id_owned)).await?;
    let user_params = match params {
        Some(p) => validate_params_update(&plugin.params, &p, false)?,
        None => {
            let configs = config::load_all(&state.db).await?;
            configs.get(&id).map(|c| config::parse_params(&c.params)).unwrap_or_default()
        }
    };
    let resolved = validate_rules_enable(&plugin.params, &user_params)?;
    let loaded = LoadedRules {
        plugin_id: id,
        version: rec.version,
        plugin,
        params: resolved,
    };

    let lim = limit.unwrap_or(50).clamp(1, 200) as i64;
    let rows = sqlx::query(
        "SELECT id, timestamp, virtual_model_name, real_model_name, is_streaming,
                client_tool, entry_kind, tools_offered_count, purpose_kind, purpose_guessed,
                purpose_prev_tools, tools_offered_types
         FROM requests
         ORDER BY timestamp DESC
         LIMIT ?",
    )
    .bind(lim)
    .fetch_all(&state.db)
    .await?;

    let mut rule_stats: HashMap<String, (usize, usize)> = loaded
        .plugin
        .rules
        .iter()
        .map(|r| (r.id.clone(), (0, 0)))
        .collect();
    let mut matched_count = 0usize;
    let mut unknown_count = 0usize;
    let mut samples = Vec::new();

    for r in &rows {
        let req_id: String = r.try_get("id").unwrap_or_default();
        let ts_ms: i64 = r.try_get("timestamp").unwrap_or(0);
        let vm_raw: String = r.try_get("virtual_model_name").unwrap_or_default();
        let real_model: String = r.try_get("real_model_name").unwrap_or_default();
        let is_streaming = r.try_get::<i64, _>("is_streaming").unwrap_or(0) != 0;
        let client_tool = r
            .try_get::<Option<String>, _>("client_tool")
            .unwrap_or(None)
            .unwrap_or_else(|| "unknown".into());
        let entry_kind = r
            .try_get::<Option<String>, _>("entry_kind")
            .unwrap_or(None)
            .unwrap_or_else(|| "messages".into());
        let tools_offered: Option<i64> = r.try_get("tools_offered_count").unwrap_or(None);
        let purpose_kind: Option<String> = r.try_get("purpose_kind").unwrap_or(None);
        // purpose_kind 有值 = 这一行记录了用途 (migration 030 起); 此时 prev_tools 为 NULL 即「没有上一步工具」
        let prev_tools: Option<Vec<String>> = purpose_kind.as_ref().map(|_| {
            r.try_get::<Option<String>, _>("purpose_prev_tools")
                .unwrap_or(None)
                .and_then(|j| serde_json::from_str(&j).ok())
                .unwrap_or_default()
        });
        // tools_offered_types 有值才知道; 请求没带 tools (count = 0 / NULL 且用途已记录) 也算知道: 空集合
        let tool_types: Option<Vec<String>> = match r.try_get::<Option<String>, _>("tools_offered_types").unwrap_or(None) {
            Some(j) => serde_json::from_str(&j).ok(),
            None if tools_offered.is_none() && purpose_kind.is_some() => Some(Vec::new()),
            None => None,
        };
        let purpose = purpose_kind.unwrap_or_else(|| "unknown".into());
        let purpose_guessed = r.try_get::<Option<i64>, _>("purpose_guessed").unwrap_or(None).unwrap_or(0) != 0;

        let vm_short = vm_raw.strip_prefix("model-").unwrap_or(&vm_raw);
        let model_req = if vm_short == "fallback" { real_model.as_str() } else { vm_raw.as_str() };
        let local_dt = Utc
            .timestamp_millis_opt(ts_ms)
            .single()
            .unwrap_or_else(Utc::now)
            .with_timezone(&Local);
        let facts = Facts {
            purpose: &purpose,
            purpose_guessed,
            virtual_model: vm_short,
            client_tool: &client_tool,
            entry: &entry_kind,
            model: model_req,
            weekday: local_dt.weekday().number_from_monday() as u8,
            minute: (local_dt.hour() * 60 + local_dt.minute()) as u16,
        };
        let partial = PartialBodyFacts {
            stream: Some(is_streaming),
            has_tools: tools_offered.map(|c| c > 0),
            has_images: None,
            thinking: None,
            approx_input_tokens: None,
            tool_types,
            // 工具名与 tool_choice 不落库
            tool_names: None,
            tool_forced: None,
            prev_tools,
        };

        let eval = dry_run_plugin(&loaded, &facts, &partial);
        for rid in &eval.matched_rules {
            if let Some(s) = rule_stats.get_mut(rid) {
                s.0 += 1;
            }
        }
        for rid in &eval.unknown_rules {
            if let Some(s) = rule_stats.get_mut(rid) {
                s.1 += 1;
            }
        }

        let status = if !eval.matched_rules.is_empty() {
            matched_count += 1;
            Some("matched")
        } else if !eval.unknown_rules.is_empty() {
            unknown_count += 1;
            Some("unknown")
        } else {
            None
        };
        if let Some(status) = status {
            if samples.len() < 15 {
                samples.push(DryRunSample {
                    request_id: req_id,
                    timestamp_ms: ts_ms,
                    virtual_model: vm_raw,
                    purpose,
                    client_tool,
                    status,
                    matched_rules: eval.matched_rules,
                    unknown_rules: eval.unknown_rules,
                    unknown_fields: eval.unknown_fields,
                    redirect_to: eval.redirect_to,
                    rejected: eval.rejected,
                });
            }
        }
    }

    let rule_counts = loaded
        .plugin
        .rules
        .iter()
        .map(|r| {
            let (m, u) = rule_stats.get(&r.id).copied().unwrap_or((0, 0));
            RuleDryRunCount { rule_id: r.id.clone(), matched: m, unknown: u }
        })
        .collect();

    Ok(RulesDryRunResult {
        total_checked: rows.len(),
        matched_count,
        unknown_count,
        rule_counts,
        samples,
    })
}

#[tauri::command]
pub async fn load_plugin(state: State<'_, AppState>, id: String) -> AppResult<PluginContent> {
    let dir = paths::app_data_dir(&state.app_handle)?;
    blocking(move || store::load(&dir, &id)).await
}

/// 从 GitHub 链接下载到暂存区。不安装: 前端校验内容、用户确认后再调 `install_staged_plugin`。
#[tauri::command]
pub async fn stage_plugin_from_github(state: State<'_, AppState>, link: String) -> AppResult<StagedPlugin> {
    let r = link::parse(&link).map_err(AppError::BadRequest)?;
    let client = &state.http_client;
    let sha = github::resolve_commit(client, &r).await?;

    let manifest = github::fetch_file(client, &r, &sha, MANIFEST_FILE, MAX_MANIFEST_BYTES)
        .await?
        .ok_or_else(|| AppError::BadRequest(format!("这个位置没有 {MANIFEST_FILE}, 不是 cc-router 插件")))?;
    let (core, _) = crate::plugins::parse_manifest(&manifest)?;
    let mut files = store::Files::new();
    files.insert(MANIFEST_FILE.into(), manifest);
    let need = |path: &str, bytes: Option<Vec<u8>>| bytes.ok_or_else(|| AppError::BadRequest(format!("仓库里缺少清单列出的文件 {path}")));
    let entry = github::fetch_file(client, &r, &sha, &core.entry, MAX_ENTRY_BYTES).await?;
    files.insert(core.entry.clone(), need(&core.entry, entry)?);
    let mut total: u64 = files.values().map(|b| b.len() as u64).sum();
    for a in &core.assets {
        let bytes = need(a, github::fetch_file(client, &r, &sha, a, MAX_FILE_BYTES).await?)?;
        total += bytes.len() as u64;
        if total > MAX_TOTAL_BYTES {
            return Err(AppError::BadRequest(format!("插件总大小超过 {} MB", MAX_TOTAL_BYTES / 1024 / 1024)));
        }
        files.insert(a.clone(), bytes);
    }

    let source = PluginSource { kind: SourceKind::Github, location: format!("{}/{}", r.owner, r.repo), path: r.path.clone(), commit: Some(sha) };
    let dir = paths::app_data_dir(&state.app_handle)?;
    blocking(move || store::stage(&dir, files, source)).await
}

/// 从本地文件夹读到暂存区 (作者调试用)。
#[tauri::command]
pub async fn stage_plugin_from_dir(state: State<'_, AppState>, path: String) -> AppResult<StagedPlugin> {
    let dir = paths::app_data_dir(&state.app_handle)?;
    blocking(move || {
        let folder = Path::new(&path);
        if !folder.is_absolute() {
            return Err(AppError::BadRequest("请选择一个文件夹".into()));
        }
        let files = store::read_dir_package(folder)?;
        let source = PluginSource { kind: SourceKind::Local, location: folder.display().to_string(), path: None, commit: None };
        store::stage(&dir, files, source)
    })
    .await
}

/// `community_sha256`: 从社区目录安装时传目录里记录的包哈希, 不一致即拒绝。
#[tauri::command]
pub async fn install_staged_plugin(
    state: State<'_, AppState>,
    staging_id: String,
    community_sha256: Option<String>,
) -> AppResult<InstalledPlugin> {
    let dir = paths::app_data_dir(&state.app_handle)?;
    let (rec, disable_check) = blocking(move || {
        let before: HashMap<String, Capabilities> = store::list(&dir)?
            .into_iter()
            .filter(|p| matches!(p.plugin_type.as_str(), "rules" | "notifier"))
            .filter_map(|p| read_system_caps(&dir, &p.id).map(|c| (p.id, c)))
            .collect();
        let rec = store::install(&dir, &staging_id, community_sha256.as_deref())?;
        // 启用中的规则 / 通知插件更新后若能力扩大 (新增动作 / 钩子 / 域名 / 事件 / 字段),
        // 自动停用要求用户重新启用 (设计 §3.3 / §7.2)
        let expanded = if matches!(rec.plugin_type.as_str(), "rules" | "notifier") {
            match (before.get(&rec.id), read_system_caps(&dir, &rec.id)) {
                (Some(old), Some(new)) => CapabilityDiff::between(old, &new).expands(),
                _ => false,
            }
        } else {
            false
        };
        let new_decls = match rec.plugin_type.as_str() {
            "rules" => read_rules_plugin(&dir, &rec.id).ok().map(|p| LoadedDecls::Rules(p.params)),
            "notifier" => read_notifier_plugin(&dir, &rec.id).ok().map(|p| LoadedDecls::Notifier(p.params)),
            _ => None,
        };
        Ok((rec, (expanded, new_decls)))
    })
    .await?;
    if matches!(rec.plugin_type.as_str(), "rules" | "notifier") {
        let configs = config::load_all(&state.db).await?;
        if config::is_enabled(&configs, &rec.id, &rec.plugin_type) {
            let user = configs.get(&rec.id).map(|c| config::parse_params(&c.params)).unwrap_or_default();
            let params_broken = match disable_check.1.as_ref() {
                Some(LoadedDecls::Rules(decls)) => resolve_params(decls, &user).is_err(),
                Some(LoadedDecls::Notifier(decls)) => resolve_notifier_params(decls, &user).is_err(),
                None => false,
            };
            if disable_check.0 || params_broken {
                config::set_enabled(&state.db, &rec.id, false).await?;
            }
        }
    }
    after_change(&state).await;
    Ok(rec)
}

#[tauri::command]
pub async fn discard_staged_plugin(staging_id: String) -> AppResult<()> {
    blocking(move || {
        store::discard(&staging_id);
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn uninstall_plugin(state: State<'_, AppState>, id: String) -> AppResult<()> {
    let dir = paths::app_data_dir(&state.app_handle)?;
    let id_owned = id.clone();
    blocking(move || store::uninstall(&dir, &id_owned)).await?;
    config::delete(&state.db, &id).await?;
    after_change(&state).await;
    Ok(())
}

/// 社区目录 `index.json` 原样返回 (前端宽松解析: 不认识的条目跳过)。
#[tauri::command]
pub async fn fetch_community_index(state: State<'_, AppState>) -> AppResult<Value> {
    let bytes = github::fetch_limited(&state.http_client, COMMUNITY_INDEX_URL, MAX_INDEX_BYTES)
        .await?
        .ok_or_else(|| AppError::BadGateway("社区目录暂时不可用".into()))?;
    serde_json::from_slice(&bytes).map_err(|_| AppError::BadGateway("社区目录格式不对".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cc_router_plugin_spec::rules_plugin::parse;
    use serde_json::json;

    fn decls() -> Vec<ParamDecl> {
        parse(
            r#"{
                "rulesApi": 1,
                "params": [
                    { "id": "target", "type": "virtual_model", "label": "Target" },
                    { "id": "flag", "type": "bool", "label": "Flag", "default": false },
                    { "id": "limit", "type": "number", "label": "Limit", "default": 100 }
                ],
                "rules": [{ "id": "r", "hook": "route", "do": [{ "use_virtual_model": "{{params.target}}" }] }]
            }"#,
        )
        .unwrap()
        .0
        .params
    }

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn enable_requires_all_mandatory_params_and_fills_defaults() {
        let d = decls();
        assert!(validate_rules_enable(&d, &Map::new()).is_err(), "缺必填参数 target 必须拒绝启用");
        let resolved = validate_rules_enable(&d, &obj(json!({ "target": "haiku" }))).unwrap();
        assert_eq!(resolved["target"], "haiku");
        assert_eq!(resolved["flag"], false, "可选参数补默认值");
        assert_eq!(resolved["limit"], 100);
        assert!(validate_rules_enable(&d, &obj(json!({ "target": "jev" }))).is_err(), "非法虚拟模型被拒");
    }

    #[test]
    fn params_update_validates_types_and_guards_enabled_plugins() {
        let d = decls();
        // 未知参数拒绝
        assert!(validate_params_update(&d, &obj(json!({ "unknown": 1 })), false).is_err());
        // 类型不符拒绝
        assert!(validate_params_update(&d, &obj(json!({ "limit": "oops" })), false).is_err());
        // 停用状态下可以暂存部分参数 (含清空 null)
        let saved = validate_params_update(&d, &obj(json!({ "flag": true, "target": null })), false).unwrap();
        assert_eq!(saved.get("flag"), Some(&json!(true)));
        assert!(!saved.contains_key("target"));
        // 启用状态下不能清空必填参数
        assert!(validate_params_update(&d, &obj(json!({ "flag": true, "target": null })), true).is_err());
        assert!(validate_params_update(&d, &obj(json!({ "target": "sonnet", "flag": true })), true).is_ok());
    }

    fn notifier_decls() -> Vec<NotifierParamDecl> {
        cc_router_plugin_spec::notifier_plugin::parse(
            r#"{
                "notifierApi": 1,
                "params": [
                    { "id": "server", "type": "base_url", "label": "Server", "default": "https://ntfy.sh" },
                    { "id": "topic", "type": "string", "label": "Topic" },
                    { "id": "token", "type": "secret", "label": "Token", "optional": true },
                    { "id": "bot_key", "type": "secret", "label": "Bot Key" }
                ],
                "events": ["test"],
                "request": {
                    "method": "POST",
                    "url": "{{params.server}}/{{params.topic}}",
                    "headers": { "Authorization": "{{#if params.token}}Bearer {{params.token}}{{/if}}", "X-Key": "{{params.bot_key}}" }
                }
            }"#,
        )
        .unwrap()
        .0
        .params
    }

    #[test]
    fn notifier_secret_params_preserved_cleared_and_redacted() {
        let d = notifier_decls();
        // 缺必填参数时拒绝启用
        assert!(validate_notifier_enable(&d, &Map::new()).is_err());
        let initial = obj(json!({ "topic": "alerts", "bot_key": "sec-1", "token": "tok-1" }));
        assert!(validate_notifier_enable(&d, &initial).is_ok());

        // 回传给前端时剥离 secret 明文, 只留 id
        let (safe, set_ids) = redact_notifier_params(&d, &initial);
        assert_eq!(safe.get("topic"), Some(&json!("alerts")));
        assert!(!safe.contains_key("bot_key"));
        assert!(!safe.contains_key("token"));
        assert_eq!(set_ids, vec!["bot_key".to_string(), "token".to_string()]);

        // 修改 topic 时省略或传空串 secret → 保留原密文; 显式传 null → 清空可选 secret
        let updated = validate_notifier_params_update(
            &d,
            &initial,
            &obj(json!({ "topic": "ops", "bot_key": "", "token": null })),
            true,
        )
        .unwrap();
        assert_eq!(updated["topic"], "ops");
        assert_eq!(updated["bot_key"], "sec-1", "留空必须保留原有 secret");
        assert!(!updated.contains_key("token"), "显式 null 应清空可选 secret");

        // 启用状态下清空必填 secret 被拒绝
        assert!(validate_notifier_params_update(
            &d,
            &initial,
            &obj(json!({ "topic": "ops", "bot_key": null })),
            true,
        )
        .is_err());

        // 公网 http base_url 被拒绝, 私网 http 允许
        assert!(validate_notifier_params_update(
            &d,
            &initial,
            &obj(json!({ "server": "http://ntfy.sh", "topic": "ops" })),
            true,
        )
        .is_err());
        assert!(validate_notifier_params_update(
            &d,
            &initial,
            &obj(json!({ "server": "http://192.168.1.2:8080", "topic": "ops" })),
            true,
        )
        .is_ok());
    }
}
