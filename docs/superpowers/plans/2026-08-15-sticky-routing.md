# 会话亲和调度 (`sticky`) 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给虚拟模型加第三种调度模式 `sticky`：同一会话固定同一订阅（保住 prompt cache、并发子代理共享缓存），跨会话轮询均衡，钉住方失败/冷却/超限时切下家并改钉。

**Architecture:** `proxy/session_key.rs` 纯函数从请求头 + body 提取会话键（`X-Claude-Code-Session-Id` → `metadata.user_id` → Responses 的 `prompt_cache_key`/`session_id` → 首条 user 消息 hash），两个 handler 算好塞进 `ClientContext.session_key`；`AppState.session_affinity` 持有内存 `AffinityTable`（(vm, key) → sub_id，空闲 1h 过期，10k 上限）；`build_candidate_order` 加 `pinned` 参数，Sticky 模式下钉住可用则排首且不前进轮询索引；pipeline 每次把请求交给某候选就立即改钉。

**Tech Stack:** Rust (axum HeaderMap / serde_json / sha2 已在依赖) + React。

**Spec:** `docs/superpowers/specs/2026-08-15-token-quota-and-sticky-routing-design.md` §B

## Global Constraints

- `RoutingMode::Sticky` serde 值 `"sticky"`；DB `virtual_model_config.mode` 是 TEXT，**无 migration**。
- 会话键**整串不透明**使用（不解析 `metadata.user_id` 格式）；不用 system prompt 做兜底键；截断 256 字节。
- 空闲 TTL 1h（命中刷新）、上限 10 000 条按 `last_seen` 最旧淘汰；**不持久化**。
- `last_used_index` 只在分配新会话时前进。
- 现有 `sequential` / `round_robin` 行为**零变化**（回归测试锁住）。
- 前端 i18n 本轮只改 `zh.json`。
- 每任务 `cd src-tauri && cargo test` 全绿；改前端后 `pnpm tsc --noEmit`。
- 提交信息末尾带 `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`。
- pipeline 层没有可构造 `AppState` 的集成测试基座，pipeline 接线靠单测覆盖的部件 + `cargo check` + 手动冒烟。

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `src-tauri/src/virtual_model/model.rs` | `RoutingMode::Sticky` |
| `src-tauri/src/virtual_model/store.rs` | `"sticky"` 字符串映射 |
| `src-tauri/src/proxy/session_key.rs` (新) | 会话键提取纯函数 |
| `src-tauri/src/proxy/mod.rs` | `pub mod session_key;` |
| `src-tauri/src/virtual_model/affinity.rs` (新) | `AffinityTable` |
| `src-tauri/src/virtual_model/mod.rs` | `pub mod affinity;` |
| `src-tauri/src/virtual_model/scheduler.rs` | `build_candidate_order(.., pinned)` |
| `src-tauri/src/proxy/client_fingerprint.rs` | `ClientContext.session_key` |
| `src-tauri/src/proxy/handler.rs` | 两个 handler 提取会话键 |
| `src-tauri/src/state.rs` + `lib.rs` | `AppState.session_affinity` |
| `src-tauri/src/proxy/pipeline.rs` | 取钉 → 调度 → 每次候选改钉 |
| `src/types.ts` / `src/lib/virtualModels.ts` / `src/routes/VirtualModels.tsx` / `zh.json` | 第三个模式按钮 |
| `CLAUDE.md` / `README.md` | 文档 |

---

### Task 1: `RoutingMode::Sticky` + store 映射

**Files:**
- Modify: `src-tauri/src/virtual_model/model.rs:117-121`
- Modify: `src-tauri/src/virtual_model/store.rs:21-24, 61-64`
- Test: `store.rs`（若无 tests 模块则新建，用内存 pool）

- [ ] **Step 1: 写失败测试**（`virtual_model/store.rs` 末尾）

