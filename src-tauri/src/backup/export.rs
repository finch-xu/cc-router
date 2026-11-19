//! Assemble an `ExportFile` from runtime snapshots.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::crypto::{seal, validate_password, KdfCost};
use super::format::{
    header_field, subscription_to_export, ExportFile, ExportVirtualModel, SecretBind, SecretItem,
    SecretRefs, SecretsPlain, FIELD_API_KEY, FORMAT, VERSION,
};
use crate::error::AppResult;
use crate::subscription::model::SubscriptionRow;
use crate::virtual_model::model::{RoutingMode, VirtualModelName};

#[derive(Debug, Clone)]
pub struct VirtualModelSnapshot {
    pub name: VirtualModelName,
    pub mode: RoutingMode,
    pub subscription_ids: Vec<Uuid>,
    pub upstream_models: BTreeMap<Uuid, String>,
}

pub struct SecretOptions<'a> {
    pub password: &'a str,
    pub auth_token: &'a str,
    pub cost: KdfCost,
}

pub fn build_export(
    rows: &[SubscriptionRow],
    vms: &[VirtualModelSnapshot],
    secrets: Option<SecretOptions<'_>>,
    app_version: &str,
    now: DateTime<Utc>,
) -> AppResult<ExportFile> {
    if let Some(o) = &secrets {
        validate_password(o.password)?;
    }
    let mut ordered: Vec<&SubscriptionRow> = rows.iter().collect();
    ordered.sort_by_key(|r| (r.created_at, r.id));

    let mut subscriptions = Vec::with_capacity(ordered.len());
    let mut items = BTreeMap::new();
    let mut counter = 0usize;
    let mut next_ref = || {
        counter += 1;
        format!("s{counter}")
    };
    for row in ordered {
        let (mut exp, extracted) = subscription_to_export(row);
        if secrets.is_some() {
            let destinations = exp.key_destinations();
            let bind = |field: String| SecretBind {
                subscription_id: exp.id,
                field,
                destinations: destinations.clone(),
            };
            let mut refs = SecretRefs::default();
            if let Some(key) = extracted.api_key {
                let r = next_ref();
                items.insert(r.clone(), SecretItem { value: key, bind: bind(FIELD_API_KEY.into()) });
                refs.api_key = Some(r);
            }
            for (name, value) in extracted.headers {
                let r = next_ref();
                items.insert(r.clone(), SecretItem { value, bind: bind(header_field(&name)) });
                refs.headers.insert(name, r);
            }
            if refs.api_key.is_some() || !refs.headers.is_empty() {
                exp.secret_refs = Some(refs);
            }
        }
        subscriptions.push(exp);
    }

    let by_name: HashMap<&VirtualModelName, &VirtualModelSnapshot> =
        vms.iter().map(|v| (&v.name, v)).collect();
    let mut custom_names: Vec<_> = vms.iter().filter(|vm| vm.name.is_custom()).map(|vm| vm.name.clone()).collect();
    custom_names.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    let virtual_models = VirtualModelName::all()
        .into_iter()
        .chain(custom_names)
        .map(|name| match by_name.get(&name) {
            Some(v) => ExportVirtualModel { name, mode: v.mode, subscription_ids: v.subscription_ids.clone(), upstream_models: v.upstream_models.clone() },
            None => ExportVirtualModel { name, mode: RoutingMode::Sequential, subscription_ids: Vec::new(), upstream_models: BTreeMap::new() },
        })
        .filter(|vm: &ExportVirtualModel| !(vm.name.is_jev() && vm.subscription_ids.is_empty()))
        .collect();

    let envelope = match secrets {
        Some(o) => {
            let plain = SecretsPlain {
                auth_token: Some(o.auth_token.to_string()).filter(|t| !t.is_empty()),
                items,
            };
            Some(seal(&plain, o.password, o.cost)?)
        }
        None => None,
    };

    Ok(ExportFile {
        format: FORMAT.into(),
        version: if vms.iter().any(|vm| vm.name.is_custom()) { VERSION } else { 1 },
        app_version: app_version.into(),
        exported_at: now,
        subscriptions,
        virtual_models,
        secrets: envelope,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::crypto::{open, KdfCost};
    use crate::backup::format::{parse_file, MAX_FILE_BYTES, FIELD_API_KEY};

    fn rows() -> Vec<SubscriptionRow> {
        let mut a = SubscriptionRow::test_fixture("zhipu", "cn");
        a.base_url = "https://open.bigmodel.cn/api/anthropic".into();
        a.api_key = "sk-a".into();
        a.required_headers.insert("x-relay-key".into(), "hk".into());
        let mut b = SubscriptionRow::test_fixture("custom", "custom");
        b.api_key = String::new();
        vec![a, b]
    }

    fn vms(rows: &[SubscriptionRow]) -> Vec<VirtualModelSnapshot> {
        vec![VirtualModelSnapshot {
            name: VirtualModelName::Opus,
            mode: RoutingMode::Sticky,
            subscription_ids: vec![rows[1].id, rows[0].id],
            upstream_models: BTreeMap::new(),
        }]
    }

    #[test]
    fn bound_jev_is_exported() {
        let r = rows();
        let mut v = vms(&r);
        v.push(VirtualModelSnapshot {
            name: VirtualModelName::Jev,
            mode: RoutingMode::Sequential,
            subscription_ids: vec![r[0].id],
            upstream_models: BTreeMap::new(),
        });
        let file = build_export(&r, &v, None, "6.1.0", Utc::now()).unwrap();
        assert_eq!(file.virtual_models.len(), 6);
        assert!(file.virtual_models.iter().any(|v| v.name.is_jev()));
    }

    #[test]
    fn plain_export_has_no_secrets_anywhere() {
        let r = rows();
        let file = build_export(&r, &vms(&r), None, "6.1.0", Utc::now()).unwrap();
        assert!(file.secrets.is_none());
        assert!(file.subscriptions.iter().all(|s| s.secret_refs.is_none()));
        let text = serde_json::to_string(&file).unwrap();
        assert!(!text.contains("sk-a") && !text.contains("\"hk\""), "{text}");
        assert_eq!(file.virtual_models.len(), 5, "无绑定的 model-jev 不输出, 其余五个全部输出");
        assert!(file.virtual_models.iter().all(|v| !v.name.is_jev()));
        let opus = file.virtual_models.iter().find(|v| v.name == VirtualModelName::Opus).unwrap();
        assert_eq!(opus.mode, RoutingMode::Sticky);
        assert_eq!(opus.subscription_ids, vec![r[1].id, r[0].id], "绑定顺序保持");
    }

    #[test]
    fn encrypted_export_binds_each_secret_to_its_destinations() {
        let r = rows();
        let secrets = SecretOptions { password: "0123456789", auth_token: "tok-x", cost: KdfCost::FAST };
        let file = build_export(&r, &vms(&r), Some(secrets), "6.1.0", Utc::now()).unwrap();
        let text = serde_json::to_string(&file).unwrap();
        assert!(!text.contains("sk-a") && !text.contains("tok-x"), "明文里不许出现密钥");

        let plain = open(file.secrets.as_ref().unwrap(), "0123456789").unwrap();
        assert_eq!(plain.auth_token.as_deref(), Some("tok-x"));
        let a = file.subscriptions.iter().find(|s| s.id == r[0].id).unwrap();
        let refs = a.secret_refs.as_ref().unwrap();
        let key_item = &plain.items[refs.api_key.as_ref().unwrap()];
        assert_eq!(key_item.value, "sk-a");
        assert_eq!(key_item.bind.subscription_id, a.id);
        assert_eq!(key_item.bind.field, FIELD_API_KEY);
        assert_eq!(key_item.bind.destinations, a.key_destinations());
        assert_eq!(plain.items[&refs.headers["x-relay-key"]].value, "hk");

        let b = file.subscriptions.iter().find(|s| s.id == r[1].id).unwrap();
        assert!(b.secret_refs.is_none(), "本来没 Key 的订阅不产生引用");
    }

    #[test]
    fn short_password_is_rejected() {
        let r = rows();
        let secrets = SecretOptions { password: "short", auth_token: "t", cost: KdfCost::FAST };
        assert!(build_export(&r, &vms(&r), Some(secrets), "6.1.0", Utc::now()).is_err());
    }

    #[test]
    fn exported_text_parses_back() {
        let r = rows();
        let file = build_export(&r, &vms(&r), None, "6.1.0", Utc::now()).unwrap();
        let text = serde_json::to_string_pretty(&file).unwrap();
        let back = parse_file(&text).unwrap();
        assert_eq!(back.subscriptions.len(), 2);
    }

    #[test]
    fn parse_rejects_wrong_format_newer_version_and_duplicates() {
        let r = rows();
        let file = build_export(&r, &vms(&r), None, "6.1.0", Utc::now()).unwrap();
        let mut v = serde_json::to_value(&file).unwrap();

        let mut wrong = v.clone();
        wrong["format"] = "something-else".into();
        assert!(parse_file(&wrong.to_string()).unwrap_err().to_string().contains("不是 cc-router"));

        let mut newer = v.clone();
        newer["version"] = 2.into();
        newer["subscriptions"] = serde_json::json!([{ "totally": "new shape" }]);
        assert!(parse_file(&newer.to_string()).unwrap_err().to_string().contains("请先升级"), "版本太新不能报成字段解析错误");

        let first = v["subscriptions"][0].clone();
        v["subscriptions"].as_array_mut().unwrap().push(first);
        assert!(parse_file(&v.to_string()).unwrap_err().to_string().contains("重复"));

        assert!(parse_file("not json").is_err());
        let huge = "x".repeat(MAX_FILE_BYTES + 1);
        assert!(parse_file(&huge).unwrap_err().to_string().contains("过大"));
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let r = rows();
        let file = build_export(&r, &vms(&r), None, "6.1.0", Utc::now()).unwrap();
        let mut v = serde_json::to_value(&file).unwrap();
        v["future_top_level"] = "x".into();
        v["subscriptions"][0]["future_field"] = 1.into();
        assert!(parse_file(&v.to_string()).is_ok());
    }
}
