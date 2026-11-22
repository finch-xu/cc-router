//! 厂商注册表: 内置 yaml + 已安装的厂商插件 (设计 §4.2)。
//!
//! `AppState.providers` 存的是 `Arc<ProviderRegistry>` 快照, 装 / 卸 / 启停插件时整体替换;
//! 读取点拿 `state.providers()` 返回的 `Arc`, 不阻塞写入。`Deref` 到内部的 `HashMap`, 所以
//! `get` / `values` 等读法与原来的 `HashMap<String, Provider>` 一致。

use std::collections::HashMap;
use std::ops::Deref;

use crate::plugins::SourceKind;
use crate::provider::model::Provider;

/// 插件厂商 id 的前缀: 注册表 key 与 `subscriptions.provider_id` 都是 `plugin:<插件 id>`。
pub const PLUGIN_PROVIDER_PREFIX: &str = "plugin:";

pub fn plugin_provider_id(plugin_id: &str) -> String {
    format!("{PLUGIN_PROVIDER_PREFIX}{plugin_id}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMeta {
    pub plugin_id: String,
    pub version: String,
    pub enabled: bool,
    pub source_kind: SourceKind,
}

#[derive(Debug, Clone, Default)]
pub struct ProviderRegistry {
    map: HashMap<String, Provider>,
    /// key = 注册表 id (`plugin:<id>`)。内置厂商不在这里。
    plugin_meta: HashMap<String, PluginMeta>,
}

impl ProviderRegistry {
    pub fn builtin(map: HashMap<String, Provider>) -> Self {
        Self { map, plugin_meta: HashMap::new() }
    }

    /// 在内置表上叠加插件厂商。`provider.id` 必须已经是 `plugin:<id>`; 与内置撞名的直接丢弃
    /// (结构上不会发生: 内置 id 不含冒号, 见 loader 的单测)。
    pub fn with_plugins(builtin: HashMap<String, Provider>, plugins: Vec<(Provider, PluginMeta)>) -> Self {
        let mut reg = Self::builtin(builtin);
        for (provider, meta) in plugins {
            if !provider.id.starts_with(PLUGIN_PROVIDER_PREFIX) || reg.map.contains_key(&provider.id) {
                tracing::warn!(id = %provider.id, "插件厂商 id 不合法或与已有厂商重复, 跳过");
                continue;
            }
            reg.plugin_meta.insert(provider.id.clone(), meta);
            reg.map.insert(provider.id.clone(), provider);
        }
        reg
    }

    /// 插件厂商的元信息; 内置厂商为 None。
    pub fn plugin_meta(&self, id: &str) -> Option<&PluginMeta> {
        self.plugin_meta.get(id)
    }

    /// 能否用来新建订阅: 内置厂商恒可; 插件厂商要启用。
    pub fn is_selectable(&self, id: &str) -> bool {
        self.map.contains_key(id) && self.plugin_meta.get(id).is_none_or(|m| m.enabled)
    }

    /// 内置部分 (重建注册表时保留)。
    pub fn builtin_map(&self) -> HashMap<String, Provider> {
        self.map.iter().filter(|(k, _)| !self.plugin_meta.contains_key(*k)).map(|(k, v)| (k.clone(), v.clone())).collect()
    }
}

impl Deref for ProviderRegistry {
    type Target = HashMap<String, Provider>;

    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::loader::parse_single_for_test;

    fn provider(id: &str) -> Provider {
        parse_single_for_test(&format!(
            "id: {id}\ndisplay_name: X\ncategory: aggregator\ncompatibility: verified\nauth: {{ type: api_key, header_name: Authorization, header_format: bearer }}\nendpoints:\n  - {{ id: main, label: Main, base_url: 'https://a.example.com', messages_path: /v1/messages }}\nmodel_discovery: {{ enabled: false }}\n"
        ))
    }

    fn meta(enabled: bool) -> PluginMeta {
        PluginMeta { plugin_id: "relay".into(), version: "1.0.0".into(), enabled, source_kind: SourceKind::Local }
    }

    #[test]
    fn plugins_are_layered_on_builtins_and_disabled_ones_are_not_selectable() {
        let builtin = HashMap::from([("deepseek".to_string(), provider("deepseek"))]);
        let reg = ProviderRegistry::with_plugins(
            builtin,
            vec![(provider("plugin:relay"), meta(false)), (provider("deepseek"), meta(true))],
        );
        assert_eq!(reg.len(), 2, "不带前缀的插件厂商被丢弃");
        assert!(reg.is_selectable("deepseek"));
        assert!(reg.get("plugin:relay").is_some(), "停用的插件厂商仍在注册表里 (已有订阅要用)");
        assert!(!reg.is_selectable("plugin:relay"));
        assert!(!reg.is_selectable("missing"));
        assert_eq!(reg.builtin_map().len(), 1);
        assert_eq!(plugin_provider_id("relay"), "plugin:relay");
    }
}