```rust
#[cfg(test)]
mod sticky_tests {
    use super::*;
    use crate::db::run_migrations;
    use sqlx::sqlite::SqlitePoolOptions;
    use std::path::PathBuf;

    #[tokio::test]
    async fn sticky_mode_roundtrips_through_db() {
        let pool = SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        run_migrations(&pool, &PathBuf::from(".")).await.unwrap();
        save_mode(&pool, VirtualModelName::Sonnet, RoutingMode::Sticky).await.unwrap();
        let all = load_all(&pool).await.unwrap();
        assert_eq!(all[&VirtualModelName::Sonnet].mode, RoutingMode::Sticky);
        // serde 值
        assert_eq!(serde_json::to_string(&RoutingMode::Sticky).unwrap(), "\"sticky\"");
    }
}
```

（`load_all` 的返回类型看文件实际签名——若返回 `HashMap<VirtualModelName, VirtualModelConfig>` 如上索引；否则按实际取。）

- [ ] **Step 2: 运行确认失败**

Run: `cd src-tauri && cargo test sticky_mode_roundtrips`
Expected: 编译错误 `no variant Sticky`

- [ ] **Step 3: 实现**

`model.rs`：
```rust
pub enum RoutingMode {
    Sequential,
    RoundRobin,
    /// 会话亲和: 同一会话钉住同一订阅, 新会话按轮询分配 (见 proxy::session_key / virtual_model::affinity)
    Sticky,
}
```
`store.rs` load：`Some("sticky") => RoutingMode::Sticky,`；save：`RoutingMode::Sticky => "sticky",`。`cargo check` 找出所有 `match mode` 非穷尽处（`commands/virtual_models.rs` 等）补分支。

- [ ] **Step 4: 运行测试**

Run: `cd src-tauri && cargo test virtual_model::`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/virtual_model/model.rs src-tauri/src/virtual_model/store.rs
git commit -m "feat(routing): RoutingMode 加 sticky 变体与持久化映射

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: `proxy/session_key.rs` 会话键提取

**Files:**
- Create: `src-tauri/src/proxy/session_key.rs`
- Modify: `src-tauri/src/proxy/mod.rs`（`pub mod session_key;`）

**Interfaces:**
- Produces:
  ```rust
  pub const MAX_KEY_BYTES: usize = 256;
  /// entry_kind 决定是否读 Responses 专属字段 (prompt_cache_key / session_id 头)
  pub fn extract(headers: &HeaderMap, body: &Value, entry_kind: RequestEntryKind) -> Option<String>
  ```
  返回值带来源前缀：`hdr:` / `meta:` / `pck:` / `sid:` / `msg:`。

