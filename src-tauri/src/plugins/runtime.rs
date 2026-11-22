//! 已安装系统插件 → 运行时 (P1: 厂商插件进厂商注册表)。
//!
//! 磁盘内容不可信: 每次重建都按 spec crate 的规则重新校验, 校验不过的插件 warn 并跳过
//! (与主题「每次应用前重新校验」同理)。

use std::collections::HashMap;
use std::path::Path;

use cc_router_plugin_spec::plugin::{check_system, SystemPlugin};

use crate::db::paths;
use crate::error::{AppError, AppResult};
use crate::plugins::config::{self, PluginConfig};
use crate::plugins::store;
use crate::provider::model::Provider;
use crate::provider::registry::{PluginMeta, ProviderRegistry};
use crate::state::AppState;

/// 已安装的厂商插件, 解析成注册表要的形状 (含停用的: 已有订阅的显示名 / 重建连接要用)。
pub fn installed_provider_plugins(app_data_dir: &Path, configs: &HashMap<String, PluginConfig>) -> Vec<(Provider, PluginMeta)> {
    let Ok(list) = store::list(app_data_dir) else { return Vec::new() };
    let mut out = Vec::new();
    for rec in list.into_iter().filter(|r| r.plugin_type == "provider") {
        let (core, entry) = match store::read_entry(app_data_dir, &rec.id) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(plugin = %rec.id, error = %e, "厂商插件读取失败, 已跳过");
                continue;
            }
        };
        match check_system(&core.plugin_type, &core.id, &entry, &core.assets) {
            (_, Some(SystemPlugin::Provider(p))) => out.push((
                p.provider,
                PluginMeta {
                    plugin_id: rec.id.clone(),
                    version: rec.version.clone(),
                    enabled: config::is_enabled(configs, &rec.id, &rec.plugin_type),
                    source_kind: rec.source.kind,
                },
            )),
            (result, _) => tracing::warn!(plugin = %rec.id, issues = ?result.issues, "厂商插件校验不过, 已跳过"),
        }
    }
    out
}

/// 按磁盘上的插件与配置表重建厂商注册表 (启动时、装 / 卸 / 启停之后)。
pub async fn rebuild_registry(state: &AppState) -> AppResult<()> {
    let dir = paths::app_data_dir(&state.app_handle)?;
    let configs = config::load_all(&state.db).await?;
    let builtin = state.providers().builtin_map();
    let plugins = tokio::task::spawn_blocking(move || installed_provider_plugins(&dir, &configs))
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;
    state.set_providers(ProviderRegistry::with_plugins(builtin, plugins));
    Ok(())
}
