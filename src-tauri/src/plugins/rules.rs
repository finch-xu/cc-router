//! 规则注册表: 已启用的规则插件 + 解析好的用户参数, 按优先级排好 (设计 §6)。
//!
//! 请求路径上只做一次 `state.rules()` 快照读取, 在 `pipeline::dispatch` 开头依次跑
//! `route` 与 `request` 两个钩子 (`count_tokens` 只跑 `route`)。

use cc_router_plugin_spec::rules_engine::{self, Applied, Facts, LoadedRules, RouteDecision};
use chrono::{Datelike, Local, Timelike};
use serde_json::Value;

use crate::proxy::client_fingerprint::RequestEntryKind;
use crate::virtual_model::VirtualModelName;

#[derive(Debug, Clone, Default)]
pub struct RulesRegistry {
    /// 按优先级从高到低; 同优先级按插件 id
    plugins: Vec<LoadedRules>,
}

/// 一次 `pipeline::dispatch` 的规则求值结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineRulesOutcome {
    /// 正常继续 (可能已改道 `redirected_to`, 且 `body` 可能已被就地改写)
    Continue {
        redirected_to: Option<VirtualModelName>,
        applied: Vec<Applied>,
    },
    /// 命中 `reject` 动作: 返回 400 (唯一能让请求失败的规则动作)
    Reject {
        message: String,
        applied: Vec<Applied>,
    },
}

impl RulesRegistry {
    /// `plugins`: (已解析的规则插件 + 参数, 优先级)。
    pub fn new(mut plugins: Vec<(LoadedRules, i64)>) -> Self {
        plugins.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.plugin_id.cmp(&b.0.plugin_id)));
        Self { plugins: plugins.into_iter().map(|(p, _)| p).collect() }
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    pub fn apply_route(&self, facts: &Facts, body: &Value) -> (RouteDecision, Vec<Applied>) {
        if self.plugins.is_empty() {
            return (RouteDecision::Continue, Vec::new());
        }
        rules_engine::apply_route(&self.plugins, facts, body)
    }

    pub fn apply_request(&self, facts: &Facts, body: &mut Value) -> (Option<String>, Vec<Applied>) {
        if self.plugins.is_empty() {
            return (None, Vec::new());
        }
        rules_engine::apply_request(&self.plugins, facts, body)
    }

    /// 完整执行一次对话请求的 `route` + `request` 钩子。
    /// `route` 改道后, `request` 钩子里的 `virtual_model` 条件看到的是改道后的虚拟模型。
    pub fn evaluate_pipeline(&self, facts: Facts<'_>, body: &mut Value) -> PipelineRulesOutcome {
        if self.plugins.is_empty() {
            return PipelineRulesOutcome::Continue { redirected_to: None, applied: Vec::new() };
        }
        let (decision, route_log) = self.apply_route(&facts, body);
        log_errors(&route_log);
        let redirected_to = match decision {
            RouteDecision::Continue => None,
            RouteDecision::Reject(message) => {
                return PipelineRulesOutcome::Reject { message, applied: route_log };
            }
            RouteDecision::UseVirtualModel(short) => vm_from_short_name(&short),
        };
        let req_facts = Facts {
            virtual_model: redirected_to.as_ref().map(vm_short_name).unwrap_or(facts.virtual_model),
            ..facts
        };
        let (reject_msg, req_log) = self.apply_request(&req_facts, body);
        log_errors(&req_log);
        let applied = merge_applied(route_log, req_log);
        if let Some(message) = reject_msg {
            return PipelineRulesOutcome::Reject { message, applied };
        }
        PipelineRulesOutcome::Continue { redirected_to, applied }
    }
}

fn log_errors(applied: &[Applied]) {
    for a in applied {
        if let Some(err) = &a.error {
            tracing::warn!(plugin = %a.plugin_id, version = %a.version, error = %err, "规则插件求值出错, 已跳过");
        }
    }
}

