use std::collections::BTreeMap;

use serde::Serialize;
use tauri::State;

use crate::error::AppResult;
use crate::provider::model::{Auth, Compatibility, EndpointProtocol, LocalizedText, ModelDiscovery, ProviderCategory};
use crate::provider::Provider;
use crate::state::AppState;

/// 上屏文字字段 (`display_name` / `description` / `compatibility_notes` / 端点的 `label` / `description`)
/// 放中文原文; 英日文在 `translations` 里, 由客户端按自己的界面语言叠加 —— 桌面端、网页界面、TUI
/// 的语言可能各不相同, 后端不替它们选。
#[derive(Debug, Serialize)]
pub struct ProviderInfo {
    pub id: String,
    pub display_name: String,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub docs_url: Option<String>,
    pub api_key_url: Option<String>,
    pub icon: Option<String>,
    pub compatibility: Compatibility,
    pub compatibility_notes: Option<String>,
    pub category: ProviderCategory,
    pub endpoints: Vec<ProviderEndpointInfo>,
    pub default_endpoint: Option<String>,
    pub auth: Auth,
    pub model_discovery: ModelDiscovery,
    pub url_params: Vec<UrlParamInfo>,
    pub translations: ProviderTranslations,
}

/// A user-supplied URL placeholder (zh text; en / ja in `translations.*.url_params`).
#[derive(Debug, Serialize)]
pub struct UrlParamInfo {
    pub id: String,
    pub label: String,
    pub placeholder: Option<String>,
    pub pattern: String,
}

#[derive(Debug, Serialize)]
pub struct UrlParamText {
    pub label: String,
    pub placeholder: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ProviderEndpointInfo {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub base_url: String,
    pub messages_path: String,
    pub region: Option<String>,
    pub billing: Option<String>,
    pub protocol: EndpointProtocol,
    pub example_models: Vec<String>,
    /// Ids of the provider's `url_params` this endpoint needs the user to fill.
    pub url_params_used: Vec<String>,
}

/// 三语齐全 (yaml 解析时已强制), 所以每种语言都是完整的一份, 客户端不需要回退。
#[derive(Debug, Serialize)]
pub struct ProviderTranslations {
    pub en: ProviderText,
    pub ja: ProviderText,
}

#[derive(Debug, Serialize)]
pub struct ProviderText {
    pub display_name: String,
    pub description: Option<String>,
    pub compatibility_notes: Option<String>,
    /// key 是 endpoint id
    pub endpoints: BTreeMap<String, EndpointText>,
    /// key 是 url_param id
    pub url_params: BTreeMap<String, UrlParamText>,
}

#[derive(Debug, Serialize)]
pub struct EndpointText {
    pub label: String,
    pub description: Option<String>,
}

impl ProviderText {
    fn pick(p: &Provider, lang: fn(&LocalizedText) -> &String) -> Self {
        Self {
            display_name: lang(&p.display_name).clone(),
            description: p.description.as_ref().map(|t| lang(t).clone()),
            compatibility_notes: p.compatibility_notes.as_ref().map(|t| lang(t).clone()),
            endpoints: p
                .endpoints
                .iter()
                .map(|e| {
                    let text = EndpointText {
                        label: lang(&e.label).clone(),
                        description: e.description.as_ref().map(|t| lang(t).clone()),
                    };
                    (e.id.clone(), text)
                })
                .collect(),
            url_params: p
                .url_params
                .iter()
                .map(|u| {
                    let text = UrlParamText {
                        label: lang(&u.label).clone(),
                        placeholder: u.placeholder.as_ref().map(|t| lang(t).clone()),
                    };
                    (u.id.clone(), text)
                })
                .collect(),
        }
    }
}

impl From<&Provider> for ProviderInfo {
    fn from(p: &Provider) -> Self {
        Self {
            id: p.id.clone(),
            display_name: p.display_name.zh.clone(),
            description: p.description.as_ref().map(|t| t.zh.clone()),
            homepage: p.homepage.clone(),
            docs_url: p.docs_url.clone(),
            api_key_url: p.api_key_url.clone(),
            icon: p.icon.clone(),
            compatibility: p.compatibility.clone(),
            compatibility_notes: p.compatibility_notes.as_ref().map(|t| t.zh.clone()),
            category: p.category,
            endpoints: p
                .endpoints
                .iter()
                .map(|e| ProviderEndpointInfo {
                    id: e.id.clone(),
                    label: e.label.zh.clone(),
                    description: e.description.as_ref().map(|t| t.zh.clone()),
                    base_url: e.base_url.clone(),
                    messages_path: e.messages_path.clone(),
                    region: e.region.clone(),
                    billing: e.billing.clone(),
                    protocol: e.protocol,
                    example_models: e.example_models.clone(),
                    url_params_used: p.params_used(e),
                })
                .collect(),
            default_endpoint: p.default_endpoint.clone(),
            auth: p.auth.clone(),
            model_discovery: p.model_discovery.clone(),
            url_params: p
                .url_params
                .iter()
                .map(|u| UrlParamInfo {
                    id: u.id.clone(),
                    label: u.label.zh.clone(),
                    placeholder: u.placeholder.as_ref().map(|t| t.zh.clone()),
                    pattern: u.pattern.clone(),
                })
                .collect(),
            translations: ProviderTranslations {
                en: ProviderText::pick(p, |t| &t.en),
                ja: ProviderText::pick(p, |t| &t.ja),
            },
        }
    }
}

#[tauri::command]
pub async fn list_providers(state: State<'_, AppState>) -> AppResult<Vec<ProviderInfo>> {
    let mut out: Vec<ProviderInfo> = state.providers.values().map(ProviderInfo::from).collect();
    // 三种界面语言都按英文名排, 顺序一致; 忽略大小写 (否则 `xAI` 会排到 `Zhipu` 后面)。
    out.sort_by_cached_key(|p| p.translations.en.display_name.to_lowercase());
    Ok(out)
}