- [ ] **Step 1: 写失败测试**（文件底部）

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use serde_json::json;

    fn hm(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs { h.insert(*k, HeaderValue::from_str(v).unwrap()); }
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
```

- [ ] **Step 2: 运行确认失败**

Run: `cd src-tauri && cargo test session_key::`
Expected: 编译错误

- [ ] **Step 3: 实现**

```rust
//! Session key extraction for sticky routing. Pure function, no IO.
//!
//! Priority (first hit wins):
//! 1. `x-claude-code-session-id` header (Claude Code >= 2.1.86)
//! 2. `body.metadata.user_id` — used verbatim as an opaque key (both the legacy
//!    `user_<hex>_account_<uuid>_session_<uuid>` and the 2.1.78+ JSON string form)
//! 3. Responses entry only: `body.prompt_cache_key`, then `session_id` header (Codex CLI)
//! 4. SHA-256 (first 32 hex) of the first `role=user` message text — NOT the system prompt,
//!    which is identical across all Claude Code sessions and would pin everything to one sub
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
        while !s.is_char_boundary(cut) { cut -= 1; }
        s.truncate(cut);
    }
    s
}
```

`proxy/mod.rs` 加 `pub mod session_key;`。`RequestEntryKind` 若不是 `Copy`，`matches!` 仍可用；确认它 `derive(PartialEq)` 或改用 `matches!`（已用）。

- [ ] **Step 4: 运行测试**

Run: `cd src-tauri && cargo test session_key::`
Expected: 6 PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/proxy/session_key.rs src-tauri/src/proxy/mod.rs
git commit -m "feat(routing): 会话键提取 (CC session 头 / metadata.user_id / Responses / 首条 user 消息 hash)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: `virtual_model/affinity.rs` 亲和表

**Files:**
- Create: `src-tauri/src/virtual_model/affinity.rs`
- Modify: `src-tauri/src/virtual_model/mod.rs`（`pub mod affinity;`）

**Interfaces:**
- Produces:
  ```rust
  pub const IDLE_TTL: Duration = Duration::from_secs(3600);
  pub const MAX_ENTRIES: usize = 10_000;
  pub struct AffinityTable { .. }   // Default
  impl AffinityTable {
      pub fn get(&mut self, vm: VirtualModelName, key: &str, now: Instant) -> Option<Uuid>;  // 过期→删并 None; 命中刷新 last_seen
      pub fn pin(&mut self, vm: VirtualModelName, key: &str, sub_id: Uuid, now: Instant);   // 超上限淘汰最旧; 每 5min 顺手 sweep
      pub fn sweep(&mut self, now: Instant);
      pub fn len(&self) -> usize;
  }
  ```

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn pin_get_and_idle_expiry() {
        let mut t = AffinityTable::default();
        let t0 = Instant::now();
        let s = Uuid::new_v4();
        assert_eq!(t.get(VirtualModelName::Sonnet, "k", t0), None);
        t.pin(VirtualModelName::Sonnet, "k", s, t0);
        assert_eq!(t.get(VirtualModelName::Sonnet, "k", t0 + Duration::from_secs(1)), Some(s));
        // 不同 vm 隔离
        assert_eq!(t.get(VirtualModelName::Opus, "k", t0), None);
        // 命中刷新: 59min 时命中, 再过 59min 仍在
        assert!(t.get(VirtualModelName::Sonnet, "k", t0 + Duration::from_secs(59 * 60)).is_some());
        assert!(t.get(VirtualModelName::Sonnet, "k", t0 + Duration::from_secs(118 * 60)).is_some());
        // 空闲超 1h → 过期
        assert_eq!(t.get(VirtualModelName::Sonnet, "k", t0 + Duration::from_secs(118 * 60 + 3601)), None);
        assert_eq!(t.len(), 0);
    }

    #[test]
    fn repin_overwrites() {
        let mut t = AffinityTable::default();
        let t0 = Instant::now();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        t.pin(VirtualModelName::Haiku, "k", a, t0);
        t.pin(VirtualModelName::Haiku, "k", b, t0);
        assert_eq!(t.get(VirtualModelName::Haiku, "k", t0), Some(b));
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn evicts_oldest_when_full() {
        let mut t = AffinityTable::default();
        let t0 = Instant::now();
        for i in 0..MAX_ENTRIES {
            t.pin(VirtualModelName::Sonnet, &format!("k{i}"), Uuid::new_v4(), t0 + Duration::from_millis(i as u64));
        }
        assert_eq!(t.len(), MAX_ENTRIES);
        t.pin(VirtualModelName::Sonnet, "new", Uuid::new_v4(), t0 + Duration::from_secs(60));
        assert_eq!(t.len(), MAX_ENTRIES);
        assert_eq!(t.get(VirtualModelName::Sonnet, "k0", t0 + Duration::from_secs(61)), None, "最旧的 k0 被淘汰");
        assert!(t.get(VirtualModelName::Sonnet, "new", t0 + Duration::from_secs(61)).is_some());
    }

    #[test]
    fn sweep_removes_only_expired() {
        let mut t = AffinityTable::default();
        let t0 = Instant::now();
        t.pin(VirtualModelName::Sonnet, "old", Uuid::new_v4(), t0);
        t.pin(VirtualModelName::Sonnet, "fresh", Uuid::new_v4(), t0 + IDLE_TTL);
        t.sweep(t0 + IDLE_TTL + Duration::from_secs(1));
        assert_eq!(t.len(), 1);
        assert!(t.get(VirtualModelName::Sonnet, "fresh", t0 + IDLE_TTL + Duration::from_secs(1)).is_some());
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cd src-tauri && cargo test affinity::`
Expected: 编译错误

