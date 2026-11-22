//! 「按新描述重建连接」(设计 §4.2): 内置或插件厂商的描述更新后, 已有订阅的连接快照不会自动跟着变
//! (快照模型: 请求只读订阅自己的字段)。这里用当前注册表里的描述 + 订阅的端点 / url_params 重算一份,
//! 与现有快照比较; 用户确认后才写回。
//!
//! 只比与连接相关的字段 (地址、路径、固定请求头、鉴权头、模型列表 / 余额地址); 示例模型、缓存时长这类
//! 不影响请求去向的差异不算「过时」, 免得每次升级都提示一遍。重建时这些字段一并刷新。

use std::collections::HashMap;

use serde::Serialize;

use crate::provider::model::{AuthHeaderFormat, AuthType, BalanceDiscovery, Provider};
use crate::subscription::model::SubscriptionRow;
use crate::subscription::snapshot::{snapshot_connection, ConnectionSnapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RebuildStatus {
    UpToDate,
    Changed,
    Blocked,
}

/// 不能重建的原因 (前端按它翻译)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockedReason {
    /// 自定义订阅: 连接信息由用户直接编辑
    UserDefined,
    /// OAuth 订阅: 连接由登录流程管理
    Oauth,
    /// 厂商不在注册表里 (插件已卸载 / yaml 已删除)
    ProviderMissing,
    EndpointMissing,
    /// 订阅保存的 url_params 不再符合新描述的要求
    ParamsInvalid,
    /// 新描述换了鉴权类型
    AuthChanged,
    /// 新描述换了端点协议 (协议创建后不可变)
    ProtocolChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldChange {
    /// `base_url` / `messages_path` / `required_headers` / `auth_header` / `forward_headers` /
    /// `model_discovery_url` / `balance`
    pub field: &'static str,
    pub old: String,
    pub new: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RebuildPreview {
    pub status: RebuildStatus,
    pub changes: Vec<FieldChange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<BlockedReason>,
}

/// 重算出来、待写回的连接字段。
pub struct RebuildPlan {
    snap: ConnectionSnapshot,
    auth_header_name: String,
    auth_header_format: AuthHeaderFormat,
    forward_headers: Vec<String>,
    balance_discovery: Option<BalanceDiscovery>,
}

pub fn plan(row: &SubscriptionRow, providers: &HashMap<String, Provider>) -> Result<(RebuildPlan, Vec<FieldChange>), BlockedReason> {
    if row.is_user_defined {
        return Err(BlockedReason::UserDefined);
    }
    if matches!(row.auth_type, AuthType::ChatgptOauth | AuthType::KiroOauth) {
        return Err(BlockedReason::Oauth);
    }
    let provider = providers.get(&row.provider_id).ok_or(BlockedReason::ProviderMissing)?;
    if provider.auth.auth_type != row.auth_type {
        return Err(BlockedReason::AuthChanged);
    }
    let endpoint = provider.endpoint(&row.endpoint_id).ok_or(BlockedReason::EndpointMissing)?;
    let snap = snapshot_connection(provider, endpoint, &row.url_params).map_err(|_| BlockedReason::ParamsInvalid)?;
    if snap.endpoint_protocol != row.endpoint_protocol {
        return Err(BlockedReason::ProtocolChanged);
    }
    let plan = RebuildPlan {
        snap,
        auth_header_name: provider.auth.header_name.clone(),
        auth_header_format: provider.auth.header_format.clone(),
        forward_headers: provider.forward_headers.clone(),
        balance_discovery: provider.balance_discovery.clone(),
    };
    let changes = diff(row, &plan);
    Ok((plan, changes))
}

fn diff(row: &SubscriptionRow, p: &RebuildPlan) -> Vec<FieldChange> {
    let mut out = Vec::new();
    let mut push = |field: &'static str, old: String, new: String| {
        if old != new {
            out.push(FieldChange { field, old, new });
        }
    };
    push("base_url", row.base_url.clone(), p.snap.base_url.clone());
    push("messages_path", row.messages_path.clone(), p.snap.messages_path.clone());
    let headers = |m: &std::collections::BTreeMap<String, String>| {
        m.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\n")
    };
    push("required_headers", headers(&row.required_headers), headers(&p.snap.required_headers));
    push(
        "auth_header",
        format!("{} ({})", row.auth_header_name, row.auth_header_format.as_str()),
        format!("{} ({})", p.auth_header_name, p.auth_header_format.as_str()),
    );
    push("forward_headers", row.forward_headers.join(", "), p.forward_headers.join(", "));
    let models = |enabled: bool, url: &Option<String>| if enabled { url.clone().unwrap_or_default() } else { "-".into() };
    push(
        "model_discovery_url",
        models(row.model_discovery.enabled, &row.model_discovery.url),
        models(p.snap.model_discovery.enabled, &p.snap.model_discovery.url),
    );
    let balance = |b: &Option<BalanceDiscovery>| {
        b.as_ref()
            .filter(|b| b.enabled)
            .map(|b| format!("{} ({})", b.url, serde_json::to_value(b.parser).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()))
            .unwrap_or_else(|| "-".into())
    };
    push("balance", balance(&row.balance_discovery), balance(&p.balance_discovery));
    out
}

pub fn preview(row: &SubscriptionRow, providers: &HashMap<String, Provider>) -> RebuildPreview {
    match plan(row, providers) {
        Ok((_, changes)) if changes.is_empty() => RebuildPreview { status: RebuildStatus::UpToDate, changes, reason: None },
        Ok((_, changes)) => RebuildPreview { status: RebuildStatus::Changed, changes, reason: None },
        Err(reason) => RebuildPreview { status: RebuildStatus::Blocked, changes: Vec::new(), reason: Some(reason) },
    }
}

/// 描述已更新、可以重建 (订阅编辑页据此显示提示条)。
pub fn is_outdated(row: &SubscriptionRow, providers: &HashMap<String, Provider>) -> bool {
    matches!(plan(row, providers), Ok((_, changes)) if !changes.is_empty())
}

pub fn apply(row: &mut SubscriptionRow, plan: RebuildPlan) {
    let RebuildPlan { snap, auth_header_name, auth_header_format, forward_headers, balance_discovery } = plan;
    row.base_url = snap.base_url;
    row.messages_path = snap.messages_path;
    row.required_headers = snap.required_headers;
    row.model_discovery = snap.model_discovery;
    row.systemone_wire = snap.systemone_wire;
    row.url_params = snap.url_params;
    row.auth_header_name = auth_header_name;
    row.auth_header_format = auth_header_format;
    row.forward_headers = forward_headers;
    row.balance_discovery = balance_discovery;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::loader::{load_all, parse_single_for_test};

    /// 按 create_subscription 的方式从描述建一个订阅行。
    fn row_from(provider: &Provider, endpoint: &str) -> SubscriptionRow {
        let snap = snapshot_connection(provider, provider.endpoint(endpoint).unwrap(), &Default::default()).unwrap();
        let mut row = SubscriptionRow::test_fixture(&provider.id, endpoint);
        row.auth_type = provider.auth.auth_type;
        row.auth_header_name = provider.auth.header_name.clone();
        row.auth_header_format = provider.auth.header_format.clone();
        row.forward_headers = provider.forward_headers.clone();
        row.balance_discovery = provider.balance_discovery.clone();
        row.base_url = snap.base_url;
        row.messages_path = snap.messages_path;
        row.required_headers = snap.required_headers;
        row.model_discovery = snap.model_discovery;
        row.endpoint_protocol = snap.endpoint_protocol;
        row.systemone_wire = snap.systemone_wire;
        row.url_params = snap.url_params;
        row
    }

    fn relay(base_url: &str, auth: &str) -> Provider {
        parse_single_for_test(&format!(
            "id: plugin:relay\ndisplay_name: R\ncategory: aggregator\ncompatibility: untested\nauth: {{ type: {auth}, header_name: Authorization, header_format: bearer }}\nendpoints:\n  - {{ id: main, label: Main, base_url: '{base_url}', messages_path: /v1/messages }}\nmodel_discovery: {{ enabled: false }}\n"
        ))
    }

    fn reg(p: Provider) -> HashMap<String, Provider> {
        HashMap::from([(p.id.clone(), p)])
    }

    #[test]
    fn freshly_created_builtin_subscriptions_are_up_to_date() {
        let providers = load_all().unwrap();
        for p in providers.values().filter(|p| !matches!(p.auth.auth_type, AuthType::ChatgptOauth | AuthType::KiroOauth)) {
            for e in p.endpoints.iter().filter(|e| p.params_used(e).is_empty()) {
                let row = row_from(p, &e.id);
                assert_eq!(preview(&row, &providers).status, RebuildStatus::UpToDate, "{} / {}", p.id, e.id);
            }
        }
    }

    #[test]
    fn changed_host_is_reported_and_applied() {
        let mut row = row_from(&relay("https://a.example.com", "api_key"), "main");
        let newer = reg(relay("https://b.example.com", "api_key"));
        assert!(is_outdated(&row, &newer));
        let p = preview(&row, &newer);
        assert_eq!(p.status, RebuildStatus::Changed);
        assert_eq!(p.changes, vec![FieldChange { field: "base_url", old: "https://a.example.com".into(), new: "https://b.example.com".into() }]);
        let (plan, _) = plan(&row, &newer).unwrap();
        apply(&mut row, plan);
        assert_eq!(row.base_url, "https://b.example.com");
        assert!(!is_outdated(&row, &newer));
    }

    #[test]
    fn blocked_cases() {
        let row = row_from(&relay("https://a.example.com", "api_key"), "main");
        assert_eq!(preview(&row, &HashMap::new()).reason, Some(BlockedReason::ProviderMissing));
        assert_eq!(preview(&row, &reg(relay("https://a.example.com", "gemini_api_key"))).reason, Some(BlockedReason::AuthChanged));
        let mut custom = row.clone();
        custom.is_user_defined = true;
        assert_eq!(preview(&custom, &reg(relay("https://a.example.com", "api_key"))).reason, Some(BlockedReason::UserDefined));
        let mut other_ep = row.clone();
        other_ep.endpoint_id = "gone".into();
        assert_eq!(preview(&other_ep, &reg(relay("https://a.example.com", "api_key"))).reason, Some(BlockedReason::EndpointMissing));
        assert!(!is_outdated(&row, &HashMap::new()), "不能重建就不提示过时");
    }
}