/// 合并同一插件在 `route` 和 `request` 两个钩子里的记录。
pub fn merge_applied(mut route_log: Vec<Applied>, req_log: Vec<Applied>) -> Vec<Applied> {
    for item in req_log {
        if let Some(existing) = route_log
            .iter_mut()
            .find(|a| a.plugin_id == item.plugin_id && a.version == item.version)
        {
            existing.rules.extend(item.rules);
            if existing.error.is_none() {
                existing.error = item.error;
            }
        } else {
            route_log.push(item);
        }
    }
    route_log
}

/// 序列化成 `requests.plugins_applied` 列的 JSON 文本 (空列表 → None)。
pub fn serialize_applied(applied: &[Applied]) -> Option<String> {
    (!applied.is_empty()).then(|| serde_json::to_string(applied).ok()).flatten()
}

/// 本地时间: (1 = 周一 … 7 = 周日, 0 点起的分钟数)。
pub fn local_weekday_minute() -> (u8, u16) {
    let now = Local::now();
    (now.weekday().number_from_monday() as u8, (now.hour() * 60 + now.minute()) as u16)
}

/// 虚拟模型在规则条件 / 动作里的短名 (不含 `model-` 前缀)。
pub fn vm_short_name(vm: &VirtualModelName) -> &str {
    match vm {
        VirtualModelName::Fable => "fable",
        VirtualModelName::Opus => "opus",
        VirtualModelName::Sonnet => "sonnet",
        VirtualModelName::Haiku => "haiku",
        VirtualModelName::Fallback => "fallback",
        VirtualModelName::Jev => "jev",
        VirtualModelName::Custom(s) => s,
    }
}

/// 规则里的虚拟模型短名 → `VirtualModelName` (`jev` 不在规则允许范围内)。
pub fn vm_from_short_name(s: &str) -> Option<VirtualModelName> {
    match s {
        "fable" => Some(VirtualModelName::Fable),
        "opus" => Some(VirtualModelName::Opus),
        "sonnet" => Some(VirtualModelName::Sonnet),
        "haiku" => Some(VirtualModelName::Haiku),
        "fallback" => Some(VirtualModelName::Fallback),
        _ => None,
    }
}