- [ ] **Step 3: 实现**

```rust
//! In-memory session → subscription affinity for `RoutingMode::Sticky`.
//! Not persisted (same trade-off as `VirtualModelConfig::last_used_index`): a restart
//! costs each live session one cold cache, which is acceptable.
//!
//! Idle TTL = 1h, aligned with the longest Anthropic/OpenAI prompt-cache lifetime;
//! after that the cache is cold anyway and staying pinned would only block rebalancing.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::virtual_model::model::VirtualModelName;

pub const IDLE_TTL: Duration = Duration::from_secs(60 * 60);
pub const MAX_ENTRIES: usize = 10_000;
const SWEEP_EVERY: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Copy)]
struct Pin {
    sub_id: Uuid,
    last_seen: Instant,
}

#[derive(Debug, Default)]
pub struct AffinityTable {
    map: HashMap<(VirtualModelName, String), Pin>,
    last_sweep: Option<Instant>,
}

impl AffinityTable {
    pub fn get(&mut self, vm: VirtualModelName, key: &str, now: Instant) -> Option<Uuid> {
        let k = (vm, key.to_string());
        let pin = self.map.get_mut(&k)?;
        if now.saturating_duration_since(pin.last_seen) > IDLE_TTL {
            self.map.remove(&k);
            return None;
        }
        pin.last_seen = now;
        Some(pin.sub_id)
    }

    pub fn pin(&mut self, vm: VirtualModelName, key: &str, sub_id: Uuid, now: Instant) {
        if self.last_sweep.map_or(true, |t| now.saturating_duration_since(t) >= SWEEP_EVERY) {
            self.sweep(now);
        }
        let k = (vm, key.to_string());
        if !self.map.contains_key(&k) && self.map.len() >= MAX_ENTRIES {
            if let Some(oldest) = self.map.iter().min_by_key(|(_, p)| p.last_seen).map(|(k, _)| k.clone()) {
                self.map.remove(&oldest);
            }
        }
        self.map.insert(k, Pin { sub_id, last_seen: now });
    }

    pub fn sweep(&mut self, now: Instant) {
        self.map.retain(|_, p| now.saturating_duration_since(p.last_seen) <= IDLE_TTL);
        self.last_sweep = Some(now);
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}
```

`VirtualModelName` 需要 `Hash + Eq + Copy`（检查 `derive`，缺则加）。`virtual_model/mod.rs` 加 `pub mod affinity;`。

- [ ] **Step 4: 运行测试**

Run: `cd src-tauri && cargo test affinity::`
Expected: 4 PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/virtual_model/affinity.rs src-tauri/src/virtual_model/mod.rs src-tauri/src/virtual_model/model.rs
git commit -m "feat(routing): 内存会话亲和表 (1h 空闲过期 / 10k 上限)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: 调度器 `pinned` 参数

**Files:**
- Modify: `src-tauri/src/virtual_model/scheduler.rs`
- Modify: `src-tauri/src/proxy/pipeline.rs:146`（调用处先传 `None`，Task 5 再接真值）
- Test: `scheduler.rs` tests

**Interfaces:**
- Produces: `pub async fn build_candidate_order(vm, all_subs, now, pinned: Option<Uuid>) -> ScheduleOrder`；Sticky 且钉住可用时 `chosen_index == None`。

- [ ] **Step 1: 写失败测试**（追加到 `scheduler.rs` tests，复用 `make_rt`）

