//! Session key extraction for sticky routing. Pure function, no IO.
//!
//! Priority (first hit wins):
//! 1. `x-claude-code-session-id` header (Claude Code >= 2.1.86)
//! 2. `body.metadata.user_id` — used verbatim as an opaque key (both the legacy
//!    `user_<hex>_account_<uuid>_session_<uuid>` and the 2.1.78+ JSON string form)
//! 3. Responses entry only: `body.prompt_cache_key`, then `session_id` header (Codex CLI)
//! 4. SHA-256 (first 32 hex) of the first `role=user` message text — NOT the system prompt,
//!    which is identical across all Claude Code sessions and would pin everything to one sub
//!    (Messages entry only — Responses bodies carry `input`, not `messages`, so this level
//!    never fires on `/v1/responses`)
//! 5. None → the request is not pinned.

use axum::http::HeaderMap;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::proxy::client_fingerprint::RequestEntryKind;

pub const MAX_KEY_BYTES: usize = 256;

pub fn extract(headers: &HeaderMap, body: &Value, entry_kind: RequestEntryKind) -> Option<String> {
    if let Some(v) = header_str(headers, "x-claude-code-session-id") {
        return Some(truncate(format!("hdr:{v}")));
    }
    if let Some(v) = body.get("metadata").and_then(|m| m.get("user_id")).and_then(|u| u.as_str()) {
        if !v.is_empty() {
            return Some(truncate(format!("meta:{v}")));
        }
    }
    if matches!(entry_kind, RequestEntryKind::Responses) {
        if let Some(v) = body.get("prompt_cache_key").and_then(|u| u.as_str()).filter(|s| !s.is_empty()) {
            return Some(truncate(format!("pck:{v}")));
        }
        if let Some(v) = header_str(headers, "session_id") {
            return Some(truncate(format!("sid:{v}")));
        }
    }
    first_user_text(body).map(|text| {
        let digest = Sha256::digest(text.as_bytes());
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        format!("msg:{}", &hex[..32])
    })
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim).filter(|s| !s.is_empty())
}

fn first_user_text(body: &Value) -> Option<String> {
    let msgs = body.get("messages")?.as_array()?;
    let first = msgs.iter().find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))?;
    let content = first.get("content")?;
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(""),
        _ => return None,
    };
    (!text.is_empty()).then_some(text)
}

fn truncate(mut s: String) -> String {
    if s.len() > MAX_KEY_BYTES {
        let mut cut = MAX_KEY_BYTES;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use serde_json::json;

    fn hm(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn header_wins_over_metadata() {
        let h = hm(&[("x-claude-code-session-id", "sess-1")]);
        let body = json!({"metadata": {"user_id": "user_x_account_y_session_z"}});
        assert_eq!(extract(&h, &body, RequestEntryKind::Messages).as_deref(), Some("hdr:sess-1"));
    }

    #[test]
    fn metadata_user_id_used_verbatim() {
        let body = json!({"metadata": {"user_id": "{\"device_id\":\"d\",\"account_uuid\":\"a\",\"session_id\":\"s\"}"}});
        let k = extract(&HeaderMap::new(), &body, RequestEntryKind::Messages).unwrap();
        assert!(k.starts_with("meta:{\"device_id\""));
    }

    #[test]
    fn responses_prompt_cache_key_then_session_id_header() {
        let body = json!({"prompt_cache_key": "thread-1", "input": "hi"});
        assert_eq!(extract(&HeaderMap::new(), &body, RequestEntryKind::Responses).as_deref(), Some("pck:thread-1"));
        let h = hm(&[("session_id", "codex-sess")]);
        assert_eq!(extract(&h, &json!({"input": "hi"}), RequestEntryKind::Responses).as_deref(), Some("sid:codex-sess"));
        // Messages 入口不读这两个
        assert_eq!(extract(&h, &json!({"prompt_cache_key": "t"}), RequestEntryKind::Messages), None);
    }

    #[test]
    fn first_user_message_hash_ignores_system() {
        let a = json!({"system": [{"type":"text","text":"SAME"}],
                       "messages": [{"role":"user","content":"hello A"}]});
        let b = json!({"system": [{"type":"text","text":"SAME"}],
                       "messages": [{"role":"user","content":[{"type":"text","text":"hello B"}]}]});
        let ka = extract(&HeaderMap::new(), &a, RequestEntryKind::Messages).unwrap();
        let kb = extract(&HeaderMap::new(), &b, RequestEntryKind::Messages).unwrap();
        assert!(ka.starts_with("msg:") && kb.starts_with("msg:"));
        assert_ne!(ka, kb);
        assert_eq!(ka.len(), "msg:".len() + 32);
        // 同内容 → 同键
        assert_eq!(extract(&HeaderMap::new(), &a, RequestEntryKind::Messages).unwrap(), ka);
    }

    #[test]
    fn none_when_nothing_usable() {
        assert_eq!(extract(&HeaderMap::new(), &json!({"messages": []}), RequestEntryKind::Messages), None);
        assert_eq!(extract(&HeaderMap::new(), &json!({"messages": [{"role":"assistant","content":"x"}]}), RequestEntryKind::Messages), None);
    }

    #[test]
    fn long_keys_are_truncated() {
        let long = "x".repeat(1000);
        let h = hm(&[("x-claude-code-session-id", &long)]);
        let k = extract(&h, &json!({}), RequestEntryKind::Messages).unwrap();
        assert!(k.len() <= MAX_KEY_BYTES);
    }
}
