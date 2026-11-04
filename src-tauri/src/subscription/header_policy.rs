//! Per-subscription client-header forwarding policy (migration 026).
//!
//! Decides which headers the client sent to cc-router are forwarded to an
//! Anthropic-passthrough upstream. It never affects headers the user
//! configured on the subscription (`required_headers` / `extra_headers`):
//! those are always sent, even in `None` mode. The hard-deny set that no mode
//! can override lives in `proxy/forward.rs`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeaderPolicyMode {
    /// Forward no client headers.
    None,
    /// Built-in Claude Code preset (prefix match, see `proxy/forward.rs`).
    ClaudeCode,
    /// Forward only `names`.
    Allowlist,
    /// Forward everything except `names`.
    Denylist,
    /// Forward everything (still subject to the hard-deny set).
    All,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderPolicy {
    pub mode: HeaderPolicyMode,
    /// Used by `Allowlist` / `Denylist`. Case-insensitive; a trailing `*`
    /// means prefix match (`x-trace-*`).
    #[serde(default)]
    pub names: Vec<String>,
}

const MAX_POLICY_NAMES: usize = 50;

impl HeaderPolicy {
    pub fn none() -> Self {
        Self { mode: HeaderPolicyMode::None, names: Vec::new() }
    }

    pub fn claude_code() -> Self {
        Self { mode: HeaderPolicyMode::ClaudeCode, names: Vec::new() }
    }

    /// Default for newly created subscriptions: only the official Anthropic
    /// provider gets the Claude Code preset. Third-party compatible vendors may
    /// reject unknown `anthropic-beta` values with a 400 the user cannot debug.
    pub fn default_for_provider(provider_id: &str) -> Self {
        if provider_id == "anthropic" {
            Self::claude_code()
        } else {
            Self::none()
        }
    }

    /// Maps the pre-026 `forward_client_headers` boolean (also used when
    /// importing old backups).
    pub fn from_legacy_flag(forward_client_headers: bool) -> Self {
        if forward_client_headers {
            Self::claude_code()
        } else {
            Self::none()
        }
    }

    /// Validates `names`. Error text is user-facing (Chinese).
    pub fn validate(&self) -> Result<(), String> {
        if self.names.len() > MAX_POLICY_NAMES {
            return Err(format!(
                "请求头名单最多 {MAX_POLICY_NAMES} 条 (当前 {} 条)",
                self.names.len()
            ));
        }
        for raw in &self.names {
            let (stem, is_prefix) = match raw.strip_suffix('*') {
                Some(stem) => (stem, true),
                None => (raw.as_str(), false),
            };
            if stem.is_empty() {
                return Err(if is_prefix {
                    "前缀不能只写 *".to_string()
                } else {
                    "请求头名不能为空".to_string()
                });
            }
            if stem.contains('*') {
                return Err(format!("\"{raw}\": * 只能写在末尾表示前缀匹配"));
            }
            if reqwest::header::HeaderName::try_from(stem).is_err() {
                return Err(format!(
                    "\"{raw}\" 含非法字符 (只允许字母、数字、连字符等 HTTP token 字符)"
                ));
            }
        }
        Ok(())
    }
}

impl Default for HeaderPolicy {
    fn default() -> Self {
        Self::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_for_anthropic_is_claude_code() {
        assert_eq!(
            HeaderPolicy::default_for_provider("anthropic").mode,
            HeaderPolicyMode::ClaudeCode
        );
        assert_eq!(
            HeaderPolicy::default_for_provider("deepseek").mode,
            HeaderPolicyMode::None
        );
        assert_eq!(
            HeaderPolicy::default_for_provider("custom-anthropic").mode,
            HeaderPolicyMode::None
        );
    }

    #[test]
    fn legacy_flag_maps() {
        assert_eq!(HeaderPolicy::from_legacy_flag(true), HeaderPolicy::claude_code());
        assert_eq!(HeaderPolicy::from_legacy_flag(false), HeaderPolicy::none());
    }

    #[test]
    fn validate_rejects_inner_star() {
        let p = |names: &[&str]| HeaderPolicy {
            mode: HeaderPolicyMode::Allowlist,
            names: names.iter().map(|s| s.to_string()).collect(),
        };
        assert!(p(&["x-*-a"]).validate().is_err());
        assert!(p(&["*"]).validate().is_err());
        assert!(p(&["bad name"]).validate().is_err());
        assert!(p(&[""]).validate().is_err());
        assert!(p(&["x-foo-*", "X-Team", "anthropic-beta"]).validate().is_ok());
        let many: Vec<String> = (0..51).map(|i| format!("x-{i}")).collect();
        assert!(HeaderPolicy { mode: HeaderPolicyMode::Denylist, names: many }.validate().is_err());
    }

    #[test]
    fn serde_roundtrip() {
        let p: HeaderPolicy = serde_json::from_str(r#"{"mode":"denylist","names":["x-a"]}"#).unwrap();
        assert_eq!(p.mode, HeaderPolicyMode::Denylist);
        assert_eq!(serde_json::from_str::<HeaderPolicy>(&serde_json::to_string(&p).unwrap()).unwrap(), p);
        let n: HeaderPolicy = serde_json::from_str(r#"{"mode":"none"}"#).unwrap();
        assert!(n.names.is_empty());
        assert_eq!(
            serde_json::to_string(&HeaderPolicy::claude_code()).unwrap(),
            r#"{"mode":"claude_code","names":[]}"#
        );
    }
}