```rust
fn three(map: &mut HashMap<Uuid, Arc<RwLock<SubscriptionRuntime>>>, states: [SubscriptionState; 3]) -> [Uuid; 3] {
    let mut ids = [Uuid::nil(); 3];
    for (i, st) in states.into_iter().enumerate() {
        let rt = make_rt(!matches!(st, SubscriptionState::Disabled), st);
        ids[i] = rt.row.id;
        map.insert(rt.row.id, Arc::new(RwLock::new(rt)));
    }
    ids
}

#[tokio::test]
async fn sticky_pinned_first_and_no_index_advance() {
    let mut map = HashMap::new();
    let ids = three(&mut map, [SubscriptionState::Healthy; 3]);
    let vm = VirtualModelConfig { name: VirtualModelName::Sonnet, mode: RoutingMode::Sticky, subscription_ids: ids.to_vec(), last_used_index: 0 };
    let order = build_candidate_order(&vm, &map, Utc::now(), Some(ids[2])).await;
    assert_eq!(order.candidate_ids[0], ids[2]);
    assert_eq!(order.candidate_ids.len(), 3);
    assert_eq!(order.chosen_index, None, "钉住命中不前进轮询索引");
    // 其余按轮询序 (last_used=0 → 从 1 开始): [2, 1, 0]
    assert_eq!(order.candidate_ids, vec![ids[2], ids[1], ids[0]]);
}

#[tokio::test]
async fn sticky_falls_back_to_round_robin_when_pin_unusable() {
    let mut map = HashMap::new();
    let ids = three(&mut map, [SubscriptionState::Healthy, SubscriptionState::Healthy, SubscriptionState::RateLimited]);
    let vm = VirtualModelConfig { name: VirtualModelName::Sonnet, mode: RoutingMode::Sticky, subscription_ids: ids.to_vec(), last_used_index: 0 };
    // 钉住的 ids[2] 不可调度 → 轮询: start=1
    let order = build_candidate_order(&vm, &map, Utc::now(), Some(ids[2])).await;
    assert_eq!(order.chosen_index, Some(1));
    assert_eq!(order.candidate_ids, vec![ids[1], ids[0]]);
    // 未钉 → 同上
    let order = build_candidate_order(&vm, &map, Utc::now(), None).await;
    assert_eq!(order.chosen_index, Some(1));
    // 钉的 id 不属于本 vm → 同上
    let order = build_candidate_order(&vm, &map, Utc::now(), Some(Uuid::new_v4())).await;
    assert_eq!(order.chosen_index, Some(1));
}

#[tokio::test]
async fn sequential_and_round_robin_ignore_pinned() {
    let mut map = HashMap::new();
    let ids = three(&mut map, [SubscriptionState::Healthy; 3]);
    let seq = VirtualModelConfig { name: VirtualModelName::Opus, mode: RoutingMode::Sequential, subscription_ids: ids.to_vec(), last_used_index: 0 };
    let o = build_candidate_order(&seq, &map, Utc::now(), Some(ids[2])).await;
    assert_eq!(o.candidate_ids[0], ids[0]);
    assert_eq!(o.chosen_index, Some(0));
    let rr = VirtualModelConfig { name: VirtualModelName::Opus, mode: RoutingMode::RoundRobin, subscription_ids: ids.to_vec(), last_used_index: 0 };
    let o = build_candidate_order(&rr, &map, Utc::now(), Some(ids[2])).await;
    assert_eq!(o.candidate_ids[0], ids[1]);
    assert_eq!(o.chosen_index, Some(1));
}
```

`RateLimited` 的 `make_rt` 需要 `cooldown_until` 才不可调度？看 `SubscriptionRuntime::is_dispatchable`：`state.is_dispatchable()` 只认 `Healthy`，所以 `RateLimited` 直接不可调度，无需 cooldown。

- [ ] **Step 2: 运行确认失败**

Run: `cd src-tauri && cargo test scheduler::`
Expected: 编译错误（参数个数不符）；现有 3 个测试调用处也要补 `, None`

- [ ] **Step 3: 实现**

