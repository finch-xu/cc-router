//! 厂商插件 (`type: "provider"`) 的解析与校验 (设计 §4)。
//!
//! 入口文件与内置 `providers/*.yaml` 同一份格式, 额外约束:
//! - 顶层 `id` 一律替换成 `plugin:<插件 id>`;
//! - 上屏文字可以只写部分语言, 解析前按 en → zh → zh-Hant → ja 补齐, 之后走内置那份严格的反序列化;
//! - 不允许 OAuth 类鉴权;
//! - 所有 URL 必须 https, 或 http 指向本机; host 里不许有占位符 (能力摘要要能静态列出 Key 会发往哪里)。

use std::collections::BTreeMap;

use serde_yaml::{Mapping, Value};

use crate::check::{Issue, IssueCode, KeyHost, ProviderCapabilities};
use crate::provider::model::{AuthType, Provider};
use crate::provider::semantics;

pub const PLUGIN_PROVIDER_PREFIX: &str = "plugin:";

/// 插件允许的鉴权类型。OAuth 类有专属登录流程与合规 UA, 只能内置。
pub const ALLOWED_AUTH: &[AuthType] = &[
    AuthType::ApiKey,
    AuthType::GeminiApiKey,
    AuthType::GeminiInteractionsApiKey,
    AuthType::OpenaiResponsesApiKey,
    AuthType::OpenaiChatCompletionsApiKey,
];

const LANGS: [&str; 4] = ["zh", "zh-Hant", "en", "ja"];
/// 补齐缺失语言时的取值顺序
const FALLBACK: [&str; 4] = ["en", "zh", "zh-Hant", "ja"];

#[derive(Debug, Clone)]
pub struct ProviderPlugin {
    pub provider: Provider,
    pub capabilities: ProviderCapabilities,
}

pub fn parse(entry: &str, plugin_id: &str) -> Result<ProviderPlugin, Vec<Issue>> {
    let invalid = |path: &str, detail: String| vec![Issue::new(path, IssueCode::InvalidValue).detail(detail)];
    let mut value: Value = serde_yaml::from_str(entry).map_err(|e| invalid("", format!("not valid YAML: {e}")))?;
    let Value::Mapping(map) = &mut value else {
        return Err(vec![Issue::new("", IssueCode::InvalidType).detail("top level must be a mapping")]);
    };
    map.insert(Value::from("id"), Value::from(format!("{PLUGIN_PROVIDER_PREFIX}{plugin_id}")));
    fill_texts(map);

    let provider: Provider = serde_yaml::from_value(value).map_err(|e| invalid("", e.to_string()))?;
    semantics::validate(&provider).map_err(|e| invalid("", e.to_string()))?;

    let mut issues = Vec::new();
    if !ALLOWED_AUTH.contains(&provider.auth.auth_type) {
        issues.push(
            Issue::new("auth.type", IssueCode::AuthTypeNotAllowed).detail(provider.auth.auth_type.as_str().to_string()),
        );
    }

    // host -> used_for
    let mut hosts: BTreeMap<String, Vec<&'static str>> = BTreeMap::new();
    let mut messages_hosts: Vec<String> = Vec::new();
    let add = |hosts: &mut BTreeMap<String, Vec<&'static str>>, host: String, what: &'static str| {
        let e = hosts.entry(host).or_default();
        if !e.contains(&what) {
            e.push(what);
        }
    };
    for (i, e) in provider.endpoints.iter().enumerate() {
        match check_url(&format!("endpoints[{i}].base_url"), &e.base_url) {
            Ok(h) => {
                messages_hosts.push(h.clone());
                add(&mut hosts, h.clone(), "messages");
                // 没有完整 URL 的模型列表走 base_url + path, 同一个 host
                if provider.model_discovery.enabled && provider.model_discovery.url.is_none() {
                    add(&mut hosts, h, "models");
                }
            }
            Err(issue) => issues.push(issue),
        }
    }
    if let Some(u) = provider.model_discovery.url.as_deref() {
        match check_url("model_discovery.url", u) {
            Ok(h) if provider.model_discovery.enabled => add(&mut hosts, h, "models"),
            Ok(_) => {}
            Err(issue) => issues.push(issue),
        }
    }
    if let Some(b) = &provider.balance_discovery {
        match check_url("balance_discovery.url", &b.url) {
            Ok(h) if b.enabled => add(&mut hosts, h, "balance"),
            Ok(_) => {}
            Err(issue) => issues.push(issue),
        }
    }
    // 只在浏览器里打开的链接: 同样只许 https, 但不算 Key 发往的地方
    for (path, u) in [
        ("homepage", provider.homepage.as_deref()),
        ("docs_url", provider.docs_url.as_deref()),
        ("api_key_url", provider.api_key_url.as_deref()),
    ] {
        if let Some(u) = u {
            if !u.starts_with("https://") || check_url(path, u).is_err() {
                issues.push(Issue::new(path, IssueCode::InsecureUrl).detail("https only"));
            }
        }
    }

    if !issues.is_empty() {
        return Err(issues);
    }
    let key_hosts = hosts
        .into_iter()
        .map(|(host, used_for)| KeyHost { differs_from_messages: !messages_hosts.contains(&host), host, used_for })
        .collect();
    let capabilities = ProviderCapabilities {
        auth_type: provider.auth.auth_type.as_str().to_string(),
        key_hosts,
        endpoints: provider.endpoints.len(),
    };
    Ok(ProviderPlugin { provider, capabilities })
}

