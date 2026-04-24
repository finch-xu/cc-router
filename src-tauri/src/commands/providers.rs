use serde::Serialize;
use tauri::State;

use crate::error::AppResult;
use crate::provider::model::{Auth, Compatibility, ModelDiscovery, ProviderEndpoint};
use crate::state::AppState;

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
    pub endpoints: Vec<ProviderEndpoint>,
    pub default_endpoint: Option<String>,
    pub auth: Auth,
    pub model_discovery: ModelDiscovery,
}

#[tauri::command]
pub async fn list_providers(state: State<'_, AppState>) -> AppResult<Vec<ProviderInfo>> {
    let mut out: Vec<ProviderInfo> = state
        .providers
        .values()
        .map(|p| ProviderInfo {
            id: p.id.clone(),
            display_name: p.display_name.clone(),
            description: p.description.clone(),
            homepage: p.homepage.clone(),
            docs_url: p.docs_url.clone(),
            api_key_url: p.api_key_url.clone(),
            icon: p.icon.clone(),
            compatibility: p.compatibility.clone(),
            compatibility_notes: p.compatibility_notes.clone(),
            endpoints: p.endpoints.clone(),
            default_endpoint: p.default_endpoint.clone(),
            auth: p.auth.clone(),
            model_discovery: p.model_discovery.clone(),
        })
        .collect();
    out.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    Ok(out)
}