```rust
pub async fn build_candidate_order(
    vm: &VirtualModelConfig,
    all_subs: &HashMap<Uuid, Arc<RwLock<SubscriptionRuntime>>>,
    now: DateTime<Utc>,
    pinned: Option<Uuid>,
) -> ScheduleOrder {
    let n = vm.subscription_ids.len();
    if n == 0 {
        return ScheduleOrder { candidate_ids: vec![], chosen_index: None };
    }

    // Sticky: 钉住的订阅在本 vm 里且可调度 → 排首, 其余按轮询序兜底, 不前进索引.
    if vm.mode == RoutingMode::Sticky {
        if let Some(pin) = pinned {
            let pin_ok = match all_subs.get(&pin) {
                Some(rt) if vm.subscription_ids.contains(&pin) => rt.read().await.is_dispatchable(now),
                _ => false,
            };
            if pin_ok {
                let start = (vm.last_used_index + 1) % n;
                let mut candidate_ids = vec![pin];
                for i in 0..n {
                    let sub_id = vm.subscription_ids[(start + i) % n];
                    if sub_id == pin { continue; }
                    let Some(rt) = all_subs.get(&sub_id) else { continue };
                    if rt.read().await.is_dispatchable(now) {
                        candidate_ids.push(sub_id);
                    }
                }
                return ScheduleOrder { candidate_ids, chosen_index: None };
            }
        }
    }

    // 构造扫描顺序 (Sticky 未命中时按轮询)
    let scan_order: Vec<usize> = match vm.mode {
        RoutingMode::Sequential => (0..n).collect(),
        RoutingMode::RoundRobin | RoutingMode::Sticky => {
            let start = (vm.last_used_index + 1) % n;
            (0..n).map(|i| (start + i) % n).collect()
        }
    };
    // ...以下原逻辑不变
```

`pipeline.rs:146` 调用改为 `build_candidate_order(&vm_config, &subs_map, Utc::now(), None).await`（Task 5 替换为真值）。`RoutingMode` 需 `PartialEq`（已有）。

- [ ] **Step 4: 运行测试**

Run: `cd src-tauri && cargo test scheduler:: && cargo check`
Expected: 6 PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/virtual_model/scheduler.rs src-tauri/src/proxy/pipeline.rs
git commit -m "feat(routing): 调度器支持 sticky 钉住优先 + 未命中回退轮询

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: 接线 — `ClientContext.session_key` / `AppState.session_affinity` / pipeline

**Files:**
- Modify: `src-tauri/src/proxy/client_fingerprint.rs:79-87`（`ClientContext` 加字段）
- Modify: `src-tauri/src/proxy/handler.rs:63-68, 170-175`（两处构造）
- Modify: `src-tauri/src/state.rs:20-46` + `src-tauri/src/lib.rs:282-299`
- Modify: `src-tauri/src/proxy/pipeline.rs:128-200`
- Modify: 其他构造 `ClientContext { .. }` 的地方（`ping.rs` / `recheck_worker` / tests）加 `session_key: None`——用 `cargo check` 找全

- [ ] **Step 1: ClientContext**

```rust
/// 会话亲和键 (见 proxy::session_key). None = 本请求不参与 sticky.
pub session_key: Option<String>,
```

- [ ] **Step 2: handler 两处**

`messages`（~L63）：`session_key: session_key::extract(&headers, &parsed, RequestEntryKind::Messages),`
`responses`（~L170）：`session_key: session_key::extract(&headers, &parsed, RequestEntryKind::Responses),`（**用翻译前的原始 `parsed`**，`prompt_cache_key` 翻译后就没了）。顶部 `use crate::proxy::session_key;`。

- [ ] **Step 3: AppState**

`state.rs`：
```rust
/// sticky 模式的会话 → 订阅亲和表. 内存, 不持久化.
pub session_affinity: Arc<std::sync::Mutex<AffinityTable>>,
```
`lib.rs` 构造加 `session_affinity: Arc::new(std::sync::Mutex::new(AffinityTable::default())),`。