/// 检查一个会收到 API Key 的 URL, 返回它的 host (小写, 带非默认端口)。
fn check_url(path: &str, url: &str) -> Result<String, Issue> {
    let (scheme, rest) = if let Some(r) = url.strip_prefix("https://") {
        ("https", r)
    } else if let Some(r) = url.strip_prefix("http://") {
        ("http", r)
    } else {
        return Err(Issue::new(path, IssueCode::InsecureUrl).detail("https or loopback http only"));
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('{') || authority.contains('}') {
        return Err(Issue::new(path, IssueCode::HostPlaceholder));
    }
    if authority.is_empty() || authority.contains('@') || authority.chars().any(|c| c.is_whitespace()) {
        return Err(Issue::new(path, IssueCode::InvalidValue).detail("bad host"));
    }
    let host = authority.to_ascii_lowercase();
    if scheme == "http" {
        let name = match host.strip_prefix('[') {
            Some(v6) => v6.split(']').next().map(|h| format!("[{h}]")).unwrap_or_default(),
            None => host.split(':').next().unwrap_or("").to_string(),
        };
        if !matches!(name.as_str(), "localhost" | "127.0.0.1" | "[::1]") {
            return Err(Issue::new(path, IssueCode::InsecureUrl).detail("http is only allowed for localhost"));
        }
    }
    Ok(host)
}

/// 上屏文字字段: 对象写法缺的语言按 FALLBACK 补齐 (纯字符串写法不动)。
fn fill_texts(map: &mut Mapping) {
    for key in ["display_name", "description", "compatibility_notes"] {
        if let Some(v) = map.get_mut(key) {
            fill_one(v);
        }
    }
    for (list, keys) in [("endpoints", &["label", "description"][..]), ("url_params", &["label", "placeholder"][..])] {
        if let Some(Value::Sequence(items)) = map.get_mut(list) {
            for item in items {
                if let Value::Mapping(m) = item {
                    for k in keys {
                        if let Some(v) = m.get_mut(*k) {
                            fill_one(v);
                        }
                    }
                }
            }
        }
    }
}

fn fill_one(v: &mut Value) {
    let Value::Mapping(m) = v else { return };
    // 只处理「键全是语言名」的对象; 别的形状留给严格反序列化去报错
    if m.is_empty() || !m.keys().all(|k| k.as_str().is_some_and(|k| LANGS.contains(&k))) {
        return;
    }
    let Some(fallback) = FALLBACK.iter().find_map(|l| m.get(*l).and_then(Value::as_str).map(str::to_string)) else {
        return;
    };
    for l in LANGS {
        if !m.contains_key(l) {
            m.insert(Value::from(l), Value::from(fallback.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"
id: whatever
display_name: { zh: 示例中转站, en: Example Relay }
category: aggregator
compatibility: verified
auth: { type: api_key, header_name: Authorization, header_format: bearer }
endpoints:
  - id: main
    label: { en: Main }
    base_url: https://api.example.com/anthropic
    messages_path: /v1/messages
model_discovery: { enabled: true, url: "https://models.example.com/v1/models" }
"#;

    fn with(extra: &str) -> String {
        format!("{BASE}{extra}")
    }

    #[test]
    fn a_valid_plugin_gets_namespaced_id_filled_texts_and_capabilities() {
        let p = parse(&with(""), "relay").unwrap();
        assert_eq!(p.provider.id, "plugin:relay");
        assert_eq!(p.provider.display_name.ja, "Example Relay", "缺 ja 时取 en");
        assert_eq!(p.provider.display_name.zh, "示例中转站");
        assert_eq!(p.provider.endpoints[0].label.zh, "Main");
        let hosts: Vec<_> = p.capabilities.key_hosts.iter().map(|h| (h.host.as_str(), h.differs_from_messages)).collect();
        assert_eq!(hosts, vec![("api.example.com", false), ("models.example.com", true)]);
        assert_eq!(p.capabilities.auth_type, "api_key");
    }

    #[test]
    fn balance_host_is_listed_as_receiving_the_key() {
        let p = parse(
            &with("balance_discovery:\n  url: https://api.example.com/api/user/self\n  parser: json\n  json: { remaining: \"$.data.quota\", unit: USD, divisor: 500000 }\n"),
            "relay",
        )
        .unwrap();
        let api = p.capabilities.key_hosts.iter().find(|h| h.host == "api.example.com").unwrap();
        assert_eq!(api.used_for, vec!["messages", "balance"]);
    }

    fn codes(src: &str) -> Vec<IssueCode> {
        parse(src, "relay").unwrap_err().into_iter().map(|i| i.code).collect()
    }

    #[test]
    fn insecure_or_templated_hosts_and_oauth_are_rejected() {
        assert_eq!(codes(&BASE.replace("https://api.example.com", "http://api.example.com")), vec![IssueCode::InsecureUrl]);
        assert!(parse(&BASE.replace("https://api.example.com", "http://localhost:11434"), "relay").is_ok());
        assert!(parse(&BASE.replace("https://api.example.com", "http://127.0.0.1:8080"), "relay").is_ok());
        assert_eq!(
            codes(&with("url_params:\n  - { id: tenant, label: Tenant, pattern: '^[a-z]+$' }\n").replace("api.example.com", "{tenant}.example.com")),
            vec![IssueCode::HostPlaceholder]
        );
        assert_eq!(codes(&BASE.replace("type: api_key", "type: chatgpt_oauth")), vec![IssueCode::AuthTypeNotAllowed]);
        assert_eq!(codes(&BASE.replace("https://api.example.com", "https://user@api.example.com")), vec![IssueCode::InvalidValue]);
        assert_eq!(codes(&with("homepage: http://example.com\n")), vec![IssueCode::InsecureUrl]);
    }

    #[test]
    fn bad_balance_rules_fail_semantics() {
        assert!(parse(&with("balance_discovery: { url: \"https://a.example.com/b\", parser: json }\n"), "relay").is_err());
        assert!(parse(
            &with("balance_discovery: { url: \"https://a.example.com/b\", parser: json, json: { remaining: \"data\", unit: USD } }\n"),
            "relay"
        )
        .is_err());
        assert!(parse(
            &with("balance_discovery: { url: \"https://a.example.com/b\", parser: json, json: { remaining: \"$.a\", unit: EUR } }\n"),
            "relay"
        )
        .is_err());
        assert!(parse(
            &with("balance_discovery: { url: \"https://a.example.com/b\", parser: deepseek, json: { remaining: \"$.a\", unit: USD } }\n"),
            "relay"
        )
        .is_err());
    }

    #[test]
    fn garbage_is_reported_not_panicked() {
        assert!(parse("- just\n- a list\n", "relay").is_err());
        assert!(parse(":::", "relay").is_err());
        assert!(parse("display_name: X\n", "relay").is_err());
    }
}