/// 入口类型在规则 `when.entry` 里的取值 (见 `rules_plugin::ENTRIES`)。
pub fn entry_rule_name(kind: RequestEntryKind) -> &'static str {
    match kind {
        RequestEntryKind::Messages => "messages",
        RequestEntryKind::Responses => "responses",
        RequestEntryKind::ChatCompletions => "chat_completions",
        RequestEntryKind::SystemOne => "systemone",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cc_router_plugin_spec::rules_plugin::parse;
    use serde_json::{json, Map};

    fn loaded(id: &str, src: &str, params: Value) -> LoadedRules {
        let (plugin, _) = parse(src).unwrap_or_else(|e| panic!("{e:?}"));
        LoadedRules {
            plugin_id: id.into(),
            version: "1.0.0".into(),
            plugin,
            params: params.as_object().cloned().unwrap_or_else(Map::new),
        }
    }

    fn sample_facts<'a>(purpose: &'a str, vm: &'a str) -> Facts<'a> {
        Facts {
            purpose,
            purpose_guessed: true,
            virtual_model: vm,
            client_tool: "claude-code",
            entry: "messages",
            model: "model-opus",
            weekday: 3,
            minute: 600,
        }
    }

    #[test]
    fn route_redirect_feeds_into_request_hook_and_merges_log() {
        let src = r#"{
            "rulesApi": 1,
            "params": [{ "id": "cheap", "type": "virtual_model", "label": "Cheap", "default": "haiku" }],
            "rules": [
                { "id": "side", "hook": "route", "when": { "purpose": ["title"], "virtual_model": ["opus"] },
                  "do": [{ "use_virtual_model": "{{params.cheap}}" }] },
                { "id": "brief", "hook": "request", "when": { "virtual_model": ["haiku"] },
                  "do": [{ "system_append": { "text": "Be concise." } }] }
            ]
        }"#;
        let reg = RulesRegistry::new(vec![(loaded("side-tasks", src, json!({ "cheap": "haiku" })), 0)]);
        let mut body = json!({ "model": "model-opus", "messages": [{ "role": "user", "content": "hi" }] });
        let out = reg.evaluate_pipeline(sample_facts("title", "opus"), &mut body);
        match out {
            PipelineRulesOutcome::Continue { redirected_to, applied } => {
                assert_eq!(redirected_to, Some(VirtualModelName::Haiku));
                assert_eq!(body["system"], "Be concise.");
                assert_eq!(applied.len(), 1, "同一插件的两个钩子合并成一条记录");
                assert_eq!(applied[0].rules, vec!["side", "brief"]);
                assert!(applied[0].error.is_none());
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn reject_stops_pipeline_and_reports_reason() {
        let src = r#"{
            "rulesApi": 1,
            "rules": [
                { "id": "no-img", "hook": "route", "when": { "has_images": true },
                  "do": [{ "reject": { "message": "images blocked" } }] }
            ]
        }"#;
        let reg = RulesRegistry::new(vec![(loaded("guard", src, json!({})), 0)]);
        let mut body = json!({ "messages": [{ "role": "user", "content": [{ "type": "image" }] }] });
        match reg.evaluate_pipeline(sample_facts("main", "opus"), &mut body) {
            PipelineRulesOutcome::Reject { message, applied } => {
                assert_eq!(message, "images blocked");
                assert_eq!(applied[0].rules, vec!["no-img"]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn error_in_one_plugin_does_not_block_request_or_other_plugins() {
        let broken = r#"{
            "rulesApi": 1,
            "params": [{ "id": "vm", "type": "virtual_model", "label": "VM" }],
            "rules": [{ "id": "bad", "hook": "route", "when": {}, "do": [{ "use_virtual_model": "{{params.vm}}" }] }]
        }"#;
        let good = r#"{
            "rulesApi": 1,
            "rules": [{ "id": "ok", "hook": "request", "when": {}, "do": [{ "set_param": { "path": "temperature", "value": 0.1 } }] }]
        }"#;
        let reg = RulesRegistry::new(vec![
            (loaded("broken", broken, json!({})), 10),
            (loaded("good", good, json!({})), 0),
        ]);
        let mut body = json!({ "messages": [] });
        match reg.evaluate_pipeline(sample_facts("main", "opus"), &mut body) {
            PipelineRulesOutcome::Continue { redirected_to, applied } => {
                assert!(redirected_to.is_none());
                assert_eq!(body["temperature"], 0.1);
                assert_eq!(applied.len(), 2);
                assert!(applied[0].error.is_some());
                assert_eq!(applied[1].rules, vec!["ok"]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn priority_orders_higher_first_then_by_id() {
        let mk = |vm: &str| {
            format!(r#"{{"rulesApi":1,"rules":[{{"id":"r","hook":"route","when":{{}},"do":[{{"use_virtual_model":"{vm}"}}]}}]}}"#)
        };
        let reg = RulesRegistry::new(vec![
            (loaded("b", &mk("sonnet"), json!({})), 0),
            (loaded("a", &mk("haiku"), json!({})), 10),
        ]);
        let (d, _) = reg.apply_route(&sample_facts("main", "opus"), &json!({}));
        assert_eq!(d, RouteDecision::UseVirtualModel("haiku".into()));
    }

    #[test]
    fn entry_names_match_spec_entries() {
        use cc_router_plugin_spec::rules_plugin::ENTRIES;
        for kind in [RequestEntryKind::Messages, RequestEntryKind::Responses, RequestEntryKind::ChatCompletions] {
            assert!(ENTRIES.contains(&entry_rule_name(kind)), "{kind:?}");
        }
    }
}