- [ ] **Step 4: pipeline**

在 `let subs_map = ...; let order = build_candidate_order(...)` 处改为：

```rust
let now_instant = std::time::Instant::now();
let pinned: Option<Uuid> = match (vm_config.mode, ctx.session_key.as_deref()) {
    (RoutingMode::Sticky, Some(key)) => state
        .session_affinity
        .lock()
        .ok()
        .and_then(|mut t| t.get(vm_name, key, now_instant)),
    _ => None,
};
let subs_map = state.subscriptions.read().await.clone();
let order = build_candidate_order(&vm_config, &subs_map, Utc::now(), pinned).await;
drop(subs_map);
```

在候选循环 `for sub_id in order.candidate_ids {` 内、`let attempt_id = ...` 之后加：

```rust
// sticky: 每次把请求交给某个候选就立即改钉 (含 retry 切下家; 不弹回)
if vm_config.mode == RoutingMode::Sticky {
    if let Some(key) = ctx.session_key.as_deref() {
        if let Ok(mut t) = state.session_affinity.lock() {
            t.pin(vm_name, key, sub_id, std::time::Instant::now());
        }
    }
}
```

顶部 `use crate::virtual_model::model::RoutingMode;`。若日志里想看到会话键来源，在 `info!("proxy received request")` 加 `session_key = ?ctx.session_key.as_deref().map(|k| &k[..k.find(':').unwrap_or(0)])`（只打前缀不打内容）。

- [ ] **Step 5: 编译 + 全量测试**

Run: `cd src-tauri && cargo check && cargo test`
Expected: 全绿

- [ ] **Step 6: 手动冒烟**

`pnpm tauri dev`，把 `model-haiku` 绑 ≥2 条订阅、模式切 sticky（先用 Task 6 之前的临时方法：`sqlite3 config.db "update virtual_model_config set mode='sticky' where virtual_model_name='model-haiku'"` 后重启 app），然后：

```bash
for i in 1 2 3; do curl -s 127.0.0.1:23456/v1/messages -H 'content-type: application/json' -H 'x-claude-code-session-id: S1' -d '{"model":"model-haiku","max_tokens":8,"messages":[{"role":"user","content":"hi"}]}' >/dev/null; done
for i in 1 2 3; do curl -s ... -H 'x-claude-code-session-id: S2' ... ; done
```

在「请求日志」页确认：S1 三条同一订阅，S2 三条同一订阅，且 S1/S2 落在不同订阅（若 ≥2 家健康）。再把 S1 所在订阅手动禁用，S1 再发一条 → 落到另一家；重新启用后 S1 继续留在新家（不弹回）。

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src
git commit -m "feat(routing): pipeline 接线 sticky — 会话键入 ClientContext, AppState 亲和表, 候选即钉

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: 前端第三个模式按钮 + zh 文案

**Files:**
- Modify: `src/types.ts:108`
- Modify: `src/lib/virtualModels.ts:66-67`
- Modify: `src/routes/VirtualModels.tsx:93-123`
- Modify: `src/i18n/locales/zh.json:392-393, 627-628`

- [ ] **Step 1: 类型与标签**

`types.ts`：`export type RoutingMode = "sequential" | "round_robin" | "sticky";`
`lib/virtualModels.ts` 标签表加 `sticky: "vm.mode.sticky",`。

- [ ] **Step 2: zh.json**

```json
"vm.mode.sticky": "会话亲和",
"virtualModels.mode.stickyHint": "同一会话固定同一订阅，跨会话轮询；保住 prompt cache，并发子代理共享缓存，失败时自动切换并改钉"
```

- [ ] **Step 3: VirtualModels.tsx**

`modeHint` 改为三分支：

```ts
const modeHint =
  vm.mode === "round_robin" ? t("virtualModels.mode.roundRobinHint")
  : vm.mode === "sticky" ? t("virtualModels.mode.stickyHint")
  : t("virtualModels.mode.sequentialHint");
```

