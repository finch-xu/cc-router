//! 通知插件注册表: 已启用的 `type: "notifier"` 插件 + 已解析用户参数, 按优先级排好 (设计 §7)。

use cc_router_plugin_spec::notifier_plugin::NotifierPlugin;
use serde_json::{Map, Value};

#[derive(Debug, Clone)]
pub struct LoadedNotifier {
    pub plugin_id: String,
    pub version: String,
    pub plugin: NotifierPlugin,
    pub params: Map<String, Value>,
}

#[derive(Debug, Clone, Default)]
pub struct NotifierRegistry {
    plugins: Vec<LoadedNotifier>,
}

impl NotifierRegistry {
    pub fn new(mut plugins: Vec<(LoadedNotifier, i64)>) -> Self {
        plugins.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.plugin_id.cmp(&b.0.plugin_id)));
        Self { plugins: plugins.into_iter().map(|(p, _)| p).collect() }
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    pub fn plugins(&self) -> &[LoadedNotifier] {
        &self.plugins
    }

    pub fn get(&self, plugin_id: &str) -> Option<&LoadedNotifier> {
        self.plugins.iter().find(|p| p.plugin_id == plugin_id)
    }
}
