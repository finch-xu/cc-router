//! Placeholders in provider yaml strings.
//!
//! - User-supplied `url_params` (`{account_id}`) are resolved once when a subscription is
//!   created / edited; the resolved strings are snapshotted into the subscription row.
//! - Two reserved names are resolved at send time only: `{model}` (in `messages_path`) and
//!   `{api_key}` (in header values). `{api_key}` is never resolved into the DB.

use std::collections::BTreeMap;

pub const RESERVED_MODEL: &str = "model";
pub const RESERVED_API_KEY: &str = "api_key";

fn is_ident(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
}

pub fn placeholders(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else { break };
        let name = &after[..end];
        if is_ident(name) {
            out.push(name.to_string());
            rest = &after[end + 1..];
        } else {
            rest = after;
        }
    }
    out
}

pub fn resolve(s: &str, params: &BTreeMap<String, String>) -> String {
    let mut out = s.to_string();
    for (k, v) in params {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

pub fn fill_model(s: &str, model: &str) -> String {
    s.replace("{model}", model)
}

pub fn fill_api_key(s: &str, key: &str) -> String {
    s.replace("{api_key}", key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn placeholders_are_found_in_order() {
        assert_eq!(
            placeholders("https://x/accounts/{account_id}/ai/run/{model}"),
            vec!["account_id".to_string(), "model".to_string()]
        );
    }

    #[test]
    fn non_identifier_braces_are_not_placeholders() {
        // JSON-ish or uppercase content is not a placeholder; unclosed brace is ignored
        assert!(placeholders("{\"a\":1} {Account} {a-b} {").is_empty());
        assert!(placeholders("no braces").is_empty());
    }

    #[test]
    fn resolve_replaces_only_known_params() {
        let out = resolve("/accounts/{account_id}/run/{model}", &params(&[("account_id", "abc")]));
        assert_eq!(out, "/accounts/abc/run/{model}");
    }

    #[test]
    fn resolve_replaces_every_occurrence() {
        assert_eq!(resolve("{a}-{a}", &params(&[("a", "x")])), "x-x");
    }

    #[test]
    fn fill_model_and_api_key() {
        assert_eq!(fill_model("/run/@cf/cloudflare/{model}", "clef-flash"), "/run/@cf/cloudflare/clef-flash");
        assert_eq!(fill_api_key("Bearer {api_key}", "k1"), "Bearer k1");
        assert_eq!(fill_api_key("no template", "k1"), "no template");
    }
}