radio-group 里在 `round_robin` 按钮后加：

```tsx
<button
  className={vm.mode === "sticky" ? "on" : ""}
  onClick={() => update("sticky", vm.subscription_ids)}
  type="button"
>
  {t("vm.mode.sticky")}
</button>
```

（`type="button"` 等属性照抄相邻两个按钮的写法。）`grep -rn "round_robin" src/` 看还有没有别处穷举模式（如 LiveRouting / RouteFlowDiagram 的模式标签），有则同样加 `sticky` 分支。

- [ ] **Step 4: 检查 + 目测**

Run: `pnpm tsc --noEmit`；`pnpm tauri dev` 虚拟模型页出现第三个按钮，切换后刷新仍是「会话亲和」，hover 有提示。

- [ ] **Step 5: Commit**

```bash
git add src/types.ts src/lib/virtualModels.ts src/routes/VirtualModels.tsx src/i18n/locales/zh.json
git commit -m "feat(routing): 虚拟模型页加「会话亲和」模式 (zh)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 7: 文档

**Files:**
- Modify: `CLAUDE.md`
- Modify: `README.md`（「调度模式：顺序还是轮询？」FAQ）

- [ ] **Step 1: CLAUDE.md**

「三个核心抽象」里 `调度模式 (sequential / round_robin)` 改为 `(sequential / round_robin / sticky)`，并在关键架构决策加一条：

「**sticky 调度按会话键钉订阅，不按内容哈希**：键优先级 `x-claude-code-session-id` 头 → `metadata.user_id` 整串 → Responses 的 `prompt_cache_key`/`session_id` → 首条 user 消息 hash（**不用 system**：CC 的 system 首块跨会话相同，会把所有会话钉到一家）。亲和表 `AppState.session_affinity` 内存不持久化，空闲 1h 过期（对齐 Anthropic/OpenAI 缓存上限）。钉住方不可调度时走轮询选新家并**改钉不弹回**；`last_used_index` 只在分配新会话时前进。LiteLLM 那种按 cache_control 前缀哈希的方案对 CC 失效（每轮 breakpoint 后移哈希就变），不要改回去。」

开头「唯一客户端是 CC，对外只暴露 Anthropic Messages 一种协议」改为「主客户端是 CC（`/v1/messages`），另有 Codex CLI 经 `/v1/responses` 入站翻译；对外不暴露 Chat Completions」。

- [ ] **Step 2: README.md FAQ**

「调度模式：顺序还是轮询？」条目加一行：
`- **会话亲和** —— 同一个 Claude Code 会话固定用同一家订阅、不同会话轮流分配。既像轮询一样均衡，又不丢 prompt cache；并发子代理也能共用缓存。订阅限流/失败时自动换家。**多家订阅都健康、又在意缓存命中时推荐。**`

（en/ja README 待用户审完中文后同步，本轮不改。）

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md README.md
git commit -m "docs: sticky 会话亲和调度说明 + 客户端范围表述更新

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## 自检记录

- Spec 覆盖：B.2 模式定义 → T1/T4/T5；B.3 会话键 → T2/T5；B.4 亲和表 → T3/T5；B.5 调度器 → T4/T5；B.6 前端 → T6；B.7 测试 → 各任务 Step 1（pipeline 集成测试无基座，以 T5 手动冒烟替代并在全局约束说明）；文档 → T7。
- 类型一致性：`session_key::extract(&HeaderMap, &Value, RequestEntryKind) -> Option<String>`、`AffinityTable::{get,pin,sweep,len,is_empty}`、`build_candidate_order(vm, all_subs, now, pinned: Option<Uuid>)`、`ClientContext.session_key: Option<String>`、`AppState.session_affinity: Arc<std::sync::Mutex<AffinityTable>>` 在 T2–T5 一致。
- 与限额计划的交汇：钉住订阅超限 → `is_dispatchable=false` → T4 的「pin_unusable 回退轮询」分支覆盖，无需额外代码。
