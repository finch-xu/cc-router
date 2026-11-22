//! 系统插件的校验结果: 问题列表 + 由内容推导出的能力摘要 (+ 与已装版本的能力差异)。
//! `Issue` 与前端 `src/plugins/manifest.ts::Issue` 同形, 前端按 `code` 翻译主句。

use serde::Serialize;

/// 与前端 `IssueCode` 同值 (snake_case)。新增值要在前端四语文案里各加一句。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueCode {
    Missing,
    InvalidType,
    InvalidValue,
    UnknownKey,
    UnsupportedType,
    UnsupportedVersion,
    NeedsNewerApp,
    ReservedId,
    BadPath,
    NotInAssets,
    WrongFileKind,
    Duplicate,
    TooMany,
    /// URL 不是 https, 也不是指向本机的 http
    InsecureUrl,
    /// URL 的 host 部分含占位符 (能力摘要列不出会连哪里)
    HostPlaceholder,
    /// 插件不允许使用的鉴权类型 (OAuth 类只能内置)
    AuthTypeNotAllowed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Issue {
    /// 出问题的位置, 如 `endpoints[0].base_url`
    pub path: String,
    pub code: IssueCode,
    /// 给作者看的补充 (英文开发者文字)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Issue {
    pub fn new(path: impl Into<String>, code: IssueCode) -> Self {
        Self { path: path.into(), code, detail: None }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// 一个会收到 API Key 的 host。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyHost {
    pub host: String,
    /// `messages` / `models` / `balance`
    pub used_for: Vec<&'static str>,
    /// 不是任何对话端点的 host (界面标黄)
    pub differs_from_messages: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderCapabilities {
    /// `AuthType` 的 snake_case 名
    pub auth_type: String,
    /// 按 host 字典序
    pub key_hosts: Vec<KeyHost>,
    pub endpoints: usize,
}

/// 计价插件: 币种、条目数、覆盖的厂商 (含 `*`)。只影响费用显示, 不碰请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PricingCapabilities {
    pub currency: String,
    pub rules: usize,
    pub providers: Vec<String>,
}

/// 由插件内容推导出的「这个插件会做什么」(不是作者声明的, 作者没法少报)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Capabilities {
    Provider(ProviderCapabilities),
    Pricing(PricingCapabilities),
    Rules(crate::rules_plugin::RulesCapabilities),
    Notifier(crate::notifier_plugin::NotifierCapabilities),
}

/// 与本机已装版本相比的能力变化 (更新确认框用)。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CapabilityDiff {
    pub added_hosts: Vec<String>,
    pub removed_hosts: Vec<String>,
    pub auth_type_changed: bool,
    /// 规则插件: 新版本多出来的钩子 / 动作 (`route` / `reject` / `redact` …)
    pub added_actions: Vec<String>,
    /// 通知插件: 新版本多订阅的事件 / 多引用的事件字段
    pub added_events: Vec<String>,
    pub added_fields: Vec<String>,
}

impl CapabilityDiff {
    pub fn between(old: &Capabilities, new: &Capabilities) -> Self {
        match (old, new) {
            (Capabilities::Provider(o), Capabilities::Provider(n)) => {
                let hosts = |c: &ProviderCapabilities| c.key_hosts.iter().map(|h| h.host.clone()).collect::<Vec<_>>();
                let (oh, nh) = (hosts(o), hosts(n));
                Self {
                    added_hosts: nh.iter().filter(|h| !oh.contains(h)).cloned().collect(),
                    removed_hosts: oh.iter().filter(|h| !nh.contains(h)).cloned().collect(),
                    auth_type_changed: o.auth_type != n.auth_type,
                    ..Self::default()
                }
            }
            (Capabilities::Rules(o), Capabilities::Rules(n)) => {
                let mut added: Vec<String> = n.actions.iter().filter(|a| !o.actions.contains(a)).map(|a| a.to_string()).collect();
                for h in &n.hooks {
                    if !o.hooks.contains(h) {
                        added.push(format!("hook:{}", serde_json::to_value(h).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()));
                    }
                }
                Self { added_actions: added, ..Self::default() }
            }
            (Capabilities::Notifier(o), Capabilities::Notifier(n)) => Self {
                added_hosts: n.hosts.iter().filter(|h| !o.hosts.contains(h)).cloned().collect(),
                removed_hosts: o.hosts.iter().filter(|h| !n.hosts.contains(h)).cloned().collect(),
                added_events: n.events.iter().filter(|e| !o.events.contains(e)).cloned().collect(),
                added_fields: n.fields.iter().filter(|f| !o.fields.contains(f)).cloned().collect(),
                ..Self::default()
            },
            // 计价插件不连网也不改请求, 没有「能力扩大」可言; 类型变了的情况不会出现 (同 id 同类型才比)
            _ => Self::default(),
        }
    }

    /// 能力是否扩大 (启用中的规则 / 通知插件更新后据此自动停用)
    pub fn expands(&self) -> bool {
        !self.added_hosts.is_empty()
            || self.auth_type_changed
            || !self.added_actions.is_empty()
            || !self.added_events.is_empty()
            || !self.added_fields.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckResult {
    pub ok: bool,
    pub issues: Vec<Issue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Capabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<CapabilityDiff>,
}

impl CheckResult {
    pub fn failed(issues: Vec<Issue>) -> Self {
        Self { ok: false, issues, capabilities: None, diff: None }
    }

    pub fn passed(capabilities: Capabilities) -> Self {
        Self { ok: true, issues: Vec::new(), capabilities: Some(capabilities), diff: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(auth: &str, hosts: &[&str]) -> Capabilities {
        Capabilities::Provider(ProviderCapabilities {
            auth_type: auth.into(),
            key_hosts: hosts
                .iter()
                .map(|h| KeyHost { host: (*h).into(), used_for: vec!["messages"], differs_from_messages: false })
                .collect(),
            endpoints: 1,
        })
    }

    #[test]
    fn diff_reports_added_and_removed_hosts() {
        let d = CapabilityDiff::between(&caps("api_key", &["a.com", "b.com"]), &caps("api_key", &["b.com", "c.com"]));
        assert_eq!(d.added_hosts, vec!["c.com"]);
        assert_eq!(d.removed_hosts, vec!["a.com"]);
        assert!(d.expands());
        let same = CapabilityDiff::between(&caps("api_key", &["a.com"]), &caps("api_key", &["a.com"]));
        assert!(!same.expands());
        assert!(CapabilityDiff::between(&caps("api_key", &["a.com"]), &caps("gemini_api_key", &["a.com"])).expands());
    }

    #[test]
    fn issue_serializes_like_the_frontend_shape() {
        let v = serde_json::to_value(Issue::new("endpoints[0].base_url", IssueCode::InsecureUrl).detail("https only")).unwrap();
        assert_eq!(v, serde_json::json!({ "path": "endpoints[0].base_url", "code": "insecure_url", "detail": "https only" }));
        let v = serde_json::to_value(caps("api_key", &[])).unwrap();
        assert_eq!(v["kind"], "provider");
    }
}
