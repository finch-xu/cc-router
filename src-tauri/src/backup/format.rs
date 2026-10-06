//! 导出文件的结构, 以及「订阅行 ↔ 导出条目」的转换。

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::provider::model::{
    AuthHeaderFormat, AuthType, BalanceDiscovery, EndpointProtocol, ModelDiscovery,
    SystemoneWire,
};
use crate::subscription::model::{ModelSlots, OAuthMetadata, SlotEfforts, SubscriptionRow};
use crate::subscription::quota::TokenQuotas;
use crate::virtual_model::model::{RoutingMode, VirtualModelName};

pub const FORMAT: &str = "cc-router-export";
/// Bump only on incompatible changes; additive optional fields keep the version.
pub const VERSION: u32 = 1;
pub const FIELD_API_KEY: &str = "api_key";

pub fn header_field(name: &str) -> String {
    format!("header:{name}")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportFile {
    pub format: String,
    pub version: u32,
    pub app_version: String,
    pub exported_at: DateTime<Utc>,
    #[serde(default)]
    pub subscriptions: Vec<ExportSubscription>,
    #[serde(default)]
    pub virtual_models: Vec<ExportVirtualModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<SecretsEnvelope>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportSubscription {
    pub id: Uuid,
    pub provider_id: String,
    pub endpoint_id: String,
    pub display_name: String,
    pub auth_type: AuthType,
    pub model_slots: ModelSlots,
    #[serde(default)]
    pub slot_efforts: SlotEfforts,
    #[serde(default)]
    pub token_quotas: TokenQuotas,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub base_url: String,
    pub messages_path: String,
    pub auth_header_name: String,
    pub auth_header_format: AuthHeaderFormat,
    #[serde(default)]
    pub required_headers: BTreeMap<String, String>,
    #[serde(default)]
    pub forward_headers: Vec<String>,
    #[serde(default)]
    pub forward_client_headers: bool,
    /// 端点协议快照; 旧导出文件没有该字段, 按对话协议 (messages) 处理。
    #[serde(default)]
    pub endpoint_protocol: EndpointProtocol,
    /// User-supplied provider params (account_id etc), plaintext: not secrets.
    #[serde(default)]
    pub url_params: BTreeMap<String, String>,
    /// System One upstream dialect snapshot; old exports default to `standard`.
    #[serde(default)]
    pub systemone_wire: SystemoneWire,
    #[serde(default)]
    pub model_discovery: ModelDiscovery,
    #[serde(default)]
    pub balance_discovery: Option<BalanceDiscovery>,
    pub provider_display_name: String,
    #[serde(default)]
    pub provider_icon: String,
    #[serde(default)]
    pub is_user_defined: bool,
    /// Whether the source row had a non-empty api_key. Local models (Ollama etc.) legitimately
    /// have none, so an empty key alone must not be treated as "missing".
    #[serde(default)]
    pub has_api_key: bool,
    /// Sensitive required_headers removed from the plaintext part (values live in `secrets`, if any).
    #[serde(default)]
    pub redacted_headers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_refs: Option<SecretRefs>,
}

impl ExportSubscription {
    pub fn key_destinations(&self) -> Vec<String> {
        key_destinations(&self.base_url, &self.model_discovery, self.balance_discovery.as_ref())
    }

    pub fn is_oauth(&self) -> bool {
        is_oauth(self.auth_type)
    }
}

fn is_oauth(t: AuthType) -> bool {
    matches!(t, AuthType::ChatgptOauth | AuthType::KiroOauth)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecretRefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportVirtualModel {
    pub name: VirtualModelName,
    pub mode: RoutingMode,
    #[serde(default)]
    pub subscription_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretsEnvelope {
    pub kdf: KdfSpec,
    pub cipher: String,
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KdfSpec {
    pub alg: String,
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
    pub salt: String,
}

/// Decrypted content of `secrets.ciphertext`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretsPlain {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,
    #[serde(default)]
    pub items: BTreeMap<String, SecretItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretItem {
    pub value: String,
    pub bind: SecretBind,
}

/// Where a secret is allowed to go. Checked on import so a tampered plaintext part
/// (e.g. swapped base_url) cannot redirect the real key to someone else's server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretBind {
    pub subscription_id: Uuid,
    pub field: String,
    pub destinations: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtractedSecrets {
    pub api_key: Option<String>,
    pub headers: BTreeMap<String, String>,
}

const SENSITIVE_HEADER_MARKERS: [&str; 7] =
    ["auth", "key", "token", "secret", "cookie", "password", "session"];

/// Over-matching only costs one extra encrypted value, so the rule is deliberately broad.
pub fn is_sensitive_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SENSITIVE_HEADER_MARKERS.iter().any(|m| lower.contains(m))
}

/// Every URL this subscription's credentials are sent to, sorted and deduplicated.
///
/// Wire-format note: this output is bound into every encrypted export (as the AAD of each
/// secret). Any change to how the list is built (normalising a trailing `/`, adding joined
/// paths, ...) makes existing encrypted files fail with "file was modified", so such a
/// change requires bumping `VERSION`. `key_destinations_is_frozen_for_v1` pins the output.
pub fn key_destinations(
    base_url: &str,
    md: &ModelDiscovery,
    bd: Option<&BalanceDiscovery>,
) -> Vec<String> {
    let mut out = vec![base_url.to_string()];
    if let Some(u) = md.url.as_deref().filter(|u| !u.trim().is_empty()) {
        out.push(u.to_string());
    }
    if let Some(b) = bd {
        out.push(b.url.clone());
    }
    out.sort();
    out.dedup();
    out
}

pub fn subscription_to_export(row: &SubscriptionRow) -> (ExportSubscription, ExtractedSecrets) {
    // No `..` on purpose: a new SubscriptionRow field breaks compilation here, forcing a
    // decision on whether it is exported in plaintext, encrypted, or never exported.
    let SubscriptionRow {
        id,
        provider_id,
        endpoint_id,
        display_name,
        api_key,
        auth_type,
        oauth_metadata: _, // never exported: refresh tokens rotate and are single-use
        model_slots,
        slot_efforts,
        token_quotas,
        enabled,
        is_auth_failed: _, // runtime state
        last_error_message: _,
        created_at,
        updated_at: _,
        base_url,
        messages_path,
        auth_header_name,
        auth_header_format,
        required_headers,
        forward_headers,
        forward_client_headers,
        endpoint_protocol,
        url_params,
        systemone_wire,
        model_discovery,
        balance_discovery,
        provider_display_name,
        provider_icon,
        is_user_defined,
    } = row;

    let mut plain_headers = BTreeMap::new();
    let mut secret_headers = BTreeMap::new();
    for (k, v) in required_headers {
        if is_sensitive_header(k) {
            secret_headers.insert(k.clone(), v.clone());
        } else {
            plain_headers.insert(k.clone(), v.clone());
        }
    }
    let has_api_key = !is_oauth(*auth_type) && !api_key.is_empty();

    let exp = ExportSubscription {
        id: *id,
        provider_id: provider_id.clone(),
        endpoint_id: endpoint_id.clone(),
        display_name: display_name.clone(),
        auth_type: *auth_type,
        model_slots: model_slots.clone(),
        slot_efforts: slot_efforts.clone(),
        token_quotas: token_quotas.clone(),
        enabled: *enabled,
        created_at: *created_at,
        base_url: base_url.clone(),
        messages_path: messages_path.clone(),
        auth_header_name: auth_header_name.clone(),
        auth_header_format: auth_header_format.clone(),
        required_headers: plain_headers,
        forward_headers: forward_headers.clone(),
        forward_client_headers: *forward_client_headers,
        endpoint_protocol: *endpoint_protocol,
        url_params: url_params.clone(),
        systemone_wire: *systemone_wire,
        model_discovery: model_discovery.clone(),
        balance_discovery: balance_discovery.clone(),
        provider_display_name: provider_display_name.clone(),
        provider_icon: provider_icon.clone(),
        is_user_defined: *is_user_defined,
        has_api_key,
        redacted_headers: secret_headers.keys().cloned().collect(),
        secret_refs: None,
    };
    let secrets = ExtractedSecrets {
        api_key: has_api_key.then(|| api_key.clone()),
        headers: secret_headers,
    };
    (exp, secrets)
}

/// `required_headers` is the final map (plaintext part + recovered or blanked secret headers).
pub fn export_to_row(
    sub: &ExportSubscription,
    api_key: String,
    required_headers: BTreeMap<String, String>,
    enabled: bool,
    now: DateTime<Utc>,
) -> SubscriptionRow {
    SubscriptionRow {
        id: sub.id,
        provider_id: sub.provider_id.clone(),
        endpoint_id: sub.endpoint_id.clone(),
        display_name: sub.display_name.clone(),
        api_key,
        auth_type: sub.auth_type,
        oauth_metadata: OAuthMetadata::default(),
        model_slots: sub.model_slots.clone(),
        slot_efforts: sub.slot_efforts.clone(),
        token_quotas: sub.token_quotas.clone(),
        enabled,
        is_auth_failed: false,
        last_error_message: None,
        created_at: sub.created_at,
        updated_at: now,
        base_url: sub.base_url.clone(),
        messages_path: sub.messages_path.clone(),
        auth_header_name: sub.auth_header_name.clone(),
        auth_header_format: sub.auth_header_format.clone(),
        required_headers,
        forward_headers: sub.forward_headers.clone(),
        forward_client_headers: sub.forward_client_headers,
        endpoint_protocol: sub.endpoint_protocol,
        url_params: sub.url_params.clone(),
        systemone_wire: sub.systemone_wire,
        model_discovery: sub.model_discovery.clone(),
        balance_discovery: sub.balance_discovery.clone(),
        provider_display_name: sub.provider_display_name.clone(),
        provider_icon: sub.provider_icon.clone(),
        is_user_defined: sub.is_user_defined,
    }
}

pub const MAX_FILE_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_SUBSCRIPTIONS: usize = 1000;

pub fn parse_file(text: &str) -> crate::error::AppResult<ExportFile> {
    use crate::error::AppError;
    use std::collections::HashSet;

    if text.len() > MAX_FILE_BYTES {
        return Err(AppError::BadRequest("文件过大 (上限 5 MB)".into()));
    }
    // Read format/version first so "file is from a newer version" is not reported as a field error.
    #[derive(Deserialize)]
    struct Head {
        format: Option<String>,
        version: Option<u32>,
    }
    let head: Head = serde_json::from_str(text)
        .map_err(|e| AppError::BadRequest(format!("不是有效的 JSON 文件: {e}")))?;
    if head.format.as_deref() != Some(FORMAT) {
        return Err(AppError::BadRequest("不是 cc-router 导出的配置文件".into()));
    }
    match head.version {
        Some(v) if v > VERSION => {
            return Err(AppError::BadRequest(format!(
                "文件来自更新版本的 cc-router (格式版本 {v}), 请先升级"
            )))
        }
        Some(_) => {}
        None => return Err(AppError::BadRequest("文件缺少格式版本号".into())),
    }
    let file: ExportFile = serde_json::from_str(text).map_err(|e| {
        AppError::BadRequest(format!(
            "文件内容无法解析: {e}。如果文件来自更新版本的 cc-router, 请先升级"
        ))
    })?;
    if file.subscriptions.len() > MAX_SUBSCRIPTIONS {
        return Err(AppError::BadRequest(format!("订阅数量超过上限 ({MAX_SUBSCRIPTIONS})")));
    }
    let mut ids = HashSet::new();
    if !file.subscriptions.iter().all(|s| ids.insert(s.id)) {
        return Err(AppError::BadRequest("文件里有重复的订阅 id".into()));
    }
    let mut names = HashSet::new();
    if !file.virtual_models.iter().all(|v| names.insert(v.name)) {
        return Err(AppError::BadRequest("文件里有重复的虚拟模型".into()));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::model::{BalanceHttpMethod, BalanceParser};

    #[test]
    fn sensitive_header_table() {
        for name in ["Authorization", "x-api-key", "X-Relay-Key", "x-auth-token", "Cookie", "x-client-secret", "X-Password", "x-session"] {
            assert!(is_sensitive_header(name), "{name} 应判为敏感");
        }
        for name in ["anthropic-version", "anthropic-beta", "OpenAI-Beta", "originator", "user-agent", "content-type"] {
            assert!(!is_sensitive_header(name), "{name} 不应判为敏感");
        }
    }

    #[test]
    fn destinations_cover_every_url_the_key_is_sent_to() {
        let mut md = ModelDiscovery::default();
        assert_eq!(key_destinations("https://a.example", &md, None), vec!["https://a.example".to_string()]);

        md.url = Some("https://models.example/v1/models".into());
        let bd = BalanceDiscovery {
            enabled: true,
            url: "https://bal.example/balance".into(),
            method: BalanceHttpMethod::Get,
            parser: BalanceParser::Deepseek,
            cache_ttl_minutes: 10,
        };
        assert_eq!(
            key_destinations("https://a.example", &md, Some(&bd)),
            vec![
                "https://a.example".to_string(),
                "https://bal.example/balance".to_string(),
                "https://models.example/v1/models".to_string(),
            ],
            "排序去重后三个地址都在"
        );

        md.url = Some("   ".into());
        assert_eq!(key_destinations("https://a.example", &md, None).len(), 1, "空白 url 不算");
    }

    #[test]
    fn key_destinations_is_frozen_for_v1() {
        let md = ModelDiscovery {
            url: Some("https://a.example/v1/models".into()),
            ..ModelDiscovery::default()
        };
        let bd = BalanceDiscovery {
            enabled: true,
            url: "https://a.example/user/balance".into(),
            method: BalanceHttpMethod::Get,
            parser: BalanceParser::Deepseek,
            cache_ttl_minutes: 10,
        };
        assert_eq!(
            key_destinations("https://a.example/anthropic", &md, Some(&bd)),
            vec![
                "https://a.example/anthropic".to_string(),
                "https://a.example/user/balance".to_string(),
                "https://a.example/v1/models".to_string(),
            ],
            "changing this breaks every encrypted v1 export, bump VERSION instead"
        );
    }

    #[test]
    fn url_params_and_wire_survive_export_round_trip() {
        let mut row = SubscriptionRow::test_fixture("cloudflare", "gateway");
        row.base_url = "https://api.example.com".into();
        row.messages_path = "/v1/chat/completions".into();
        row.url_params.insert("account_id".into(), "abc".into());
        row.systemone_wire = crate::provider::model::SystemoneWire::CloudflareRun;
        let (exp, _secrets) = subscription_to_export(&row);
        let json = serde_json::to_value(&exp).unwrap();
        assert_eq!(json["url_params"]["account_id"], "abc");
        assert_eq!(json["systemone_wire"], "cloudflare_run");
        let back: ExportSubscription = serde_json::from_value(json).unwrap();
        assert_eq!(back.url_params.get("account_id").map(String::as_str), Some("abc"));
        assert_eq!(back.systemone_wire, crate::provider::model::SystemoneWire::CloudflareRun);
        let restored = export_to_row(&back, String::new(), BTreeMap::new(), true, Utc::now());
        assert_eq!(restored.url_params.get("account_id").map(String::as_str), Some("abc"));
        assert_eq!(restored.systemone_wire, crate::provider::model::SystemoneWire::CloudflareRun);
    }

    #[test]
    fn old_export_without_new_fields_still_parses() {
        let mut row = SubscriptionRow::test_fixture("p", "e");
        row.base_url = "https://api.example.com".into();
        row.messages_path = "/v1/messages".into();
        let (exp, _) = subscription_to_export(&row);
        let mut json = serde_json::to_value(&exp).unwrap();
        json.as_object_mut().unwrap().remove("url_params");
        json.as_object_mut().unwrap().remove("systemone_wire");
        let back: ExportSubscription = serde_json::from_value(json).unwrap();
        assert!(back.url_params.is_empty());
        assert_eq!(back.systemone_wire, crate::provider::model::SystemoneWire::Standard);
    }

    #[test]
    fn export_splits_sensitive_headers_out() {
        let mut row = SubscriptionRow::test_fixture("zhipu", "cn");
        row.api_key = "sk-real".into();
        row.required_headers.insert("anthropic-version".into(), "2023-06-01".into());
        row.required_headers.insert("x-relay-key".into(), "relay-secret".into());

        let (exp, secrets) = subscription_to_export(&row);
        assert!(exp.has_api_key);
        assert_eq!(exp.required_headers.get("anthropic-version").map(String::as_str), Some("2023-06-01"));
        assert!(!exp.required_headers.contains_key("x-relay-key"));
        assert_eq!(exp.redacted_headers, vec!["x-relay-key".to_string()]);
        assert_eq!(secrets.api_key.as_deref(), Some("sk-real"));
        assert_eq!(secrets.headers.get("x-relay-key").map(String::as_str), Some("relay-secret"));

        let json = serde_json::to_string(&exp).unwrap();
        assert!(!json.contains("sk-real") && !json.contains("relay-secret"), "明文部分不许带密钥: {json}");
    }

    #[test]
    fn empty_key_is_not_reported_as_having_one() {
        let mut row = SubscriptionRow::test_fixture("custom", "custom");
        row.api_key = String::new();
        let (exp, secrets) = subscription_to_export(&row);
        assert!(!exp.has_api_key);
        assert!(secrets.api_key.is_none());
    }

    #[test]
    fn oauth_metadata_never_leaves_the_machine() {
        let mut row = SubscriptionRow::test_fixture("openai_codex", "default");
        row.auth_type = AuthType::ChatgptOauth;
        row.api_key = String::new();
        row.oauth_metadata.refresh_token = "rt-never-export".into();
        row.oauth_metadata.account_id = "acct-1".into();
        let (exp, secrets) = subscription_to_export(&row);
        let json = serde_json::to_string(&exp).unwrap();
        assert!(!json.contains("rt-never-export") && !json.contains("acct-1"), "{json}");
        assert!(exp.is_oauth());
        assert!(!exp.has_api_key);
        assert!(secrets.api_key.is_none());
    }

    #[test]
    fn oauth_row_with_leftover_api_key_is_not_reported_as_having_one() {
        let mut row = SubscriptionRow::test_fixture("openai_codex", "default");
        row.auth_type = AuthType::ChatgptOauth;
        row.api_key = "sk-leftover-must-not-export".into();
        let (exp, secrets) = subscription_to_export(&row);
        assert!(!exp.has_api_key);
        assert!(secrets.api_key.is_none());
        let json = serde_json::to_string(&exp).unwrap();
        assert!(!json.contains("sk-leftover-must-not-export"), "{json}");
    }

    #[test]
    fn row_roundtrips_through_export_entry() {
        let mut row = SubscriptionRow::test_fixture("deepseek", "default");
        row.base_url = "https://api.deepseek.com/anthropic".into();
        row.display_name = "主号".into();
        row.enabled = false;
        row.forward_client_headers = true;
        row.is_auth_failed = true;
        row.last_error_message = Some("boom".into());
        let (exp, secrets) = subscription_to_export(&row);
        let now = Utc::now();
        let back = export_to_row(&exp, secrets.api_key.unwrap(), row.required_headers.clone(), exp.enabled, now);

        assert_eq!(back.id, row.id);
        assert_eq!(back.display_name, row.display_name);
        assert_eq!(back.api_key, row.api_key);
        assert_eq!(back.base_url, row.base_url);
        assert_eq!(back.enabled, row.enabled);
        assert_eq!(back.forward_client_headers, row.forward_client_headers);
        assert_eq!(back.created_at.timestamp_millis(), row.created_at.timestamp_millis());
        assert_eq!(back.updated_at, now);
        assert!(!back.is_auth_failed, "运行状态不随文件迁移");
        assert!(back.last_error_message.is_none());
        assert_eq!(
            serde_json::to_value(&back.model_discovery).unwrap(),
            serde_json::to_value(&row.model_discovery).unwrap()
        );
    }
}
