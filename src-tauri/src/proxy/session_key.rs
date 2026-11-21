//! Session key extraction for sticky routing. Pure function, no IO.
//!
//! Priority (first hit wins):
//! 0. SystemOne entry: only the `x-session-id` header (no body-derived keys — identical `state`s are independent decisions)
//! 1. `x-claude-code-session-id` header (Claude Code >= 2.1.86)
//! 2. `body.metadata.user_id` — used verbatim as an opaque key (both the legacy
//!    `user_<hex>_account_<uuid>_session_<uuid>` and the 2.1.78+ JSON string form)
//! 3. Responses entry only: `body.prompt_cache_key`, then `session_id` header (Codex CLI)
//! 4. ChatCompletions entry only: `body.user` → `x-session-id` header
//! 5. SHA-256 (first 32 hex) of the first `role=user` message text — NOT the system prompt,
//!    which is identical across all Claude Code sessions and would pin everything to one sub
//!    (Messages entry only — Responses bodies carry `input`, not `messages`, so this level
//!    never fires on `/v1/responses`)
//! 6. None → the request is not pinned.

use axum::http::HeaderMap;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::proxy::client_fingerprint::RequestEntryKind;

pub const MAX_KEY_BYTES: usize = 256;
/// OpenAI Responses API `prompt_cache_key` 上限 (64 字符).
pub const MAX_PROMPT_CACHE_KEY_LEN: usize = 64;

/// 把 `extract()` 得到的内部会话键规范化为可安全发往 OpenAI Responses / ChatGPT Codex
/// 后端的 `prompt_cache_key` (同时用作 Codex 的 `session_id` header):
/// - `pck:` / `sid:` / `hdr:` 前缀下若原始值非空、≤ 64 字符且均为可见非空白 ASCII, 直接复用原始 ID
///   (保留 Codex CLI / Claude Code 原生 UUID);
/// - 其余情况 (超长 `meta:` JSON、含空白/非 ASCII、或 `msg:` 等) 经 SHA-256 归一化为
///   36 字符稳定键 `ccr_<32-hex>`, 既满足 64 字符上限也不泄露原始 metadata 文本.
pub fn derive_prompt_cache_key(session_key: &str) -> String {
    for prefix in ["pck:", "sid:", "hdr:"] {
        if let Some(raw) = session_key.strip_prefix(prefix) {
            if !raw.is_empty()
                && raw.len() <= MAX_PROMPT_CACHE_KEY_LEN
                && raw.bytes().all(|b| b.is_ascii_graphic())
            {
                return raw.to_string();
            }
        }
    }
    let digest = Sha256::digest(session_key.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("ccr_{}", &hex[..32])
}

pub fn extract(headers: &HeaderMap, body: &Value, entry_kind: RequestEntryKind) -> Option<String> {
    if matches!(entry_kind, RequestEntryKind::SystemOne) {
        return header_str(headers, "x-session-id").map(|v| truncate(format!("sid:{v}")));
    }
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
    if matches!(entry_kind, RequestEntryKind::ChatCompletions) {
        // OpenAI 官方的 `user` 字段 (Open WebUI 等按用户填); 读翻译前的原始 body.
        if let Some(v) = body.get("user").and_then(|u| u.as_str()).map(str::trim).filter(|s| !s.is_empty()) {
            return Some(truncate(format!("user:{v}")));
        }
        if let Some(v) = header_str(headers, "x-session-id") {
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
    fn derive_prompt_cache_key_preserves_safe_ids_and_hashes_long_or_opaque_keys() {
        assert_eq!(derive_prompt_cache_key("hdr:550e8400-e29b-41d4-a716-446655440000"), "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(derive_prompt_cache_key("pck:thread-1"), "thread-1");
        assert_eq!(derive_prompt_cache_key("sid:codex-sess"), "codex-sess");

        // metadata JSON / 超长值 / 含空白 → 归一化为定长 36 字符 ccr_<32-hex>
        let meta = "meta:{\"device_id\":\"d\",\"account_uuid\":\"a\",\"session_id\":\"s\"}";
        let derived = derive_prompt_cache_key(meta);
        assert!(derived.starts_with("ccr_"));
        assert_eq!(derived.len(), 36);
        assert_eq!(derived, derive_prompt_cache_key(meta));
        assert_ne!(derived, derive_prompt_cache_key("meta:{\"session_id\":\"other\"}"));

        let too_long = format!("hdr:{}", "a".repeat(65));
        let derived_long = derive_prompt_cache_key(&too_long);
        assert!(derived_long.len() <= MAX_PROMPT_CACHE_KEY_LEN);
        assert!(derived_long.starts_with("ccr_"));

        let with_space = "hdr:bad id";
        assert!(derive_prompt_cache_key(with_space).starts_with("ccr_"));
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

    #[test]
    fn chat_completions_user_field_then_x_session_id_header() {
        let body = json!({"user": "u-1", "messages": [{"role":"user","content":"hi"}]});
        assert_eq!(
            extract(&HeaderMap::new(), &body, RequestEntryKind::ChatCompletions).as_deref(),
            Some("user:u-1")
        );
        let h = hm(&[("x-session-id", "owui-1")]);
        let no_user = json!({"messages": [{"role":"user","content":"hi"}]});
        assert_eq!(
            extract(&h, &no_user, RequestEntryKind::ChatCompletions).as_deref(),
            Some("sid:owui-1")
        );
        // user 优先于 x-session-id
        assert_eq!(extract(&h, &body, RequestEntryKind::ChatCompletions).as_deref(), Some("user:u-1"));
        // 其他入口不读 user / x-session-id, 落到首条 user 消息 hash
        assert!(extract(&h, &body, RequestEntryKind::Messages).unwrap().starts_with("msg:"));
        assert!(extract(&h, &body, RequestEntryKind::Responses).unwrap().starts_with("msg:"));
    }

    #[test]
    fn chat_completions_falls_back_to_first_user_hash_and_ignores_blank_user() {
        let body = json!({"user": "  ", "messages": [{"role":"user","content":"hello"}]});
        let k = extract(&HeaderMap::new(), &body, RequestEntryKind::ChatCompletions).unwrap();
        assert!(k.starts_with("msg:"));
    }

    #[test]
    fn systemone_only_uses_x_session_id() {
        use axum::http::HeaderMap;
        let body = serde_json::json!({
            "model": "jev-latest",
            "state": "hello",
            "metadata": {"user_id": "u1"},
            "messages": [{"role": "user", "content": "x"}]
        });
        let mut h = HeaderMap::new();
        assert_eq!(extract(&h, &body, RequestEntryKind::SystemOne), None, "不拿 metadata / state 做键");
        h.insert("x-claude-code-session-id", "cc".parse().unwrap());
        assert_eq!(extract(&h, &body, RequestEntryKind::SystemOne), None, "CC 的会话头也不认");
        h.insert("x-session-id", "s-42".parse().unwrap());
        assert_eq!(extract(&h, &body, RequestEntryKind::SystemOne).as_deref(), Some("sid:s-42"));
    }

    #[test]
    fn systemone_entry_kind_wire_name() {
        assert_eq!(RequestEntryKind::SystemOne.as_str(), "systemone");
    }
}
