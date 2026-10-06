//! Import: preview, secret binding checks, and a pure `plan`; `apply` (Task 6) writes it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use super::format::{export_to_row, header_field, ExportFile, ExportSubscription, SecretsPlain, FIELD_API_KEY};
use crate::commands::subscriptions::{
    validate_base_url, validate_gemini_messages_path, validate_messages_path,
    validate_required_headers, validate_slot_efforts, validate_token_quotas,
};
use crate::error::{AppError, AppResult};
use crate::provider::model::{AuthType, EndpointProtocol, Provider};
use crate::subscription::model::SubscriptionRow;
use crate::subscription::snapshot::{snapshot_connection, ConnectionSnapshot};
use crate::virtual_model::model::{RoutingMode, VirtualModelName};

pub struct LocalState {
    pub existing_ids: HashSet<Uuid>,
    pub bindings: HashMap<VirtualModelName, Vec<Uuid>>,
    /// Built-in provider registry: built-in subscriptions get their connection re-snapshotted
    /// from it instead of trusting the file (see [`rebuilt_connection`]).
    pub providers: Arc<HashMap<String, Provider>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewStatus {
    New,
    SkipExistingId,
    SkipOauth,
    /// Fails the same checks create/update apply (the plaintext part is untrusted).
    SkipInvalid,
}

#[derive(Debug, Serialize)]
pub struct PreviewItem {
    pub id: String,
    pub display_name: String,
    pub provider_id: String,
    pub provider_display_name: String,
    pub provider_icon: String,
    pub status: PreviewStatus,
    pub has_api_key: bool,
    pub redacted_headers: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invalid_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ImportPreview {
    pub app_version: String,
    pub exported_at: i64,
    pub has_secrets: bool,
    pub local_subscription_count: usize,
    pub subscriptions: Vec<PreviewItem>,
}

/// Re-runs the checks `create_subscription` / `update_subscription` apply, so an edited file
/// cannot create a row the UI could never create. Deliberately independent of secrets: preview
/// (no password) and apply must reach the same verdict. Redacted headers are checked by name only
/// (their value is either encrypted or written back blank on purpose).
fn validate_entry(sub: &ExportSubscription) -> Result<(), String> {
    fn check(sub: &ExportSubscription) -> AppResult<()> {
        validate_base_url(&sub.base_url)?;
        validate_messages_path(&sub.messages_path)?;
        if let Some(u) = sub.model_discovery.url.as_deref().filter(|u| !u.trim().is_empty()) {
            validate_base_url(u)?;
        }
        if let Some(b) = &sub.balance_discovery {
            validate_base_url(&b.url)?;
        }
        if sub.auth_type == AuthType::GeminiApiKey {
            validate_gemini_messages_path(&sub.messages_path)?;
        }
        let mut headers = sub.required_headers.clone();
        for name in &sub.redacted_headers {
            headers.entry(name.clone()).or_insert_with(|| "redacted".into());
        }
        validate_required_headers(&headers, &sub.auth_header_name)?;
        validate_slot_efforts(&sub.slot_efforts)?;
        validate_token_quotas(&sub.token_quotas)?;
        // 与 provider 加载期的约束一致: System One 只走透传, 其它鉴权类型没有对应 dispatch。
        if sub.endpoint_protocol == EndpointProtocol::Systemone && sub.auth_type != AuthType::ApiKey {
            return Err(AppError::BadRequest("System One 端点只支持 api_key 鉴权".into()));
        }
        Ok(())
    }
    check(sub).map_err(|e| match e {
        AppError::BadRequest(msg) => msg,
        other => other.to_string(),
    })
}

/// Built-in subscriptions' connection info only ever comes from the provider yaml
/// (`update_subscription` rejects connection patches for them), so re-snapshotting it from the
/// local registry is both more trustworthy than the (untrusted) file and restores the template
/// headers a password-less export redacted (e.g. `cf-aig-authorization: Bearer {api_key}`, whose
/// name looks sensitive but whose value is public). `None` = keep the file's values: user-defined
/// subscription, provider / endpoint gone, url_params no longer valid, or the yaml changed the
/// auth type / endpoint protocol underneath (binding isolation relies on the file's protocol).
fn rebuilt_connection(
    sub: &ExportSubscription,
    providers: &HashMap<String, Provider>,
) -> Option<ConnectionSnapshot> {
    if sub.is_user_defined {
        return None;
    }
    let provider = providers.get(&sub.provider_id)?;
    if provider.auth.auth_type != sub.auth_type {
        return None;
    }
    let endpoint = provider.endpoint(&sub.endpoint_id)?;
    let snap = snapshot_connection(provider, endpoint, &sub.url_params).ok()?;
    (snap.endpoint_protocol == sub.endpoint_protocol).then_some(snap)
}

enum Verdict {
    New,
    SkipExistingId,
    SkipOauth,
    SkipInvalid(String),
}

/// Shared by `preview` and `plan` so the preview and the actual import cannot disagree.
fn verdict_of(sub: &ExportSubscription, local: &LocalState) -> Verdict {
    if local.existing_ids.contains(&sub.id) {
        Verdict::SkipExistingId
    } else if sub.is_oauth() {
        Verdict::SkipOauth
    } else if let Err(reason) = validate_entry(sub) {
        Verdict::SkipInvalid(reason)
    } else {
        Verdict::New
    }
}

pub fn preview(file: &ExportFile, local: &LocalState) -> ImportPreview {
    ImportPreview {
        app_version: file.app_version.clone(),
        exported_at: file.exported_at.timestamp_millis(),
        has_secrets: file.secrets.is_some(),
        local_subscription_count: local.existing_ids.len(),
        subscriptions: file
            .subscriptions
            .iter()
            .map(|s| {
                let (status, invalid_reason) = match verdict_of(s, local) {
                    Verdict::New => (PreviewStatus::New, None),
                    Verdict::SkipExistingId => (PreviewStatus::SkipExistingId, None),
                    Verdict::SkipOauth => (PreviewStatus::SkipOauth, None),
                    Verdict::SkipInvalid(reason) => (PreviewStatus::SkipInvalid, Some(reason)),
                };
                PreviewItem {
                    id: s.id.to_string(),
                    display_name: s.display_name.clone(),
                    provider_id: s.provider_id.clone(),
                    provider_display_name: s.provider_display_name.clone(),
                    provider_icon: s.provider_icon.clone(),
                    status,
                    has_api_key: s.has_api_key,
                    // Re-snapshotted subscriptions get every header back from the registry.
                    redacted_headers: if rebuilt_connection(s, &local.providers).is_some() {
                        Vec::new()
                    } else {
                        s.redacted_headers.clone()
                    },
                    invalid_reason,
                }
            })
            .collect(),
    }
}

#[derive(Debug, Default)]
pub struct ResolvedSecrets {
    pub auth_token: Option<String>,
    pub api_keys: HashMap<Uuid, String>,
    pub headers: HashMap<Uuid, BTreeMap<String, String>>,
}

/// Every referenced secret must be bound to the same subscription, field and destinations as
/// the (untrusted) plaintext part claims; each ref is consumed once so refs cannot be shared.
pub fn resolve_secrets(file: &ExportFile, plain: SecretsPlain) -> AppResult<ResolvedSecrets> {
    let tampered = || AppError::BadRequest("文件被修改过, 已拒绝导入密钥".into());
    let SecretsPlain { auth_token, mut items } = plain;
    let mut out = ResolvedSecrets {
        auth_token: auth_token.filter(|t| !t.is_empty()),
        ..Default::default()
    };
    for sub in &file.subscriptions {
        let Some(refs) = &sub.secret_refs else { continue };
        let destinations = sub.key_destinations();
        let mut take = |r: &str, field: String| -> AppResult<String> {
            let item = items.remove(r).ok_or_else(tampered)?;
            let b = &item.bind;
            if b.subscription_id != sub.id || b.field != field || b.destinations != destinations {
                return Err(tampered());
            }
            Ok(item.value)
        };
        if let Some(r) = &refs.api_key {
            let v = take(r, FIELD_API_KEY.into())?;
            out.api_keys.insert(sub.id, v);
        }
        for (name, r) in &refs.headers {
            let v = take(r, header_field(name))?;
            out.headers.entry(sub.id).or_default().insert(name.clone(), v);
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InvalidEntry {
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct ImportReport {
    pub imported: usize,
    pub skipped_existing: usize,
    pub skipped_oauth: Vec<String>,
    pub skipped_invalid: Vec<InvalidEntry>,
    pub disabled_missing_key: Vec<String>,
    pub token_imported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_error: Option<String>,
    /// 跨类绑定 (对话订阅 ↔ model-jev) 被跳过的记录: name = 订阅名, reason = 原因。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_bindings: Vec<InvalidEntry>,
}

#[derive(Debug)]
pub struct ImportPlan {
    pub inserts: Vec<SubscriptionRow>,
    /// Full new binding list for each virtual model that changes.
    pub bindings: Vec<(VirtualModelName, Vec<Uuid>)>,
    pub modes: Vec<(VirtualModelName, RoutingMode)>,
    pub report: ImportReport,
}

pub fn plan(
    file: &ExportFile,
    secrets: Option<&ResolvedSecrets>,
    local: &LocalState,
    now: DateTime<Utc>,
) -> ImportPlan {
    let mut report = ImportReport::default();
    let mut inserts = Vec::new();
    let mut new_ids = HashSet::new();

    for sub in &file.subscriptions {
        match verdict_of(sub, local) {
            Verdict::SkipExistingId => {
                report.skipped_existing += 1;
                continue;
            }
            Verdict::SkipOauth => {
                report.skipped_oauth.push(sub.display_name.clone());
                continue;
            }
            Verdict::SkipInvalid(reason) => {
                report.skipped_invalid.push(InvalidEntry { name: sub.display_name.clone(), reason });
                continue;
            }
            Verdict::New => {}
        }
        let api_key = secrets.and_then(|s| s.api_keys.get(&sub.id)).cloned();
        let mut missing = sub.has_api_key && api_key.is_none();
        let rebuilt = rebuilt_connection(sub, &local.providers);
        let headers = match &rebuilt {
            Some(snap) => snap.required_headers.clone(),
            None => {
                let recovered = secrets.and_then(|s| s.headers.get(&sub.id));
                let mut headers = sub.required_headers.clone();
                for name in &sub.redacted_headers {
                    match recovered.and_then(|h| h.get(name)) {
                        Some(v) => {
                            headers.insert(name.clone(), v.clone());
                        }
                        None => {
                            // Keep the header name so the edit page shows what still needs filling in.
                            headers.insert(name.clone(), String::new());
                            missing = true;
                        }
                    }
                }
                headers
            }
        };
        if missing {
            report.disabled_missing_key.push(sub.display_name.clone());
        }
        let enabled = sub.enabled && !missing;
        let mut row = export_to_row(sub, api_key.unwrap_or_default(), headers, enabled, now);
        if let Some(snap) = rebuilt {
            row.base_url = snap.base_url;
            row.messages_path = snap.messages_path;
            row.model_discovery = snap.model_discovery;
            row.systemone_wire = snap.systemone_wire;
            row.url_params = snap.url_params;
        }
        inserts.push(row);
        new_ids.insert(sub.id);
    }
    report.imported = inserts.len();

    // 订阅 id -> (端点协议快照, 名称): 绑定时按虚拟模型隔离对话类 / systemone 订阅。
    let protocol_of: HashMap<Uuid, (crate::provider::model::EndpointProtocol, &str)> = file
        .subscriptions
        .iter()
        .map(|s| (s.id, (s.endpoint_protocol, s.display_name.as_str())))
        .collect();

    let mut bindings = Vec::new();
    let mut modes = Vec::new();
    for vm in &file.virtual_models {
        let current = local.bindings.get(&vm.name).cloned().unwrap_or_default();
        let mut next = current.clone();
        for id in &vm.subscription_ids {
            if !new_ids.contains(id) || next.contains(id) {
                continue;
            }
            if let Some((protocol, name)) = protocol_of.get(id) {
                if let Some(reason) = vm.name.binding_rejection(*protocol, name) {
                    report.skipped_bindings.push(InvalidEntry { name: name.to_string(), reason });
                    continue;
                }
            }
            next.push(*id);
        }
        if next != current {
            if current.is_empty() {
                modes.push((vm.name, vm.mode));
            }
            bindings.push((vm.name, next));
        }
    }

    ImportPlan { inserts, bindings, modes, report }
}

pub async fn apply(pool: &sqlx::SqlitePool, plan: &ImportPlan) -> AppResult<()> {
    use crate::subscription::store as sub_store;
    use crate::virtual_model::store as vm_store;

    let mut tx = pool.begin().await?;
    for row in &plan.inserts {
        sub_store::insert_on(&mut tx, row).await?;
    }
    for (name, ids) in &plan.bindings {
        vm_store::save_bindings_on(&mut tx, *name, ids).await?;
    }
    for (name, mode) in &plan.modes {
        vm_store::save_mode_on(&mut tx, *name, *mode).await?;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::crypto::{open, KdfCost};
    use crate::backup::export::{build_export, SecretOptions, VirtualModelSnapshot};
    use crate::db::run_migrations;
    use crate::provider::model::AuthType;
    use crate::subscription::model::SubscriptionRow;
    use sqlx::sqlite::SqlitePoolOptions;

    const PW: &str = "0123456789";

    struct Fixture {
        rows: Vec<SubscriptionRow>,
        file: ExportFile,
    }

    /// rows[0]: 有 Key + 敏感头, rows[1]: 本来没 Key (本地模型), rows[2]: ChatGPT OAuth
    /// 连接信息必须能过导入校验 (test_fixture 默认是空串)。
    fn fixture(with_secrets: bool) -> Fixture {
        let mut a = SubscriptionRow::test_fixture("zhipu", "cn");
        a.display_name = "A".into();
        a.base_url = "https://a.example".into();
        a.messages_path = "/v1/messages".into();
        a.api_key = "sk-a".into();
        a.required_headers.insert("x-relay-key".into(), "hk".into());
        let mut b = SubscriptionRow::test_fixture("custom", "custom");
        b.display_name = "B".into();
        b.base_url = "http://127.0.0.1:11434".into();
        b.messages_path = "/v1/messages".into();
        b.api_key = String::new();
        let mut c = SubscriptionRow::test_fixture("openai_codex", "default");
        c.display_name = "C".into();
        c.base_url = "https://chatgpt.com/backend-api/codex".into();
        c.messages_path = "/responses".into();
        c.auth_type = AuthType::ChatgptOauth;
        c.api_key = String::new();
        let rows = vec![a, b, c];
        let vms = vec![VirtualModelSnapshot {
            name: VirtualModelName::Opus,
            mode: RoutingMode::Sticky,
            subscription_ids: rows.iter().map(|r| r.id).collect(),
        }];
        let secrets = with_secrets.then(|| SecretOptions { password: PW, auth_token: "tok", cost: KdfCost::FAST });
        let file = build_export(&rows, &vms, secrets, "6.1.0", Utc::now()).unwrap();
        Fixture { rows, file }
    }

    fn empty_local() -> LocalState {
        LocalState { existing_ids: HashSet::new(), bindings: HashMap::new(), providers: Arc::new(HashMap::new()) }
    }

    fn resolved(f: &Fixture) -> ResolvedSecrets {
        let plain = open(f.file.secrets.as_ref().unwrap(), PW).unwrap();
        resolve_secrets(&f.file, plain).unwrap()
    }

    #[test]
    fn preview_marks_statuses() {
        let f = fixture(false);
        let mut local = empty_local();
        local.existing_ids.insert(f.rows[1].id);
        let p = preview(&f.file, &local);
        let status = |id: Uuid| p.subscriptions.iter().find(|s| s.id == id.to_string()).unwrap().status;
        assert_eq!(status(f.rows[0].id), PreviewStatus::New);
        assert_eq!(status(f.rows[1].id), PreviewStatus::SkipExistingId);
        assert_eq!(status(f.rows[2].id), PreviewStatus::SkipOauth);
        assert!(!p.has_secrets);
        assert_eq!(p.local_subscription_count, 1);
    }

    #[test]
    fn full_import_with_secrets() {
        let f = fixture(true);
        let s = resolved(&f);
        assert_eq!(s.auth_token.as_deref(), Some("tok"));
        let plan = plan(&f.file, Some(&s), &empty_local(), Utc::now());
        assert_eq!(plan.report.imported, 2);
        assert_eq!(plan.report.skipped_oauth, vec!["C".to_string()]);
        assert!(plan.report.disabled_missing_key.is_empty());
        let a = plan.inserts.iter().find(|r| r.id == f.rows[0].id).unwrap();
        assert_eq!(a.api_key, "sk-a");
        assert_eq!(a.required_headers.get("x-relay-key").map(String::as_str), Some("hk"));
        assert!(a.enabled);
        assert_eq!(plan.bindings, vec![(VirtualModelName::Opus, vec![f.rows[0].id, f.rows[1].id])], "OAuth 不进绑定");
        assert_eq!(plan.modes, vec![(VirtualModelName::Opus, RoutingMode::Sticky)], "本机为空时采用文件的模式");
    }

    #[test]
    fn without_secrets_keyed_subscriptions_are_disabled_but_keyless_ones_are_not() {
        let f = fixture(false);
        let plan = plan(&f.file, None, &empty_local(), Utc::now());
        assert_eq!(plan.report.disabled_missing_key, vec!["A".to_string()]);
        let a = plan.inserts.iter().find(|r| r.id == f.rows[0].id).unwrap();
        assert!(!a.enabled);
        assert_eq!(a.api_key, "");
        assert_eq!(a.required_headers.get("x-relay-key").map(String::as_str), Some(""), "保留头名, 值置空");
        let b = plan.inserts.iter().find(|r| r.id == f.rows[1].id).unwrap();
        assert!(b.enabled, "本来就没 Key 的订阅保持原启停状态");
    }

    /// A Cloudflare gateway subscription built exactly like create_subscription does.
    fn cloudflare_gateway_row(providers: &HashMap<String, Provider>) -> SubscriptionRow {
        let provider = &providers["cloudflare_clef"];
        let params: BTreeMap<String, String> = [
            ("account_id".to_string(), "0123456789abcdef0123456789abcdef".to_string()),
            ("gateway_id".to_string(), "gw-1".to_string()),
        ]
        .into();
        let snap = crate::subscription::snapshot::snapshot_connection(provider, provider.endpoint("gateway").unwrap(), &params)
            .unwrap();
        let mut row = SubscriptionRow::test_fixture("cloudflare_clef", "gateway");
        row.display_name = "CF".into();
        row.api_key = "cf-token".into();
        row.auth_header_name = "Authorization".into();
        row.base_url = snap.base_url;
        row.messages_path = snap.messages_path;
        row.required_headers = snap.required_headers;
        row.model_discovery = snap.model_discovery;
        row.endpoint_protocol = snap.endpoint_protocol;
        row.systemone_wire = snap.systemone_wire;
        row.url_params = snap.url_params;
        row
    }

    #[test]
    fn builtin_subscription_without_secrets_gets_template_headers_back_from_the_registry() {
        let providers = Arc::new(crate::provider::loader::load_all().unwrap());
        let row = cloudflare_gateway_row(&providers);
        let file = build_export(std::slice::from_ref(&row), &[], None, "6.1.0", Utc::now()).unwrap();
        let exported = &file.subscriptions[0];
        assert!(exported.redacted_headers.contains(&"cf-aig-authorization".to_string()), "前提: 无密码导出时模板头被当成敏感头去掉");

        let local = LocalState { providers: providers.clone(), ..empty_local() };
        let plan = plan(&file, None, &local, Utc::now());
        let got = &plan.inserts[0];
        assert_eq!(got.required_headers.get("cf-aig-authorization").map(String::as_str), Some("Bearer {api_key}"));
        assert_eq!(got.required_headers.get("cf-aig-gateway-id").map(String::as_str), Some("gw-1"));
        assert_eq!(got.required_headers, row.required_headers);
        assert_eq!(got.base_url, row.base_url);
        assert_eq!(got.messages_path, row.messages_path);
        assert_eq!(got.systemone_wire, row.systemone_wire);
        assert_eq!(got.url_params, row.url_params);
        assert!(!got.enabled, "API Key 仍然缺失, 照旧停用");
        assert_eq!(plan.report.disabled_missing_key, vec!["CF".to_string()]);

        let preview = preview(&file, &local);
        assert!(preview.subscriptions[0].redacted_headers.is_empty(), "注册表能补回的头不再列为需要重填");
    }

    #[test]
    fn builtin_subscription_connection_comes_from_the_registry_not_the_file() {
        let providers = Arc::new(crate::provider::loader::load_all().unwrap());
        let row = cloudflare_gateway_row(&providers);
        let mut file = build_export(std::slice::from_ref(&row), &[], None, "6.1.0", Utc::now()).unwrap();
        file.subscriptions[0].base_url = "https://evil.example/accounts/x/ai".into();
        file.subscriptions[0].required_headers.insert("cf-aig-gateway-id".into(), "other".into());
        let local = LocalState { providers, ..empty_local() };
        let got = &plan(&file, None, &local, Utc::now()).inserts[0];
        assert_eq!(got.base_url, row.base_url);
        assert_eq!(got.required_headers.get("cf-aig-gateway-id").map(String::as_str), Some("gw-1"));
    }

    #[test]
    fn unknown_provider_keeps_the_file_values() {
        // The registry is empty here: same behaviour as before (redacted header kept blank).
        let providers = crate::provider::loader::load_all().unwrap();
        let row = cloudflare_gateway_row(&providers);
        let file = build_export(std::slice::from_ref(&row), &[], None, "6.1.0", Utc::now()).unwrap();
        let got = &plan(&file, None, &empty_local(), Utc::now()).inserts[0];
        assert_eq!(got.required_headers.get("cf-aig-authorization").map(String::as_str), Some(""));
        assert_eq!(got.required_headers.get("cf-aig-gateway-id").map(String::as_str), Some("gw-1"));
    }

    #[test]
    fn existing_ids_are_skipped_and_bindings_only_append_new_ones() {
        let f = fixture(false);
        let other = Uuid::new_v4();
        let mut local = empty_local();
        local.existing_ids.insert(f.rows[1].id);
        local.existing_ids.insert(other);
        local.bindings.insert(VirtualModelName::Opus, vec![other]);
        let plan = plan(&f.file, None, &local, Utc::now());
        assert_eq!(plan.report.skipped_existing, 1);
        assert_eq!(plan.report.imported, 1);
        assert_eq!(plan.bindings, vec![(VirtualModelName::Opus, vec![other, f.rows[0].id])]);
        assert!(plan.modes.is_empty(), "本机已有绑定时不改模式");
    }

    #[test]
    fn reimporting_the_same_file_changes_nothing() {
        let f = fixture(false);
        let mut local = empty_local();
        for r in &f.rows {
            local.existing_ids.insert(r.id);
        }
        local.bindings.insert(VirtualModelName::Opus, vec![f.rows[0].id, f.rows[1].id]);
        let plan = plan(&f.file, None, &local, Utc::now());
        assert!(plan.inserts.is_empty());
        assert!(plan.bindings.is_empty());
        assert!(plan.modes.is_empty());
    }

    #[test]
    fn tampered_destination_is_rejected() {
        let mut f = fixture(true);
        let plain = open(f.file.secrets.as_ref().unwrap(), PW).unwrap();
        let a = f.file.subscriptions.iter_mut().find(|s| s.display_name == "A").unwrap();
        a.base_url = "https://evil.example".into();
        assert!(resolve_secrets(&f.file, plain).unwrap_err().to_string().contains("被修改过"));
    }

    #[test]
    fn tampered_model_discovery_url_is_rejected() {
        let mut f = fixture(true);
        let plain = open(f.file.secrets.as_ref().unwrap(), PW).unwrap();
        let a = f.file.subscriptions.iter_mut().find(|s| s.display_name == "A").unwrap();
        a.model_discovery.url = Some("https://evil.example/models".into());
        assert!(resolve_secrets(&f.file, plain).is_err());
    }

    #[test]
    fn moving_a_secret_ref_to_another_subscription_is_rejected() {
        let mut f = fixture(true);
        let plain = open(f.file.secrets.as_ref().unwrap(), PW).unwrap();
        let refs = f.file.subscriptions.iter().find(|s| s.display_name == "A").unwrap().secret_refs.clone();
        f.file.subscriptions.iter_mut().find(|s| s.display_name == "A").unwrap().secret_refs = None;
        let b = f.file.subscriptions.iter_mut().find(|s| s.display_name == "B").unwrap();
        b.base_url = "https://a.example".into(); // same destination, different subscription id
        b.secret_refs = refs;
        assert!(resolve_secrets(&f.file, plain).unwrap_err().to_string().contains("被修改过"));
    }

    #[test]
    fn shared_secret_ref_is_rejected() {
        let mut f = fixture(true);
        let plain = open(f.file.secrets.as_ref().unwrap(), PW).unwrap();
        let refs = f.file.subscriptions.iter().find(|s| s.display_name == "A").unwrap().secret_refs.clone();
        let b = f.file.subscriptions.iter_mut().find(|s| s.display_name == "B").unwrap();
        b.base_url = "https://a.example".into(); // same destination, different subscription id
        b.secret_refs = refs;
        // A keeps its refs, B has a clone pointing to A's secret items; second to process will fail on consume
        assert!(resolve_secrets(&f.file, plain).unwrap_err().to_string().contains("被修改过"));
    }

    async fn memory_pool() -> sqlx::SqlitePool {
        let pool = SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        run_migrations(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn export_then_import_into_a_fresh_database_restores_rows() {
        use crate::backup::format::parse_file;
        use crate::subscription::store::{insert, load_runtime};
        use crate::virtual_model::store as vm_store;

        let src = memory_pool().await;
        let f = fixture(true);
        for r in &f.rows {
            insert(&src, r).await.unwrap();
        }
        let text = serde_json::to_string_pretty(&f.file).unwrap();

        let dst = memory_pool().await;
        let file = parse_file(&text).unwrap();
        let plain = open(file.secrets.as_ref().unwrap(), PW).unwrap();
        let secrets = resolve_secrets(&file, plain).unwrap();
        let plan = plan(&file, Some(&secrets), &empty_local(), Utc::now());
        apply(&dst, &plan).await.unwrap();

        let loaded = load_runtime(&dst).await.unwrap();
        assert_eq!(loaded.len(), 2, "OAuth 那条不导入");
        let a = loaded[&f.rows[0].id].read().await.row.clone();
        let orig = &f.rows[0];
        assert_eq!(a.api_key, orig.api_key);
        assert_eq!(a.display_name, orig.display_name);
        assert_eq!(a.base_url, orig.base_url);
        assert_eq!(a.required_headers, orig.required_headers);
        assert_eq!(a.enabled, orig.enabled);
        assert_eq!(a.created_at.timestamp_millis(), orig.created_at.timestamp_millis());
        assert_eq!(serde_json::to_value(&a.model_slots).unwrap(), serde_json::to_value(&orig.model_slots).unwrap());

        let vms = vm_store::load_all(&dst).await.unwrap();
        assert_eq!(vms[&VirtualModelName::Opus].subscription_ids, vec![f.rows[0].id, f.rows[1].id]);
        assert_eq!(vms[&VirtualModelName::Opus].mode, RoutingMode::Sticky);
    }

    #[tokio::test]
    async fn failure_midway_rolls_everything_back() {
        use crate::subscription::store::load_runtime;
        let dst = memory_pool().await;
        let f = fixture(false);
        let mut plan = plan(&f.file, None, &empty_local(), Utc::now());
        let dup = plan.inserts[0].clone();
        plan.inserts.push(dup); // second insert hits the primary key
        assert!(apply(&dst, &plan).await.is_err());
        assert!(load_runtime(&dst).await.unwrap().is_empty(), "整体回滚");
    }

    fn find_mut<'a>(f: &'a mut Fixture, name: &str) -> &'a mut ExportSubscription {
        f.file.subscriptions.iter_mut().find(|s| s.display_name == name).unwrap()
    }

    #[test]
    fn invalid_entries_are_skipped_and_reported_with_a_reason() {
        let mut f = fixture(false);
        find_mut(&mut f, "A").messages_path = "v1/messages".into();

        let p = preview(&f.file, &empty_local());
        let a = p.subscriptions.iter().find(|s| s.display_name == "A").unwrap();
        assert_eq!(a.status, PreviewStatus::SkipInvalid);
        assert!(a.invalid_reason.as_deref().unwrap().contains("messages_path"), "{:?}", a.invalid_reason);

        let plan = plan(&f.file, None, &empty_local(), Utc::now());
        assert!(plan.inserts.iter().all(|r| r.display_name != "A"));
        assert_eq!(plan.report.skipped_invalid.len(), 1);
        assert_eq!(plan.report.skipped_invalid[0].name, "A");
        assert!(plan.report.skipped_invalid[0].reason.contains("messages_path"));
        assert!(
            plan.bindings.iter().all(|(_, ids)| !ids.contains(&f.rows[0].id)),
            "skipped entries never enter bindings"
        );
    }

    #[test]
    fn existing_id_wins_over_invalid() {
        let mut f = fixture(false);
        find_mut(&mut f, "A").base_url = "ftp://a.example".into();
        let mut local = empty_local();
        local.existing_ids.insert(f.rows[0].id);
        let plan = plan(&f.file, None, &local, Utc::now());
        assert_eq!(plan.report.skipped_existing, 1);
        assert!(plan.report.skipped_invalid.is_empty());
    }

    #[test]
    fn each_create_time_rule_is_enforced() {
        type Mutate = fn(&mut ExportSubscription);
        let cases: [(&str, Mutate); 8] = [
            ("base_url", |s| s.base_url = "a.example".into()),
            ("messages_path", |s| s.messages_path = "no-slash".into()),
            ("models url", |s| s.model_discovery.url = Some("file:///etc/passwd".into())),
            ("reserved header", |s| {
                s.required_headers.insert("Host".into(), "x".into());
            }),
            ("auth header clash", |s| {
                s.auth_header_name = "X-Custom-Auth".into();
                s.required_headers.insert("x-custom-auth".into(), "x".into());
            }),
            ("gemini {model}", |s| {
                s.auth_type = AuthType::GeminiApiKey;
                s.messages_path = "/v1beta/models/gemini:generateContent".into();
            }),
            ("slot effort", |s| s.slot_efforts.opus = Some("turbo".into())),
            ("zero quota", |s| s.token_quotas.daily = Some(0)),
        ];
        for (what, mutate) in cases {
            let mut f = fixture(false);
            mutate(find_mut(&mut f, "B"));
            let plan = plan(&f.file, None, &empty_local(), Utc::now());
            assert_eq!(
                plan.report.skipped_invalid.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
                vec!["B"],
                "{what} should make B invalid"
            );
        }
    }

    #[test]
    fn blank_placeholder_for_a_redacted_header_is_not_an_empty_value_error() {
        // A's x-relay-key is redacted; without secrets it is written back as "".
        let f = fixture(false);
        let plan = plan(&f.file, None, &empty_local(), Utc::now());
        assert!(plan.report.skipped_invalid.is_empty(), "{:?}", plan.report.skipped_invalid);
        assert!(plan.inserts.iter().any(|r| r.display_name == "A"));
    }

    /// Every built-in provider endpoint, snapshotted the way create_subscription does it, must
    /// pass import validation — otherwise a plain backup of a built-in subscription would be skipped.
    #[test]
    fn every_builtin_provider_endpoint_passes_import_validation() {
        use crate::backup::format::subscription_to_export;
        let providers = crate::provider::loader::load_all().unwrap();
        for p in providers.values() {
            // Sample value satisfies every declared url_param pattern (cloudflare's two).
            let sample: std::collections::BTreeMap<String, String> = p
                .url_params
                .iter()
                .map(|u| (u.id.clone(), "0123456789abcdef0123456789abcdef".to_string()))
                .collect();
            for ep in &p.endpoints {
                let mut row = SubscriptionRow::test_fixture(&p.id, &ep.id);
                row.auth_type = p.auth.auth_type;
                row.base_url = crate::provider::url_template::resolve(&ep.base_url, &sample);
                row.messages_path = ep.messages_path.clone();
                row.auth_header_name = p.auth.header_name.clone();
                row.auth_header_format = p.auth.header_format.clone();
                row.required_headers = p.required_headers.clone();
                row.forward_headers = p.forward_headers.clone();
                row.model_discovery = p.model_discovery.clone();
                row.model_discovery.url = row
                    .model_discovery
                    .url
                    .as_deref()
                    .map(|u| crate::provider::url_template::resolve(u, &sample));
                row.balance_discovery = p.balance_discovery.clone();
                let (exp, _) = subscription_to_export(&row);
                if let Err(reason) = validate_entry(&exp) {
                    panic!("{}/{} rejected: {reason}", p.id, ep.id);
                }
            }
        }
    }

    /// System One 端点只支持 api_key 鉴权: 手改过的文件里 systemone + 其它鉴权类型整条跳过。
    #[test]
    fn systemone_with_non_api_key_auth_is_skipped_invalid() {
        let mut f = fixture(false);
        {
            let a = find_mut(&mut f, "A");
            a.endpoint_protocol = EndpointProtocol::Systemone;
            a.auth_type = AuthType::OpenaiResponsesApiKey;
        }
        let p = preview(&f.file, &empty_local());
        let a = p.subscriptions.iter().find(|s| s.display_name == "A").unwrap();
        assert_eq!(a.status, PreviewStatus::SkipInvalid);
        assert!(a.invalid_reason.as_deref().unwrap().contains("api_key"), "{:?}", a.invalid_reason);
        let plan = plan(&f.file, None, &empty_local(), Utc::now());
        assert!(plan.inserts.iter().all(|r| r.display_name != "A"));
        assert_eq!(plan.report.skipped_invalid[0].name, "A");
    }

    /// 文件里的跨类绑定 (对话订阅绑到 model-jev / systemone 订阅绑到 model-opus) 跳过并说明,
    /// 订阅本身照常导入。
    #[test]
    fn cross_kind_bindings_are_skipped_but_subscriptions_imported() {
        use crate::provider::model::EndpointProtocol;
        let mut f = fixture(true);
        let chat_id = f.file.subscriptions[0].id;
        f.file.subscriptions[1].endpoint_protocol = EndpointProtocol::Systemone;
        let jev_id = f.file.subscriptions[1].id;
        f.file.virtual_models.push(crate::backup::format::ExportVirtualModel {
            name: VirtualModelName::Jev,
            mode: RoutingMode::Sequential,
            subscription_ids: vec![chat_id, jev_id],
        });
        let plan = plan(&f.file, Some(&resolved(&f)), &empty_local(), Utc::now());

        let jev = plan.bindings.iter().find(|(n, _)| *n == VirtualModelName::Jev).unwrap();
        assert_eq!(jev.1, vec![jev_id], "对话订阅不能进 model-jev");
        let opus = plan.bindings.iter().find(|(n, _)| *n == VirtualModelName::Opus).unwrap();
        assert!(!opus.1.contains(&jev_id), "systemone 订阅不能进 model-opus");
        assert_eq!(plan.report.skipped_bindings.len(), 2);
        assert!(plan.inserts.iter().any(|r| r.id == jev_id), "订阅本身照常导入");
    }
}
