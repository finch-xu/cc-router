//! Connection snapshot of a built-in subscription: yaml template + user url_params → the
//! strings stored on the subscription row. Shared by create and edit so both resolve identically.

use std::collections::BTreeMap;

use crate::provider::model::{EndpointProtocol, ModelDiscovery, Provider, ProviderEndpoint, SystemoneWire};
use crate::provider::url_template::resolve;

#[derive(Debug, Clone)]
pub struct ConnectionSnapshot {
    pub base_url: String,
    pub messages_path: String,
    pub required_headers: BTreeMap<String, String>,
    pub model_discovery: ModelDiscovery,
    pub endpoint_protocol: EndpointProtocol,
    pub systemone_wire: SystemoneWire,
    pub url_params: BTreeMap<String, String>,
}

pub fn snapshot_connection(
    provider: &Provider,
    endpoint: &ProviderEndpoint,
    input: &BTreeMap<String, String>,
) -> Result<ConnectionSnapshot, String> {
    let mut params = BTreeMap::new();
    for id in provider.params_used(endpoint) {
        let decl = provider.url_param(&id).expect("params_used only returns declared ids");
        let value = input.get(&id).map(|v| v.trim()).unwrap_or("");
        if value.is_empty() {
            return Err(format!("请填写{}", decl.label.zh));
        }
        // pattern compiled successfully at load time (loader validate_semantics)
        let re = regex::Regex::new(&decl.pattern).map_err(|_| format!("{}格式不正确", decl.label.zh))?;
        if !re.is_match(value) {
            return Err(format!("{}格式不正确", decl.label.zh));
        }
        params.insert(id, value.to_string());
    }

    let mut required_headers: BTreeMap<String, String> =
        provider.required_headers.iter().map(|(k, v)| (k.clone(), resolve(v, &params))).collect();
    for (k, v) in &endpoint.headers {
        required_headers.insert(k.clone(), resolve(v, &params));
    }

    let mut model_discovery = provider.model_discovery.clone();
    if !endpoint.example_models.is_empty() {
        model_discovery.example_models = endpoint.example_models.clone();
    }
    model_discovery.url = model_discovery.url.as_deref().map(|u| resolve(u, &params));

    Ok(ConnectionSnapshot {
        base_url: resolve(&endpoint.base_url, &params),
        messages_path: resolve(&endpoint.messages_path, &params),
        required_headers,
        model_discovery,
        endpoint_protocol: endpoint.protocol,
        systemone_wire: endpoint.systemone_wire,
        url_params: params,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::loader::parse_single_for_test; // 见 Step 3 说明

    const Y: &str = r#"
id: t
display_name: "T"
compatibility: verified
url_params:
  - id: account_id
    label: {zh: "账户 ID", en: "Account ID", ja: "アカウント ID"}
    pattern: "^[0-9a-f]{32}$"
  - id: gateway_id
    label: {zh: "网关 ID", en: "Gateway ID", ja: "ゲートウェイ ID"}
    pattern: "^[A-Za-z0-9_-]{1,64}$"
endpoints:
  - id: direct
    label: "Direct"
    base_url: "https://api.example.com/accounts/{account_id}/ai"
    messages_path: "/run/{model}"
    protocol: systemone
    systemone_wire: cloudflare_run
    example_models: ["clef-flash"]
  - id: gateway
    label: "Gateway"
    base_url: "https://api.example.com/accounts/{account_id}/ai"
    messages_path: "/run/{model}"
    protocol: systemone
    systemone_wire: cloudflare_run
    headers:
      cf-aig-gateway-id: "{gateway_id}"
      cf-aig-authorization: "Bearer {api_key}"
auth:
  type: api_key
  header_name: "Authorization"
  header_format: bearer
required_headers:
  x-base: "1"
model_discovery:
  enabled: false
  url: "https://api.example.com/accounts/{account_id}/ai/models"
"#;
    const ACCT: &str = "0123456789abcdef0123456789abcdef";

    fn p(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn direct_endpoint_resolves_and_keeps_reserved_placeholders() {
        let prov = parse_single_for_test(Y);
        let s = snapshot_connection(&prov, prov.endpoint("direct").unwrap(), &p(&[("account_id", &format!(" {ACCT} ")), ("gateway_id", "ignored")])).unwrap();
        assert_eq!(s.base_url, format!("https://api.example.com/accounts/{ACCT}/ai"));
        assert_eq!(s.messages_path, "/run/{model}");
        assert_eq!(s.model_discovery.url.as_deref(), Some(&*format!("https://api.example.com/accounts/{ACCT}/ai/models")));
        assert_eq!(s.model_discovery.example_models, vec!["clef-flash".to_string()]);
        assert_eq!(s.systemone_wire, SystemoneWire::CloudflareRun);
        // only params the endpoint uses are kept, trimmed
        assert_eq!(s.url_params, p(&[("account_id", ACCT)]));
        assert_eq!(s.required_headers, p(&[("x-base", "1")]));
    }

    #[test]
    fn gateway_endpoint_merges_headers_and_keeps_api_key_template() {
        let prov = parse_single_for_test(Y);
        let s = snapshot_connection(&prov, prov.endpoint("gateway").unwrap(), &p(&[("account_id", ACCT), ("gateway_id", "my-gw")])).unwrap();
        assert_eq!(s.required_headers["cf-aig-gateway-id"], "my-gw");
        assert_eq!(s.required_headers["cf-aig-authorization"], "Bearer {api_key}");
        assert_eq!(s.required_headers["x-base"], "1");
    }

    #[test]
    fn missing_or_malformed_params_are_rejected_with_label() {
        let prov = parse_single_for_test(Y);
        let gw = prov.endpoint("gateway").unwrap();
        let err = snapshot_connection(&prov, gw, &p(&[("account_id", ACCT)])).unwrap_err();
        assert!(err.contains("网关 ID"), "{err}");
        let err = snapshot_connection(&prov, gw, &p(&[("account_id", "nothex"), ("gateway_id", "g")])).unwrap_err();
        assert!(err.contains("账户 ID"), "{err}");
    }

    const PLAIN: &str = r#"
id: plain
display_name: "Plain"
compatibility: verified
endpoints:
  - id: main
    label: "Main"
    base_url: "https://api.example.com"
    messages_path: "/v1/messages"
auth:
  type: api_key
  header_name: "x-api-key"
  header_format: raw
required_headers:
  anthropic-version: "2023-06-01"
model_discovery:
  enabled: true
  example_models: ["m1"]
"#;

    #[test]
    fn provider_without_params_snapshots_as_before() {
        let prov = parse_single_for_test(PLAIN);
        let s = snapshot_connection(&prov, prov.endpoint("main").unwrap(), &BTreeMap::new()).unwrap();
        assert_eq!(s.base_url, "https://api.example.com");
        assert_eq!(s.messages_path, "/v1/messages");
        assert_eq!(s.required_headers, p(&[("anthropic-version", "2023-06-01")]));
        assert_eq!(s.model_discovery.example_models, vec!["m1".to_string()]);
        assert_eq!(s.systemone_wire, SystemoneWire::Standard);
        assert!(s.url_params.is_empty());
    }
}
