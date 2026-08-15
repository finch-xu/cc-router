# 订阅 token 限额 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 用户可给每条订阅设 daily / weekly / monthly / total 四种 token 上限；任一超出即该订阅暂停调度，到下一周期自动恢复；UI 用四段堆叠进度条展示用量。

**Architecture:** 限额配置存 `subscriptions.token_quotas` JSON 列；用量按 (订阅, 周期) 存 `subscription_quota_usage` 表，内存 `SubscriptionRuntime.quota_usage` 为判定真值。所有 dispatch 路径的用量都经 `request_log_tx` 汇入 `run_consumer`，在收到 entry 的瞬间累加内存计数、flush 时按快照 UPSERT。判定放 `SubscriptionRuntime::is_dispatchable`，不新增状态机状态。

**Tech Stack:** Rust (tokio / sqlx 动态 API / chrono / serde) + React + TS。

**Spec:** `docs/superpowers/specs/2026-08-15-token-quota-and-sticky-routing-design.md` §A

## Global Constraints

- 计量口径 = `input + output + cache_creation + cache_read` 四项总和。
- 周期边界按**本地时区**日历：daily 当日 0 点、weekly 周一 0 点、monthly 1 号 0 点；`total` 仅手动重置。
- 软限：只在调度时判定，`used >= limit` 即不可调度。
- **不新增 `SubscriptionState`、不动 `state_machine.rs`、不走冷却定时器**。
- SQL migration 注释里不写 `;`；不用 `sqlx::query!` 宏。
- 前端 DTO 类型在 `src/types.ts` **手工同步**；i18n 本轮**只改 `zh.json`**（en/ja 用户审过文案后另行同步）。
- 用户 facing 文案中文，Rust 注释/标识符英文。
- 每个任务结束 `cd src-tauri && cargo test` 全绿；改前端后 `pnpm tsc --noEmit` 通过。
- 提交信息末尾带 `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`。
- 项目里没有前端测试框架（无 vitest），前端只靠 `tsc`；不新增测试框架。
- 项目里没有 toast 组件（现有代码只用 `window.alert`）；限额达标事件走「事件流记录 + query 失效刷新徽标」，**不做 toast**（对 spec §A.6 的收窄）。

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `src-tauri/migrations/017_add_token_quotas.sql` (新) | 加列 + 建用量表 |
| `src-tauri/src/db/mod.rs` | `MIGRATIONS` 加 v17 |
| `src-tauri/tauri.conf.json` | `bundle.resources` 加 migration 文件 |
| `src-tauri/src/subscription/quota.rs` (新) | 纯逻辑：`QuotaPeriod` / `TokenQuotas` / `QuotaBucket` / `QuotaUsage` / `period_start` / 判定 |
| `src-tauri/src/subscription/mod.rs` | `pub mod quota;` |
| `src-tauri/src/subscription/model.rs` | `SubscriptionRow.token_quotas`、`SubscriptionRuntime.quota_usage`、`is_dispatchable` 追加判定、DTO 字段 |
| `src-tauri/src/subscription/store.rs` | 读写 `token_quotas` 列；`load_quota_usage` / `save_quota_usage_rows` / `reset_total_quota_usage` |
| `src-tauri/src/observability/request_log.rs` | consumer 累加内存计数 + flush 快照落库 + 达标事件 |
| `src-tauri/src/observability/events.rs` | `EventKind::QuotaReached` |
| `src-tauri/src/lib.rs` | bootstrap 装填用量、把订阅 map + event_tx 传给 consumer |
| `src-tauri/src/commands/subscriptions.rs` | `update_token_quotas` / `reset_total_quota_usage` 两个 command |
| `src-tauri/src/proxy/overloaded.rs` + `pipeline.rs` | 503 摘要里区分「已达限额」 |
| `src/types.ts` / `src/api/tauri.ts` | DTO / API 同步 |
| `src/lib/quota.ts` (新) | `5M`/`100M` 快捷写法解析与格式化 |
| `src/components/SubscriptionQuotaCard.tsx` (新) | 四段堆叠进度条卡片 + total 重置 |
| `src/routes/SubscriptionEdit.tsx` | 「用量限额」编辑卡片 |
| `src/routes/Subscriptions.tsx` | 列表「已达限额」徽标 |
| `src/hooks/useSubscriptions.ts` | 监听 `subscription_quota_reached` 失效 query |
| `src/i18n/locales/zh.json` | 文案 |
| `CLAUDE.md` | 隐藏约束补一条 |

---

### Task 1: migration 017（列 + 表）

**Files:**
- Create: `src-tauri/migrations/017_add_token_quotas.sql`
- Modify: `src-tauri/src/db/mod.rs:14-60`（`MIGRATIONS` 末尾）
- Modify: `src-tauri/tauri.conf.json:90` 附近（`bundle.resources`）
- Test: `src-tauri/src/db/mod.rs` tests 模块

**Interfaces:**
- Produces: 表 `subscription_quota_usage(subscription_id, period, period_start_ms, input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens, updated_at_ms)`，列 `subscriptions.token_quotas TEXT NOT NULL DEFAULT '{}'`。

- [ ] **Step 1: 写失败测试**（追加到 `src-tauri/src/db/mod.rs` 的 `mod tests`，仿照现有 fresh migration 测试的 pool 构造）

```rust
#[tokio::test]
async fn v17_adds_token_quotas_column_and_usage_table() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    run_migrations(&pool, &PathBuf::from(".")).await.unwrap();

    // 列存在: 能 SELECT
    let v: String = sqlx::query_scalar("SELECT token_quotas FROM subscriptions LIMIT 0")
        .fetch_optional(&pool)
        .await
        .unwrap()
        .unwrap_or_else(|| "{}".to_string());
    assert_eq!(v, "{}");

    // 表存在: 能插入并读回
    sqlx::query(
        "INSERT INTO subscription_quota_usage
         (subscription_id, period, period_start_ms, input_tokens, output_tokens,
          cache_creation_tokens, cache_read_tokens, updated_at_ms)
         VALUES ('s1', 'daily', 0, 1, 2, 3, 4, 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM subscription_quota_usage")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cd src-tauri && cargo test v17_adds_token_quotas -- --nocapture`
Expected: FAIL（`no such column: token_quotas` 或 `no such table`）

- [ ] **Step 3: 写 migration 文件**

`src-tauri/migrations/017_add_token_quotas.sql`：

```sql
-- 订阅 token 限额配置 (JSON, 形如 {"daily":5000000,"weekly":null,...}); '{}' = 未设限
ALTER TABLE subscriptions ADD COLUMN token_quotas TEXT NOT NULL DEFAULT '{}';

-- 每订阅每周期一行的用量快照 (内存为真值, 这里只做重启恢复)
-- period 取值 daily / weekly / monthly / total
-- period_start_ms 为该周期起点 (本地时区换算成 Unix ms), total 为上次重置时刻
CREATE TABLE subscription_quota_usage (
  subscription_id       TEXT    NOT NULL,
  period                TEXT    NOT NULL,
  period_start_ms       INTEGER NOT NULL,
  input_tokens          INTEGER NOT NULL DEFAULT 0,
  output_tokens         INTEGER NOT NULL DEFAULT 0,
  cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens     INTEGER NOT NULL DEFAULT 0,
  updated_at_ms         INTEGER NOT NULL,
  PRIMARY KEY (subscription_id, period)
);
```

- [ ] **Step 4: 注册 migration**

`src-tauri/src/db/mod.rs` `MIGRATIONS` 末尾（在 v16 条目后）加：

```rust
    (
        17,
        include_str!("../../migrations/017_add_token_quotas.sql"),
    ),
```

`src-tauri/tauri.conf.json` `bundle.resources` 在 `"migrations/016_add_model_slot_fallback.sql",` 后加一行 `"migrations/017_add_token_quotas.sql",`。

- [ ] **Step 5: 运行测试**

Run: `cd src-tauri && cargo test db::`
Expected: 全部 PASS（包括原有 fresh / legacy / rerun 三个幂等测试）

- [ ] **Step 6: Commit**

```bash
git add src-tauri/migrations/017_add_token_quotas.sql src-tauri/src/db/mod.rs src-tauri/tauri.conf.json
git commit -m "feat(db): migration 017 订阅 token 限额列 + 用量快照表

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: `subscription/quota.rs` 纯逻辑模块

**Files:**
- Create: `src-tauri/src/subscription/quota.rs`
- Modify: `src-tauri/src/subscription/mod.rs`（加 `pub mod quota;`）

**Interfaces:**
- Produces:
  ```rust
  pub enum QuotaPeriod { Daily, Weekly, Monthly, Total }   // serde snake_case; as_str()/parse()
  pub const ALL_PERIODS: [QuotaPeriod; 4];
  pub struct TokenQuotas { daily/weekly/monthly/total: Option<u64> }  // serde 同 SlotEfforts
  impl TokenQuotas { fn limit(&self, p) -> Option<u64>; fn is_empty(&self) -> bool;
                     fn first_exceeded(&self, usage: &QuotaUsage, now: DateTime<Utc>) -> Option<QuotaPeriod>;
                     fn any_exceeded(&self, usage, now) -> bool }
  pub struct QuotaBucket { period_start: DateTime<Utc>, input, output, cache_creation, cache_read: u64 }
  impl QuotaBucket { fn total(&self) -> u64 }
  pub struct QuotaUsage { .. }   // Default = 4 桶全 0, period_start = UNIX_EPOCH
  impl QuotaUsage { fn bucket(&self, p) -> &QuotaBucket; fn bucket_mut(&mut self, p) -> &mut QuotaBucket;
                    fn roll_if_needed(&mut self, now) ; fn add(&mut self, now, input, output, cc, cr);
                    fn effective(&self, p, now) -> QuotaBucket /* 过期视为 0, 只读 */;
                    fn reset_total(&mut self, now) }
  pub fn period_start(period, now: DateTime<Utc>) -> DateTime<Utc>            // 用 chrono::Local
  pub fn period_start_in<Tz: TimeZone>(period, now: DateTime<Utc>, tz: &Tz) -> DateTime<Utc>  // 可注入时区, 测试用
  pub fn period_end(period, now) -> Option<DateTime<Utc>>                      // Total → None
  ```

- [ ] **Step 1: 写失败测试**（新建文件时直接把测试写在文件底部 `#[cfg(test)] mod tests`）

```rust
// src-tauri/src/subscription/quota.rs  —— 先只放测试, 让编译失败驱动实现
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone, Utc};

    fn cst() -> FixedOffset { FixedOffset::east_opt(8 * 3600).unwrap() }

    #[test]
    fn daily_start_is_local_midnight() {
        // 2026-08-15 01:30 北京 = 2026-08-14 17:30 UTC; 本地日起点应为 08-15 00:00 CST = 08-14 16:00 UTC
        let now = Utc.with_ymd_and_hms(2026, 8, 14, 17, 30, 0).unwrap();
        let start = period_start_in(QuotaPeriod::Daily, now, &cst());
        assert_eq!(start, Utc.with_ymd_and_hms(2026, 8, 14, 16, 0, 0).unwrap());
        // 同一时刻按 UTC 算则是 08-14 00:00 UTC —— 证明是本地边界不是 UTC 边界
        let start_utc = period_start_in(QuotaPeriod::Daily, now, &Utc);
        assert_eq!(start_utc, Utc.with_ymd_and_hms(2026, 8, 14, 0, 0, 0).unwrap());
    }

    #[test]
    fn weekly_start_is_local_monday() {
        // 2026-08-16 是周日. 北京 08-16 12:00 → 周起点 08-10 (周一) 00:00 CST = 08-09 16:00 UTC
        let now = Utc.with_ymd_and_hms(2026, 8, 16, 4, 0, 0).unwrap();
        let start = period_start_in(QuotaPeriod::Weekly, now, &cst());
        assert_eq!(start, Utc.with_ymd_and_hms(2026, 8, 9, 16, 0, 0).unwrap());
        // 周一当天 00:30 仍归本周
        let mon = Utc.with_ymd_and_hms(2026, 8, 9, 16, 30, 0).unwrap();
        assert_eq!(period_start_in(QuotaPeriod::Weekly, mon, &cst()), start);
    }

    #[test]
    fn monthly_start_handles_year_boundary() {
        // 北京 2027-01-01 00:10 = 2026-12-31 16:10 UTC → 月起点 2027-01-01 00:00 CST = 2026-12-31 16:00 UTC
        let now = Utc.with_ymd_and_hms(2026, 12, 31, 16, 10, 0).unwrap();
        let start = period_start_in(QuotaPeriod::Monthly, now, &cst());
        assert_eq!(start, Utc.with_ymd_and_hms(2026, 12, 31, 16, 0, 0).unwrap());
    }

    #[test]
    fn total_period_start_is_epoch_and_no_end() {
        let now = Utc::now();
        assert_eq!(period_start_in(QuotaPeriod::Total, now, &cst()), DateTime::<Utc>::UNIX_EPOCH);
        assert!(period_end(QuotaPeriod::Total, now).is_none());
    }

    #[test]
    fn roll_if_needed_resets_only_expired_buckets() {
        let t0 = Utc.with_ymd_and_hms(2026, 8, 14, 17, 30, 0).unwrap(); // 北京 08-15 01:30
        let mut u = QuotaUsage::default();
        u.add_in(t0, &cst(), 10, 20, 30, 40);
        assert_eq!(u.bucket(QuotaPeriod::Daily).total(), 100);
        assert_eq!(u.bucket(QuotaPeriod::Total).total(), 100);
        // 次日 (北京 08-16 01:30): daily 清零, weekly (同周) / monthly / total 保留
        let t1 = t0 + chrono::Duration::days(1);
        u.roll_if_needed_in(t1, &cst());
        assert_eq!(u.bucket(QuotaPeriod::Daily).total(), 0);
        assert_eq!(u.bucket(QuotaPeriod::Weekly).total(), 100);
        assert_eq!(u.bucket(QuotaPeriod::Monthly).total(), 100);
        assert_eq!(u.bucket(QuotaPeriod::Total).total(), 100);
    }

    #[test]
    fn effective_treats_expired_bucket_as_zero_without_mutation() {
        let t0 = Utc.with_ymd_and_hms(2026, 8, 14, 17, 30, 0).unwrap();
        let mut u = QuotaUsage::default();
        u.add_in(t0, &cst(), 50, 0, 0, 0);
        let t1 = t0 + chrono::Duration::days(1);
        assert_eq!(u.effective_in(QuotaPeriod::Daily, t1, &cst()).total(), 0);
        // 只读: 原桶不变
        assert_eq!(u.bucket(QuotaPeriod::Daily).total(), 50);
    }

    #[test]
    fn first_exceeded_uses_ge_and_ignores_unset() {
        let now = Utc.with_ymd_and_hms(2026, 8, 14, 17, 30, 0).unwrap();
        let mut u = QuotaUsage::default();
        u.add_in(now, &cst(), 100, 0, 0, 0);
        let none = TokenQuotas::default();
        assert!(none.first_exceeded_in(&u, now, &cst()).is_none());
        let q = TokenQuotas { daily: Some(100), ..Default::default() };
        assert_eq!(q.first_exceeded_in(&u, now, &cst()), Some(QuotaPeriod::Daily));
        let q2 = TokenQuotas { daily: Some(101), monthly: Some(1000), ..Default::default() };
        assert!(q2.first_exceeded_in(&u, now, &cst()).is_none());
    }

    #[test]
    fn reset_total_zeroes_and_moves_start() {
        let now = Utc::now();
        let mut u = QuotaUsage::default();
        u.add_in(now, &cst(), 1, 1, 1, 1);
        u.reset_total(now);
        assert_eq!(u.bucket(QuotaPeriod::Total).total(), 0);
        assert_eq!(u.bucket(QuotaPeriod::Total).period_start, now);
        // 其他桶不受影响
        assert_eq!(u.bucket(QuotaPeriod::Daily).total(), 4);
    }

    #[test]
    fn token_quotas_serde_matches_slot_efforts_recipe() {
        let q: TokenQuotas = serde_json::from_str("{}").unwrap();
        assert!(q.is_empty());
        let q: TokenQuotas = serde_json::from_str(r#"{"daily":5000000,"unknown":1}"#).unwrap();
        assert_eq!(q.daily, Some(5_000_000));
        assert_eq!(serde_json::to_string(&q).unwrap(), r#"{"daily":5000000}"#);
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cd src-tauri && cargo test quota::`
Expected: 编译错误（模块不存在 / 类型未定义）

- [ ] **Step 3: 实现**

在测试上方写实现（同一文件）：

```rust
//! Per-subscription token quota: config (`TokenQuotas`), in-memory usage (`QuotaUsage`),
//! and local-calendar period arithmetic. Pure logic, no IO; persistence lives in
//! `subscription::store`, accounting hook in `observability::request_log::run_consumer`,
//! dispatch gate in `SubscriptionRuntime::is_dispatchable`.
//!
//! Period boundaries follow the machine's local calendar (`chrono::Local`): daily = local
//! midnight, weekly = local Monday 00:00, monthly = local 1st 00:00. `Total` never rolls;
//! its `period_start` is the last manual reset (UNIX epoch when never reset).

use std::collections::HashMap;

use chrono::{DateTime, Datelike, Duration, Local, NaiveTime, TimeZone, Utc, Weekday};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaPeriod {
    Daily,
    Weekly,
    Monthly,
    Total,
}

pub const ALL_PERIODS: [QuotaPeriod; 4] = [
    QuotaPeriod::Daily,
    QuotaPeriod::Weekly,
    QuotaPeriod::Monthly,
    QuotaPeriod::Total,
];

impl QuotaPeriod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
            Self::Total => "total",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "daily" => Some(Self::Daily),
            "weekly" => Some(Self::Weekly),
            "monthly" => Some(Self::Monthly),
            "total" => Some(Self::Total),
            _ => None,
        }
    }
    /// 中文标签, 用于 503 摘要 / 事件 summary (UI 自己走 i18n, 不用这个).
    pub fn label_zh(self) -> &'static str {
        match self {
            Self::Daily => "每日",
            Self::Weekly => "每周",
            Self::Monthly => "每月",
            Self::Total => "累计",
        }
    }
}

/// Persisted in `subscriptions.token_quotas` (JSON). Same serde recipe as `SlotEfforts`:
/// every field optional + `skip_serializing_if`, so `'{}'` = no limits and unknown keys are ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenQuotas {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekly: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

impl TokenQuotas {
    pub fn limit(&self, p: QuotaPeriod) -> Option<u64> {
        match p {
            QuotaPeriod::Daily => self.daily,
            QuotaPeriod::Weekly => self.weekly,
            QuotaPeriod::Monthly => self.monthly,
            QuotaPeriod::Total => self.total,
        }
    }
    pub fn is_empty(&self) -> bool {
        ALL_PERIODS.iter().all(|p| self.limit(*p).is_none())
    }
    /// First period (in ALL_PERIODS order) whose effective usage >= its limit.
    pub fn first_exceeded(&self, usage: &QuotaUsage, now: DateTime<Utc>) -> Option<QuotaPeriod> {
        self.first_exceeded_in(usage, now, &Local)
    }
    pub fn first_exceeded_in<Tz: TimeZone>(
        &self,
        usage: &QuotaUsage,
        now: DateTime<Utc>,
        tz: &Tz,
    ) -> Option<QuotaPeriod> {
        ALL_PERIODS.into_iter().find(|p| {
            self.limit(*p)
                .is_some_and(|limit| usage.effective_in(*p, now, tz).total() >= limit)
        })
    }
    pub fn any_exceeded(&self, usage: &QuotaUsage, now: DateTime<Utc>) -> bool {
        self.first_exceeded(usage, now).is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaBucket {
    pub period_start: DateTime<Utc>,
    pub input: u64,
    pub output: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
}

impl Default for QuotaBucket {
    fn default() -> Self {
        Self {
            period_start: DateTime::<Utc>::UNIX_EPOCH,
            input: 0,
            output: 0,
            cache_creation: 0,
            cache_read: 0,
        }
    }
}

impl QuotaBucket {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_creation + self.cache_read
    }
    fn zeroed(period_start: DateTime<Utc>) -> Self {
        Self { period_start, ..Default::default() }
    }
}

/// In-memory usage, 4 buckets. Source of truth for the dispatch gate; DB is a restart snapshot.
#[derive(Debug, Clone, Default)]
pub struct QuotaUsage {
    buckets: HashMap<QuotaPeriod, QuotaBucket>,
}

impl QuotaUsage {
    pub fn bucket(&self, p: QuotaPeriod) -> QuotaBucket {
        self.buckets.get(&p).cloned().unwrap_or_default()
    }
    pub fn set_bucket(&mut self, p: QuotaPeriod, b: QuotaBucket) {
        self.buckets.insert(p, b);
    }
    /// Reset every calendar bucket whose period_start no longer matches `now`'s period.
    pub fn roll_if_needed(&mut self, now: DateTime<Utc>) {
        self.roll_if_needed_in(now, &Local)
    }
    pub fn roll_if_needed_in<Tz: TimeZone>(&mut self, now: DateTime<Utc>, tz: &Tz) {
        for p in [QuotaPeriod::Daily, QuotaPeriod::Weekly, QuotaPeriod::Monthly] {
            let start = period_start_in(p, now, tz);
            let cur = self.buckets.entry(p).or_default();
            if cur.period_start != start {
                *cur = QuotaBucket::zeroed(start);
            }
        }
        self.buckets.entry(QuotaPeriod::Total).or_default();
    }
    pub fn add(&mut self, now: DateTime<Utc>, input: u64, output: u64, cache_creation: u64, cache_read: u64) {
        self.add_in(now, &Local, input, output, cache_creation, cache_read)
    }
    pub fn add_in<Tz: TimeZone>(
        &mut self,
        now: DateTime<Utc>,
        tz: &Tz,
        input: u64,
        output: u64,
        cache_creation: u64,
        cache_read: u64,
    ) {
        self.roll_if_needed_in(now, tz);
        for p in ALL_PERIODS {
            let b = self.buckets.entry(p).or_default();
            b.input += input;
            b.output += output;
            b.cache_creation += cache_creation;
            b.cache_read += cache_read;
        }
    }
    /// Read-only view: an expired calendar bucket reads as zero (period rolled but not yet mutated).
    pub fn effective(&self, p: QuotaPeriod, now: DateTime<Utc>) -> QuotaBucket {
        self.effective_in(p, now, &Local)
    }
    pub fn effective_in<Tz: TimeZone>(&self, p: QuotaPeriod, now: DateTime<Utc>, tz: &Tz) -> QuotaBucket {
        let b = self.bucket(p);
        if p == QuotaPeriod::Total {
            return b;
        }
        let start = period_start_in(p, now, tz);
        if b.period_start == start { b } else { QuotaBucket::zeroed(start) }
    }
    pub fn reset_total(&mut self, now: DateTime<Utc>) {
        self.buckets.insert(QuotaPeriod::Total, QuotaBucket::zeroed(now));
    }
}

pub fn period_start(p: QuotaPeriod, now: DateTime<Utc>) -> DateTime<Utc> {
    period_start_in(p, now, &Local)
}

pub fn period_start_in<Tz: TimeZone>(p: QuotaPeriod, now: DateTime<Utc>, tz: &Tz) -> DateTime<Utc> {
    if p == QuotaPeriod::Total {
        return DateTime::<Utc>::UNIX_EPOCH;
    }
    let local = now.with_timezone(tz);
    let date = local.date_naive();
    let day = match p {
        QuotaPeriod::Daily => date,
        QuotaPeriod::Weekly => {
            let back = date.weekday().num_days_from_monday() as i64;
            date - Duration::days(back)
        }
        QuotaPeriod::Monthly => date.with_day(1).expect("day 1 always valid"),
        QuotaPeriod::Total => unreachable!(),
    };
    let midnight = day.and_time(NaiveTime::MIN);
    // DST gap 时 single() 为 None; 取 earliest, 再退回 UTC 解释兜底.
    tz.from_local_datetime(&midnight)
        .earliest()
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|| Utc.from_utc_datetime(&midnight))
}

pub fn period_end(p: QuotaPeriod, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    period_end_in(p, now, &Local)
}

pub fn period_end_in<Tz: TimeZone>(p: QuotaPeriod, now: DateTime<Utc>, tz: &Tz) -> Option<DateTime<Utc>> {
    let start = period_start_in(p, now, tz);
    let start_local = start.with_timezone(tz).date_naive();
    let next = match p {
        QuotaPeriod::Total => return None,
        QuotaPeriod::Daily => start_local + Duration::days(1),
        QuotaPeriod::Weekly => start_local + Duration::days(7),
        QuotaPeriod::Monthly => {
            let (y, m) = (start_local.year(), start_local.month());
            let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
            chrono::NaiveDate::from_ymd_opt(ny, nm, 1).expect("valid first of month")
        }
    };
    let midnight = next.and_time(NaiveTime::MIN);
    Some(
        tz.from_local_datetime(&midnight)
            .earliest()
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|| Utc.from_utc_datetime(&midnight)),
    )
}
```

`src-tauri/src/subscription/mod.rs` 加 `pub mod quota;`（与其他 `pub mod` 并列）。`Weekday` 未用则删掉 import。

- [ ] **Step 4: 运行测试**

Run: `cd src-tauri && cargo test quota::`
Expected: 9 个测试 PASS；`cargo check` 无 warning（未用 import 删掉）

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/subscription/quota.rs src-tauri/src/subscription/mod.rs
git commit -m "feat(quota): 订阅 token 限额纯逻辑模块 (周期边界/用量桶/判定)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: 接进 `SubscriptionRow` / `SubscriptionRuntime` / `is_dispatchable` / store 列读写

**Files:**
- Modify: `src-tauri/src/subscription/model.rs`（`SubscriptionRow` 加字段 ~L237、`SubscriptionRuntime` 加字段 ~L316、`from_row` ~L365、`is_dispatchable` ~L387、`test_fixture` ~L484）
- Modify: `src-tauri/src/subscription/store.rs`（`load_runtime` SELECT 列表 ~L36 与行解析 ~L101、`insert` ~L171、`update_row` ~L258）
- Test: `src-tauri/src/subscription/model.rs` tests

**Interfaces:**
- Consumes: Task 2 的 `TokenQuotas` / `QuotaUsage`。
- Produces: `SubscriptionRow.token_quotas: TokenQuotas`；`SubscriptionRuntime.quota_usage: QuotaUsage`；`is_dispatchable(now)` 已含限额判定。

- [ ] **Step 1: 写失败测试**（追加到 `model.rs` 的 `mod tests`）

```rust
#[test]
fn is_dispatchable_false_when_quota_exceeded() {
    use crate::subscription::quota::{QuotaPeriod, TokenQuotas};
    let mut row = SubscriptionRow::test_fixture("p", "e");
    row.token_quotas = TokenQuotas { total: Some(100), ..Default::default() };
    let mut rt = SubscriptionRuntime::from_row(row);
    let now = Utc::now();
    assert!(rt.is_dispatchable(now));
    rt.quota_usage.add(now, 60, 40, 0, 0); // 恰好 100 → 视为超
    assert!(!rt.is_dispatchable(now));
    // 未设限的订阅不受影响
    let mut row2 = SubscriptionRow::test_fixture("p", "e");
    row2.token_quotas = TokenQuotas::default();
    let mut rt2 = SubscriptionRuntime::from_row(row2);
    rt2.quota_usage.add(now, 1_000_000, 0, 0, 0);
    assert!(rt2.is_dispatchable(now));
    let _ = QuotaPeriod::Total;
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cd src-tauri && cargo test is_dispatchable_false_when_quota_exceeded`
Expected: 编译错误（`token_quotas` / `quota_usage` 字段不存在）

- [ ] **Step 3: 实现**

`model.rs`：
- 顶部 `use crate::subscription::quota::{QuotaUsage, TokenQuotas};`
- `SubscriptionRow` 在 `slot_efforts` 之后加：
  ```rust
  /// 用户设的 token 限额 (cc-router 侧安全阀). 字段缺失 = 该周期不限.
  pub token_quotas: TokenQuotas,
  ```
- `SubscriptionRuntime` 在 `balance_cache` 之后加：
  ```rust
  /// 内存用量计数 (4 周期桶), 判定真值; 启动时由 store::load_quota_usage 装填,
  /// request_log consumer 实时累加, flush 时按快照落 subscription_quota_usage 表.
  pub quota_usage: QuotaUsage,
  ```
- `from_row` 的 `Self { .. }` 加 `quota_usage: QuotaUsage::default(),`
- `is_dispatchable` 在 `true` 之前加：
  ```rust
  if self.row.token_quotas.any_exceeded(&self.quota_usage, now) {
      return false;
  }
  ```
- `test_fixture` 加 `token_quotas: TokenQuotas::default(),`

`store.rs`：
- `load_runtime` 的 SELECT 列表末尾加 `, token_quotas`；行解析处仿 `slot_efforts` 写：
  ```rust
  let token_quotas_json: String = row.try_get("token_quotas")?;
  let token_quotas: TokenQuotas = match serde_json::from_str::<TokenQuotas>(&token_quotas_json) {
      Ok(v) => v,
      Err(e) => {
          warn!(error = %e, raw = %token_quotas_json, "token_quotas JSON 解析失败, 该订阅视为未设限");
          TokenQuotas::default()
      }
  };
  ```
  并在构造 `SubscriptionRow { .. }` 处加 `token_quotas,`。
- `insert`：INSERT 列表加 `token_quotas`，值 `serde_json::to_string(&sub.token_quotas)?`（占位符数量同步 +1）。
- `update_row`：SET 加 `token_quotas = ?,`（放在 `slot_efforts = ?,` 之后），bind 顺序对应位置加 `.bind(token_quotas_json)`。
- 其他构造 `SubscriptionRow { .. }` 的地方（`commands/subscriptions.rs` create 两处 ~L234 / ~L348）加 `token_quotas: TokenQuotas::default(),`；用 `cargo check` 找全。

- [ ] **Step 4: 运行测试**

Run: `cd src-tauri && cargo test`
Expected: 全绿（含 `subscription::` 既有测试）

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/subscription/model.rs src-tauri/src/subscription/store.rs src-tauri/src/commands/subscriptions.rs
git commit -m "feat(quota): SubscriptionRow.token_quotas + Runtime.quota_usage, 超限不可调度

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: store 用量快照读写

**Files:**
- Modify: `src-tauri/src/subscription/store.rs`（文件末尾追加三个函数 + tests）

**Interfaces:**
- Produces:
  ```rust
  pub struct QuotaUsageRow { pub subscription_id: Uuid, pub period: QuotaPeriod, pub bucket: QuotaBucket }
  pub async fn load_quota_usage(pool) -> AppResult<HashMap<Uuid, QuotaUsage>>   // 装填时已 roll_if_needed(now)
  pub async fn save_quota_usage_rows(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, rows: &[QuotaUsageRow], now_ms: i64) -> Result<(), sqlx::Error>  // 快照 UPSERT
  pub async fn save_quota_usage_snapshot(pool, subscription_id, usage: &QuotaUsage) -> AppResult<()>  // 单订阅 4 行, 立即落库 (reset 用)
  ```

- [ ] **Step 1: 写失败测试**（`store.rs` 若无 `mod tests` 则新建；pool 构造照 `request_log.rs::fresh_pool`）

```rust
#[cfg(test)]
mod quota_tests {
    use super::*;
    use crate::db::run_migrations;
    use crate::subscription::quota::{QuotaBucket, QuotaPeriod, QuotaUsage};
    use chrono::{TimeZone, Utc};
    use sqlx::sqlite::SqlitePoolOptions;
    use std::path::PathBuf;

    async fn fresh_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        run_migrations(&pool, &PathBuf::from(".")).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn snapshot_roundtrip_and_upsert() {
        let pool = fresh_pool().await;
        let id = Uuid::new_v4();
        let now = Utc::now();
        let mut u = QuotaUsage::default();
        u.add(now, 1, 2, 3, 4);
        save_quota_usage_snapshot(&pool, &id, &u).await.unwrap();
        // 再写一次 (upsert), 值覆盖
        u.add(now, 1, 0, 0, 0);
        save_quota_usage_snapshot(&pool, &id, &u).await.unwrap();
        let loaded = load_quota_usage(&pool).await.unwrap();
        let got = loaded.get(&id).expect("row loaded");
        assert_eq!(got.bucket(QuotaPeriod::Total).input, 2);
        assert_eq!(got.bucket(QuotaPeriod::Total).total(), 11);
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM subscription_quota_usage").fetch_one(&pool).await.unwrap();
        assert_eq!(n, 4);
    }

    #[tokio::test]
    async fn load_rolls_expired_calendar_buckets() {
        let pool = fresh_pool().await;
        let id = Uuid::new_v4();
        // 手工塞一个 period_start 在很久以前的 daily 桶
        let mut u = QuotaUsage::default();
        u.set_bucket(QuotaPeriod::Daily, QuotaBucket {
            period_start: Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
            input: 999, output: 0, cache_creation: 0, cache_read: 0,
        });
        u.set_bucket(QuotaPeriod::Total, QuotaBucket { input: 999, ..Default::default() });
        save_quota_usage_snapshot(&pool, &id, &u).await.unwrap();
        let loaded = load_quota_usage(&pool).await.unwrap();
        let got = loaded.get(&id).unwrap();
        assert_eq!(got.bucket(QuotaPeriod::Daily).total(), 0, "过期 daily 桶装填时清零");
        assert_eq!(got.bucket(QuotaPeriod::Total).total(), 999, "total 永不滚动");
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cd src-tauri && cargo test quota_tests`
Expected: 编译错误（函数不存在）

- [ ] **Step 3: 实现**（追加到 `store.rs`）

```rust
use crate::subscription::quota::{QuotaBucket, QuotaPeriod, QuotaUsage, ALL_PERIODS};

/// One (subscription, period) row of `subscription_quota_usage`.
#[derive(Debug, Clone)]
pub struct QuotaUsageRow {
    pub subscription_id: Uuid,
    pub period: QuotaPeriod,
    pub bucket: QuotaBucket,
}

pub fn usage_to_rows(subscription_id: Uuid, usage: &QuotaUsage) -> Vec<QuotaUsageRow> {
    ALL_PERIODS
        .into_iter()
        .map(|p| QuotaUsageRow { subscription_id, period: p, bucket: usage.bucket(p) })
        .collect()
}

/// Load every subscription's usage; expired calendar buckets are rolled to zero on load
/// (covers app downtime crossing a period boundary).
pub async fn load_quota_usage(pool: &SqlitePool) -> AppResult<HashMap<Uuid, QuotaUsage>> {
    let rows = sqlx::query(
        "SELECT subscription_id, period, period_start_ms, input_tokens, output_tokens,
                cache_creation_tokens, cache_read_tokens
         FROM subscription_quota_usage",
    )
    .fetch_all(pool)
    .await?;
    let mut out: HashMap<Uuid, QuotaUsage> = HashMap::new();
    for row in rows {
        let id_str: String = row.try_get("subscription_id")?;
        let Ok(id) = Uuid::parse_str(&id_str) else { continue };
        let period_str: String = row.try_get("period")?;
        let Some(period) = QuotaPeriod::parse(&period_str) else { continue };
        let start_ms: i64 = row.try_get("period_start_ms")?;
        let bucket = QuotaBucket {
            period_start: DateTime::<Utc>::from_timestamp_millis(start_ms).unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
            input: row.try_get::<i64, _>("input_tokens")?.max(0) as u64,
            output: row.try_get::<i64, _>("output_tokens")?.max(0) as u64,
            cache_creation: row.try_get::<i64, _>("cache_creation_tokens")?.max(0) as u64,
            cache_read: row.try_get::<i64, _>("cache_read_tokens")?.max(0) as u64,
        };
        out.entry(id).or_default().set_bucket(period, bucket);
    }
    let now = Utc::now();
    for u in out.values_mut() {
        u.roll_if_needed(now);
    }
    Ok(out)
}

/// Snapshot UPSERT inside a caller-owned transaction (used by request_log flush).
pub async fn save_quota_usage_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    rows: &[QuotaUsageRow],
    now_ms: i64,
) -> Result<(), sqlx::Error> {
    for r in rows {
        sqlx::query(
            "INSERT INTO subscription_quota_usage
               (subscription_id, period, period_start_ms, input_tokens, output_tokens,
                cache_creation_tokens, cache_read_tokens, updated_at_ms)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(subscription_id, period) DO UPDATE SET
               period_start_ms = excluded.period_start_ms,
               input_tokens = excluded.input_tokens,
               output_tokens = excluded.output_tokens,
               cache_creation_tokens = excluded.cache_creation_tokens,
               cache_read_tokens = excluded.cache_read_tokens,
               updated_at_ms = excluded.updated_at_ms",
        )
        .bind(r.subscription_id.to_string())
        .bind(r.period.as_str())
        .bind(r.bucket.period_start.timestamp_millis())
        .bind(r.bucket.input as i64)
        .bind(r.bucket.output as i64)
        .bind(r.bucket.cache_creation as i64)
        .bind(r.bucket.cache_read as i64)
        .bind(now_ms)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Immediate single-subscription snapshot (manual total reset path).
pub async fn save_quota_usage_snapshot(
    pool: &SqlitePool,
    subscription_id: &Uuid,
    usage: &QuotaUsage,
) -> AppResult<()> {
    let rows = usage_to_rows(*subscription_id, usage);
    let mut tx = pool.begin().await?;
    save_quota_usage_rows(&mut tx, &rows, Utc::now().timestamp_millis()).await?;
    tx.commit().await?;
    Ok(())
}
```

需要 `use std::collections::HashMap;`、`use chrono::{DateTime, Utc};`（若文件里没有）。`delete()` 里顺手加一句 `sqlx::query("DELETE FROM subscription_quota_usage WHERE subscription_id = ?").bind(id.to_string()).execute(pool).await?;`（订阅删除时清理用量行）。

- [ ] **Step 4: 运行测试**

Run: `cd src-tauri && cargo test quota_tests`
Expected: 2 PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/subscription/store.rs
git commit -m "feat(quota): 用量快照表读写 (load / upsert / 单订阅立即落库)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: request_log consumer 实时累加 + flush 落库 + 达标事件

**Files:**
- Modify: `src-tauri/src/observability/request_log.rs`（`run_consumer` ~L114、`flush` ~L150、`flush_batch` ~L184）
- Modify: `src-tauri/src/observability/events.rs`（`EventKind` 加 `QuotaReached`）
- Test: `request_log.rs` tests

**Interfaces:**
- Consumes: Task 3 的 `SubscriptionRuntime.quota_usage`、Task 4 的 `save_quota_usage_rows` / `usage_to_rows` / `QuotaUsageRow`。
- Produces:
  ```rust
  pub fn apply_entry_to_quota(rt: &mut SubscriptionRuntime, entry: &RequestLogEntry, now: DateTime<Utc>) -> Option<QuotaPeriod>  // 返回本次首次跨过的周期
  pub async fn run_consumer(pool, rx, app, subscriptions: Arc<RwLock<HashMap<Uuid, Arc<RwLock<SubscriptionRuntime>>>>>, event_tx: mpsc::Sender<EventEntry>)
  pub(crate) async fn flush_batch(pool, batch, quota_rows: Vec<QuotaUsageRow>) -> Result<(), FlushError>
  ```
- 前端事件名：`app.emit("subscription_quota_reached", { subscription_id, period })`。

- [ ] **Step 1: 写失败测试**（追加到 `request_log.rs` 的 `mod tests`；复用已有 `fresh_pool` / `make_entry`）

```rust
#[test]
fn apply_entry_to_quota_accumulates_and_reports_first_crossing() {
    use crate::subscription::model::{SubscriptionRow, SubscriptionRuntime};
    use crate::subscription::quota::{QuotaPeriod, TokenQuotas};
    let mut row = SubscriptionRow::test_fixture("p", "e");
    row.token_quotas = TokenQuotas { daily: Some(150), ..Default::default() };
    let mut rt = SubscriptionRuntime::from_row(row);
    let now = Utc::now();
    let sub = rt.row.id;
    // make_entry(ts, vm, sub, provider, status, latency, input, output) —— 见现有工厂签名; 若参数不同按实际改
    let mut e = make_entry(now.timestamp_millis(), VirtualModelName::Sonnet, sub, "p", RequestStatus::Success, Some(1), Some(100), Some(20));
    e.upstream_cache_creation = Some(0);
    e.upstream_cache_read = Some(0);
    assert_eq!(apply_entry_to_quota(&mut rt, &e, now), None); // 120 < 150
    assert_eq!(rt.quota_usage.bucket(QuotaPeriod::Daily).total(), 120);
    assert_eq!(apply_entry_to_quota(&mut rt, &e, now), Some(QuotaPeriod::Daily)); // 240 >= 150, 首次跨
    assert_eq!(apply_entry_to_quota(&mut rt, &e, now), None); // 已达标, 不再重复报
    // usage 为 None 的 entry (失败请求) 不改变计数
    let mut e2 = e.clone();
    e2.upstream_input_tokens = None; e2.upstream_output_tokens = None;
    let before = rt.quota_usage.bucket(QuotaPeriod::Daily).total();
    apply_entry_to_quota(&mut rt, &e2, now);
    assert_eq!(rt.quota_usage.bucket(QuotaPeriod::Daily).total(), before);
}

#[tokio::test]
async fn flush_batch_persists_quota_rows_in_same_tx() {
    use crate::subscription::quota::{QuotaPeriod, QuotaUsage};
    use crate::subscription::store::usage_to_rows;
    let pool = fresh_pool().await;
    let sub = Uuid::new_v4();
    let now = Utc::now();
    let mut u = QuotaUsage::default();
    u.add(now, 5, 6, 7, 8);
    let rows = usage_to_rows(sub, &u);
    flush_batch(&pool, vec![], rows).await.expect("flush ok");
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM subscription_quota_usage").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 4);
    let total: i64 = sqlx::query_scalar(
        "SELECT input_tokens + output_tokens + cache_creation_tokens + cache_read_tokens
         FROM subscription_quota_usage WHERE subscription_id = ? AND period = ?")
        .bind(sub.to_string()).bind(QuotaPeriod::Weekly.as_str())
        .fetch_one(&pool).await.unwrap();
    assert_eq!(total, 26);
}
```

`RequestLogEntry` 若未 `derive(Clone)` 则加上（测试用 `e.clone()`）。现有调用 `flush_batch(&pool, batch)` 的测试统一加第三个参数 `vec![]`。

- [ ] **Step 2: 运行确认失败**

Run: `cd src-tauri && cargo test request_log::`
Expected: 编译错误

- [ ] **Step 3: 实现**

`events.rs`：`EventKind` 加 `QuotaReached`，`as_str` 加 `Self::QuotaReached => "quota_reached"`；模块顶部注释加一行 `- quota_reached  订阅 token 用量首次达到用户设的限额`。

`request_log.rs`：

```rust
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::observability::events::{EventEntry, EventKind, Severity};
use crate::subscription::model::SubscriptionRuntime;
use crate::subscription::quota::QuotaPeriod;
use crate::subscription::store::{usage_to_rows, save_quota_usage_rows, QuotaUsageRow};

/// Add one finished request's usage to the subscription's in-memory quota buckets.
/// Returns the first period that crossed its limit *because of this entry* (for one-shot alerting).
pub fn apply_entry_to_quota(
    rt: &mut SubscriptionRuntime,
    entry: &RequestLogEntry,
    now: DateTime<Utc>,
) -> Option<QuotaPeriod> {
    let input = entry.upstream_input_tokens.unwrap_or(0) as u64;
    let output = entry.upstream_output_tokens.unwrap_or(0) as u64;
    let cc = entry.upstream_cache_creation.unwrap_or(0) as u64;
    let cr = entry.upstream_cache_read.unwrap_or(0) as u64;
    if input + output + cc + cr == 0 {
        return None;
    }
    let before = rt.row.token_quotas.first_exceeded(&rt.quota_usage, now);
    rt.quota_usage.add(now, input, output, cc, cr);
    let after = rt.row.token_quotas.first_exceeded(&rt.quota_usage, now);
    match (before, after) {
        (None, Some(p)) => Some(p),
        _ => None,
    }
}
```

`run_consumer` 签名改为：

```rust
pub async fn run_consumer(
    pool: SqlitePool,
    mut rx: mpsc::Receiver<RequestLogEntry>,
    app: AppHandle,
    subscriptions: Arc<RwLock<HashMap<Uuid, Arc<RwLock<SubscriptionRuntime>>>>>,
    event_tx: mpsc::Sender<EventEntry>,
)
```

循环体里 `Some(entry) =>` 分支开头（push_back 之前）加：

```rust
let rt = subscriptions.read().await.get(&entry.subscription_id).cloned();
if let Some(rt) = rt {
    let now = Utc::now();
    let crossed = {
        let mut g = rt.write().await;
        apply_entry_to_quota(&mut g, &entry, now)
    };
    dirty.insert(entry.subscription_id);
    if let Some(period) = crossed {
        let display_name = rt.read().await.row.display_name.clone();
        let ev = EventEntry {
            id: Uuid::new_v4(),
            timestamp_ms: now.timestamp_millis(),
            kind: EventKind::QuotaReached,
            severity: Severity::Warn,
            subscription_id: Some(entry.subscription_id),
            request_id: Some(entry.id),
            summary: format!("{display_name} 已达 {} token 限额, 暂停调度至下一周期", period.label_zh()),
            payload: Some(serde_json::json!({ "period": period.as_str() })),
        };
        let _ = event_tx.try_send(ev);
        let _ = app.emit(
            "subscription_quota_reached",
            serde_json::json!({ "subscription_id": entry.subscription_id.to_string(), "period": period.as_str() }),
        );
    }
}
```

`let mut dirty: HashSet<Uuid> = HashSet::new();` 在 loop 之前声明。`flush(...)` 三处调用改成 `flush(&pool, &mut buffer, &app, &subscriptions, &mut dirty).await`，`flush` 内部在 drain batch 后：

```rust
let mut quota_rows: Vec<QuotaUsageRow> = Vec::new();
{
    let map = subscriptions.read().await;
    for id in dirty.drain() {
        if let Some(rt) = map.get(&id) {
            let g = rt.read().await;
            quota_rows.extend(usage_to_rows(id, &g.quota_usage));
        }
    }
}
match flush_batch(pool, batch, quota_rows).await { ... }   // BeginFailed 分支里 dirty 无法退还, 下一次 entry 会再次标脏, 可接受
```

`flush_batch(pool, batch, quota_rows)`：在 `tx.commit()` 之前加

```rust
if let Err(e) = save_quota_usage_rows(&mut tx, &quota_rows, now_ms()).await {
    warn!(?e, "写 subscription_quota_usage 快照失败 (局部丢, 下次 flush 补写)");
}
```

（`flush_batch` 目前"batch 为空直接 return"的早退若存在，改成 `batch.is_empty() && quota_rows.is_empty()` 才早退。）

- [ ] **Step 4: 运行测试**

Run: `cd src-tauri && cargo test request_log::`
Expected: 全部 PASS（此时 `lib.rs` 调用点编译失败属预期，Task 6 修）。若 lib.rs 编译错误阻塞 `cargo test`，先在 lib.rs 调用处临时传 `Arc::new(RwLock::new(HashMap::new()))` 和一个新的 `mpsc::channel(1).0` 占位，Task 6 再换成真值。

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/observability/request_log.rs src-tauri/src/observability/events.rs src-tauri/src/lib.rs
git commit -m "feat(quota): request_log consumer 实时累加用量 + flush 快照落库 + 达标事件

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: bootstrap 装配

**Files:**
- Modify: `src-tauri/src/lib.rs:218-240`

**Interfaces:**
- Consumes: Task 4 `load_quota_usage`，Task 5 新 `run_consumer` 签名。

- [ ] **Step 1: 改 bootstrap 顺序**

在 `// 4. 订阅运行时状态初始化` 的 `load_runtime` 之后加：

```rust
// 4b. 装填 token 限额用量 (内存为真值, 表只做重启恢复)
{
    let usage_map = subscription::store::load_quota_usage(&pool).await?;
    for (id, rt) in subscription_map.iter() {
        if let Some(u) = usage_map.get(id) {
            rt.write().await.quota_usage = u.clone();
        }
    }
}
```

（`subscription_map` 的实际类型看 `load_runtime` 返回值：若是 `HashMap<Uuid, Arc<RwLock<SubscriptionRuntime>>>` 就如上写；若已经包了 `Arc<RwLock<..>>` 就先 `.read().await` 再迭代。）

把 `// 6b. 事件流 channel` 那段**上移到 `// 6. 请求日志 channel` 之前**（request_log consumer 需要 `event_tx`），然后 request_log 的 spawn 改为：

```rust
let log_subs = subscriptions_arc.clone();   // 即后面塞进 AppState.subscriptions 的那个 Arc<RwLock<HashMap>>; 若此处尚未包 Arc, 先在这里包好并复用到 AppState
let log_event_tx = event_tx.clone();
tauri::async_runtime::spawn(async move {
    observability::request_log::run_consumer(log_pool, log_rx, log_handle, log_subs, log_event_tx).await;
});
```

- [ ] **Step 2: 编译 + 全量测试**

Run: `cd src-tauri && cargo check && cargo test`
Expected: 无错误、全绿

- [ ] **Step 3: 手动冒烟**

Run: `pnpm tauri dev`，用任意订阅发一条请求（`curl -s 127.0.0.1:23456/v1/messages -H 'content-type: application/json' -d '{"model":"model-haiku","max_tokens":16,"messages":[{"role":"user","content":"hi"}]}'`），5s 后：
`sqlite3 "$HOME/Library/Application Support/com.cc-router.desktop/config.db" "select period, input_tokens, output_tokens from subscription_quota_usage"` 应有 4 行且数字非零。

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat(quota): bootstrap 装填用量并接入日志 consumer

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 7: DTO + commands + 503 摘要

**Files:**
- Modify: `src-tauri/src/subscription/model.rs`（`SubscriptionDto` ~L404、`from_runtime` ~L521）
- Modify: `src-tauri/src/commands/subscriptions.rs`（新增两个 command）
- Modify: `src-tauri/src/lib.rs:109` `generate_handler!` 列表
- Modify: `src-tauri/src/proxy/pipeline.rs:150-160`（503 摘要）
- Test: `model.rs` tests

**Interfaces:**
- Produces:
  ```rust
  pub struct QuotaUsageDto { period: QuotaPeriod, limit: Option<u64>, input, output, cache_creation, cache_read: u64, period_start_ms: i64, period_end_ms: Option<i64>, exceeded: bool }
  SubscriptionDto { token_quotas: TokenQuotas, quota_usage: Vec<QuotaUsageDto>, .. }
  #[tauri::command] update_token_quotas(id: String, quotas: TokenQuotas) -> SubscriptionDto
  #[tauri::command] reset_total_quota_usage(id: String) -> SubscriptionDto
  ```

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn dto_exposes_four_quota_periods_with_exceeded_flag() {
    use crate::subscription::quota::{QuotaPeriod, TokenQuotas};
    let mut row = SubscriptionRow::test_fixture("p", "e");
    row.token_quotas = TokenQuotas { weekly: Some(10), ..Default::default() };
    let mut rt = SubscriptionRuntime::from_row(row);
    rt.quota_usage.add(Utc::now(), 10, 0, 0, 0);
    let dto = SubscriptionDto::from_runtime(&rt, vec![]);
    assert_eq!(dto.quota_usage.len(), 4);
    let weekly = dto.quota_usage.iter().find(|q| q.period == QuotaPeriod::Weekly).unwrap();
    assert_eq!(weekly.limit, Some(10));
    assert!(weekly.exceeded);
    assert!(weekly.period_end_ms.is_some());
    let total = dto.quota_usage.iter().find(|q| q.period == QuotaPeriod::Total).unwrap();
    assert!(total.period_end_ms.is_none());
    assert!(!total.exceeded);
    assert_eq!(dto.token_quotas.weekly, Some(10));
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cd src-tauri && cargo test dto_exposes_four_quota_periods`
Expected: 编译错误

- [ ] **Step 3: 实现 DTO**

`model.rs`：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaUsageDto {
    pub period: QuotaPeriod,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    pub input: u64,
    pub output: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
    pub period_start_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period_end_ms: Option<i64>,
    pub exceeded: bool,
}
```

`SubscriptionDto` 在 `slot_efforts` 后加：

```rust
/// 用户设的 token 限额; 刻意不 skip: 前端声明为必填.
#[serde(default)]
pub token_quotas: TokenQuotas,
/// 4 个周期的当前用量 (始终 4 项, 顺序 daily/weekly/monthly/total).
#[serde(default)]
pub quota_usage: Vec<QuotaUsageDto>,
```

`from_runtime` 加：

```rust
token_quotas: rt.row.token_quotas.clone(),
quota_usage: {
    let now = Utc::now();
    crate::subscription::quota::ALL_PERIODS.into_iter().map(|p| {
        let b = rt.quota_usage.effective(p, now);
        let limit = rt.row.token_quotas.limit(p);
        QuotaUsageDto {
            period: p,
            limit,
            input: b.input, output: b.output, cache_creation: b.cache_creation, cache_read: b.cache_read,
            period_start_ms: b.period_start.timestamp_millis(),
            period_end_ms: crate::subscription::quota::period_end(p, now).map(|t| t.timestamp_millis()),
            exceeded: limit.is_some_and(|l| b.total() >= l),
        }
    }).collect()
},
```

- [ ] **Step 4: 实现 commands**（`commands/subscriptions.rs` 末尾）

```rust
fn validate_token_quotas(q: &TokenQuotas) -> AppResult<()> {
    for p in crate::subscription::quota::ALL_PERIODS {
        if q.limit(p) == Some(0) {
            return Err(AppError::BadRequest(format!("{} 限额必须大于 0 (不限请留空)", p.label_zh())));
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn update_token_quotas(
    state: State<'_, AppState>,
    id: String,
    quotas: TokenQuotas,
) -> AppResult<SubscriptionDto> {
    let id = Uuid::parse_str(&id).map_err(|_| AppError::BadRequest("无效 id".into()))?;
    validate_token_quotas(&quotas)?;
    let rt = state.get_subscription(&id).await?;   // 若 AppState 无此 helper, 照 update_subscription 开头的读锁写法取
    let row_snapshot = {
        let mut g = rt.write().await;
        g.row.token_quotas = quotas;
        g.row.updated_at = Utc::now();
        g.row.clone()
    };
    store::update_row(&state.db, &row_snapshot).await?;
    let referenced_by = referenced_by_names(&state, &id).await;   // 复用 get_subscription command 里算 referenced_by 的方式
    Ok(SubscriptionDto::from_runtime(&*rt.read().await, referenced_by))
}

#[tauri::command]
pub async fn reset_total_quota_usage(
    state: State<'_, AppState>,
    id: String,
) -> AppResult<SubscriptionDto> {
    let id = Uuid::parse_str(&id).map_err(|_| AppError::BadRequest("无效 id".into()))?;
    let rt = state.get_subscription(&id).await?;
    let usage = {
        let mut g = rt.write().await;
        g.quota_usage.reset_total(Utc::now());
        g.quota_usage.clone()
    };
    store::save_quota_usage_snapshot(&state.db, &id, &usage).await?;
    let referenced_by = referenced_by_names(&state, &id).await;
    Ok(SubscriptionDto::from_runtime(&*rt.read().await, referenced_by))
}
```

`referenced_by_names` 若不存在：看 `get_subscription` command 如何构造 `referenced_by`（遍历 `state.virtual_models` 找包含该 id 的虚拟模型名），抽成私有 helper 并让 `get_subscription` 复用。`lib.rs` `generate_handler!` 加 `commands::subscriptions::update_token_quotas, commands::subscriptions::reset_total_quota_usage,`。

- [ ] **Step 5: 503 摘要区分限额**

`pipeline.rs` 构造 `summary` 处（~L152-158）：

```rust
let now = Utc::now();
for sub_id in &vm_config.subscription_ids {
    if let Some(rt) = subs_map.get(sub_id) {
        let g = rt.read().await;
        let reason = match g.row.token_quotas.first_exceeded(&g.quota_usage, now) {
            Some(p) => format!("已达 {} token 限额", p.label_zh()),
            None => format!("{:?}", g.state),
        };
        summary.push(format!("- {}: {}", g.row.display_name, reason));
    }
}
```

- [ ] **Step 6: 测试 + 检查**

Run: `cd src-tauri && cargo test && cargo check`
Expected: 全绿

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src
git commit -m "feat(quota): DTO 暴露四周期用量 + update_token_quotas / reset_total_quota_usage 命令 + 503 摘要

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 8: 前端 — 类型 / API / 快捷写法解析

**Files:**
- Modify: `src/types.ts`（`SubscriptionDto` ~L235、附近加 `TokenQuotas` / `QuotaPeriod` / `QuotaUsageDto`）
- Modify: `src/api/tauri.ts`（~L58 附近加两个调用）
- Create: `src/lib/quota.ts`
- Modify: `src/hooks/useSubscriptions.ts`（事件桥加 `subscription_quota_reached`）

**Interfaces:**
- Produces:
  ```ts
  export type QuotaPeriod = "daily" | "weekly" | "monthly" | "total";
  export interface TokenQuotas { daily?: number | null; weekly?: number | null; monthly?: number | null; total?: number | null }
  export interface QuotaUsageDto { period: QuotaPeriod; limit?: number; input: number; output: number; cache_creation: number; cache_read: number; period_start_ms: number; period_end_ms?: number; exceeded: boolean }
  SubscriptionDto.token_quotas: TokenQuotas; SubscriptionDto.quota_usage: QuotaUsageDto[]
  api.updateTokenQuotas(id, quotas) / api.resetTotalQuotaUsage(id)
  parseTokenShorthand("5M") → 5_000_000 | null ; formatTokenShorthand(5_000_000) → "5M"
  ```

- [ ] **Step 1: types.ts**

在 `SlotEfforts` 定义附近加上面三个类型；`SubscriptionDto` 加 `token_quotas: TokenQuotas;` 与 `quota_usage: QuotaUsageDto[];`（必填，与后端 `#[serde(default)]` 对应）。

- [ ] **Step 2: api/tauri.ts**

```ts
updateTokenQuotas: (id: string, quotas: TokenQuotas) =>
  invoke<SubscriptionDto>("update_token_quotas", { id, quotas }),
resetTotalQuotaUsage: (id: string) =>
  invoke<SubscriptionDto>("reset_total_quota_usage", { id }),
```

- [ ] **Step 3: lib/quota.ts**

```ts
/** "5M" / "100m" / "2.5B" / "500k" / "1200000" → 整数 token 数; 非法返回 null; 空串返回 undefined (=不限). */
export function parseTokenShorthand(raw: string): number | null | undefined {
  const s = raw.trim().replace(/[,_\s]/g, "");
  if (s === "") return undefined;
  const m = /^(\d+(?:\.\d+)?)([kKmMbB]?)$/.exec(s);
  if (!m) return null;
  const n = parseFloat(m[1]);
  const mult = { "": 1, k: 1e3, m: 1e6, b: 1e9 }[m[2].toLowerCase() as "" | "k" | "m" | "b"];
  const v = Math.round(n * mult);
  return v > 0 && Number.isSafeInteger(v) ? v : null;
}

/** 5_000_000 → "5M"; 1_500_000 → "1.5M"; 800 → "800"; 与 parse 互逆 (小数最多 2 位). */
export function formatTokenShorthand(n: number | null | undefined): string {
  if (n == null) return "";
  const units: Array<[number, string]> = [[1e9, "B"], [1e6, "M"], [1e3, "k"]];
  for (const [base, suf] of units) {
    if (n >= base) {
      const v = n / base;
      return `${Number.isInteger(v) ? v : v.toFixed(2).replace(/\.?0+$/, "")}${suf}`;
    }
  }
  return String(n);
}
```

- [ ] **Step 4: 事件桥**

`useSubscriptions.ts::useSubscriptionEventBridge` 里再 `listen("subscription_quota_reached", () => invalidateSubscriptions(queryClient))`，卸载时同样 unlisten。

- [ ] **Step 5: 类型检查**

Run: `pnpm tsc --noEmit`
Expected: 通过（此时无组件使用新字段，只需类型闭合）

- [ ] **Step 6: Commit**

```bash
git add src/types.ts src/api/tauri.ts src/lib/quota.ts src/hooks/useSubscriptions.ts
git commit -m "feat(quota): 前端类型/API/快捷写法解析/事件桥

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 9: 前端 — 编辑页限额卡片 + 四段进度条卡片 + 列表徽标 + zh 文案

**Files:**
- Create: `src/components/SubscriptionQuotaCard.tsx`
- Modify: `src/routes/SubscriptionEdit.tsx`（在 `<SubscriptionBalanceCard .../>` 之后挂新卡片；「用量限额」输入卡片）
- Modify: `src/routes/Subscriptions.tsx`（`<StatusBadge state={sub.state} />` 旁加限额徽标）
- Modify: `src/i18n/locales/zh.json`

- [ ] **Step 1: zh.json 文案**（在 `subscriptionEdit.*` 邻近位置追加）

```json
"quota.title": "用量限额",
"quota.desc": "cc-router 侧的安全阀：任一周期达到上限即暂停调度该订阅，到下一周期自动恢复。四项 token（输入 / 输出 / 缓存写 / 缓存读）合计。",
"quota.period.daily": "每日",
"quota.period.weekly": "每周",
"quota.period.monthly": "每月",
"quota.period.total": "累计总量",
"quota.unlimited": "不限",
"quota.placeholder": "如 5M / 100M / 2.5B，留空不限",
"quota.invalid": "限额格式不正确（示例：5M、100M、500k）",
"quota.usedOf": "已用 {used} / {limit}",
"quota.resetAt": "于 {time} 恢复",
"quota.exceeded": "已达限额",
"quota.resetTotal": "重置累计计数",
"quota.resetTotalConfirm": "确定把累计总量的计数清零？此操作不可撤销。",
"quota.legend.input": "输入",
"quota.legend.output": "输出",
"quota.legend.cacheCreation": "缓存写",
"quota.legend.cacheRead": "缓存读",
"quota.save": "保存限额",
"quota.noLimits": "未设置限额。在上方设置后这里会显示各周期用量。"
```

- [ ] **Step 2: `SubscriptionQuotaCard.tsx`**

```tsx
import { useState } from "react";
import { Gauge } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { api } from "@/api/tauri";
import { useT } from "@/i18n";
import { cn } from "@/lib/utils";
import { fmtCompact, fmtTimeShort } from "@/lib/format";
import { formatTokenShorthand } from "@/lib/quota";
import type { QuotaUsageDto, SubscriptionDto } from "@/types";

interface Props {
  subscription: SubscriptionDto;
  onChanged?: () => void;
}

const SEGMENTS: Array<{ key: keyof Pick<QuotaUsageDto, "input" | "output" | "cache_creation" | "cache_read">; labelKey: string; className: string }> = [
  { key: "input", labelKey: "quota.legend.input", className: "bg-sky-500" },
  { key: "output", labelKey: "quota.legend.output", className: "bg-emerald-500" },
  { key: "cache_creation", labelKey: "quota.legend.cacheCreation", className: "bg-amber-500" },
  { key: "cache_read", labelKey: "quota.legend.cacheRead", className: "bg-violet-500" },
];

export function SubscriptionQuotaCard({ subscription, onChanged }: Props) {
  const t = useT();
  const [resetting, setResetting] = useState(false);
  const rows = subscription.quota_usage.filter((q) => q.limit != null);

  async function resetTotal() {
    if (!window.confirm(t("quota.resetTotalConfirm"))) return;
    setResetting(true);
    try {
      await api.resetTotalQuotaUsage(subscription.id);
      onChanged?.();
    } finally {
      setResetting(false);
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <Gauge className="h-4 w-4" /> {t("quota.title")}
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        {rows.length === 0 && <p className="text-sm text-muted-foreground">{t("quota.noLimits")}</p>}
        {rows.map((q) => {
          const limit = q.limit!;
          const used = q.input + q.output + q.cache_creation + q.cache_read;
          const ratio = Math.min(used / limit, 1);
          const warn = ratio >= 0.8;
          return (
            <div key={q.period} className="space-y-1">
              <div className="flex items-center justify-between text-sm">
                <span className="font-medium">{t(`quota.period.${q.period}`)}</span>
                <span className={cn("text-muted-foreground", q.exceeded && "text-red-600 font-medium")}>
                  {q.exceeded
                    ? t("quota.exceeded")
                    : t("quota.usedOf", { used: fmtCompact(used), limit: formatTokenShorthand(limit) })}
                  {q.period_end_ms != null && !q.exceeded ? "" : ""}
                </span>
              </div>
              <div
                className={cn(
                  "flex h-3 w-full overflow-hidden rounded-full bg-muted",
                  warn && !q.exceeded && "ring-1 ring-amber-500",
                  q.exceeded && "ring-1 ring-red-500",
                )}
                title={SEGMENTS.map((s) => `${t(s.labelKey)} ${fmtCompact(q[s.key])}`).join(" · ")}
              >
                {SEGMENTS.map((s) => (
                  <div
                    key={s.key}
                    className={cn("h-full", s.className)}
                    style={{ width: `${(Math.min(q[s.key], limit) / limit) * 100}%` }}
                  />
                ))}
              </div>
              <div className="flex items-center justify-between text-xs text-muted-foreground">
                <span className="flex gap-3">
                  {SEGMENTS.map((s) => (
                    <span key={s.key} className="flex items-center gap-1">
                      <i className={cn("inline-block h-2 w-2 rounded-sm", s.className)} />
                      {t(s.labelKey)} {fmtCompact(q[s.key])}
                    </span>
                  ))}
                </span>
                {q.period_end_ms != null ? (
                  <span>{t("quota.resetAt", { time: fmtTimeShort(q.period_end_ms) })}</span>
                ) : (
                  <Button variant="outline" size="sm" disabled={resetting} onClick={resetTotal}>
                    {t("quota.resetTotal")}
                  </Button>
                )}
              </div>
            </div>
          );
        })}
      </CardContent>
    </Card>
  );
}
```

（四段宽度各自按 `min(值, limit)/limit` 计算；四段之和 = used/limit，超过 100% 时被容器 `overflow-hidden` 裁掉，视觉即「满格」。颜色类若项目 Tailwind 配置未启用对应色，换成项目已用的语义色或内联 `style.background`；`useT` 的插值签名 `t(key, vars)` 以 `src/i18n/index.tsx` 实际为准。）

- [ ] **Step 3: SubscriptionEdit.tsx 编辑卡片**

state：`const [quotaInputs, setQuotaInputs] = useState<Record<QuotaPeriod, string>>({ daily: "", weekly: "", monthly: "", total: "" });` `const [quotaError, setQuotaError] = useState<string | null>(null);`
加载 `subQuery.data` 时（现有 `setSlotEfforts(...)` 旁）：

```ts
const q = subQuery.data.token_quotas ?? {};
setQuotaInputs({
  daily: formatTokenShorthand(q.daily), weekly: formatTokenShorthand(q.weekly),
  monthly: formatTokenShorthand(q.monthly), total: formatTokenShorthand(q.total),
});
```

保存函数（独立按钮，不混进 `save()` 的 patch，因为走独立 command）：

```ts
async function saveQuotas() {
  if (!id) return;
  setQuotaError(null);
  const out: TokenQuotas = {};
  for (const p of ["daily", "weekly", "monthly", "total"] as QuotaPeriod[]) {
    const v = parseTokenShorthand(quotaInputs[p]);
    if (v === null) return setQuotaError(t("quota.invalid"));
    if (v !== undefined) out[p] = v;
  }
  await api.updateTokenQuotas(id, out);
  queryClient.invalidateQueries({ queryKey: ["subscription", id] });
  queryClient.invalidateQueries({ queryKey: ["subscriptions"] });
}
```

JSX：紧接 `<SubscriptionBalanceCard .../>` 之后：

```tsx
<Card>
  <CardHeader><CardTitle>{t("quota.title")}</CardTitle></CardHeader>
  <CardContent className="space-y-3">
    <p className="text-sm text-muted-foreground">{t("quota.desc")}</p>
    {(["daily", "weekly", "monthly", "total"] as QuotaPeriod[]).map((p) => (
      <div key={p} className="flex items-center gap-3">
        <label className="w-24 text-sm">{t(`quota.period.${p}`)}</label>
        <input
          className="input flex-1"
          placeholder={t("quota.placeholder")}
          value={quotaInputs[p]}
          onChange={(e) => setQuotaInputs({ ...quotaInputs, [p]: e.target.value })}
        />
      </div>
    ))}
    {quotaError && <p className="text-sm text-red-600">{quotaError}</p>}
    <Button size="sm" onClick={saveQuotas}>{t("quota.save")}</Button>
  </CardContent>
</Card>
<SubscriptionQuotaCard
  subscription={sub}
  onChanged={() => {
    queryClient.invalidateQueries({ queryKey: ["subscription", id] });
    queryClient.invalidateQueries({ queryKey: ["subscriptions"] });
  }}
/>
```

（`input` 的 className 用页面里现有输入框同款 class；查看 `displayName` 输入框写法照抄。）

- [ ] **Step 4: 列表徽标**

`Subscriptions.tsx` 在 `<StatusBadge state={sub.state} />` 后：

```tsx
{sub.quota_usage?.some((q) => q.exceeded) && (
  <span className="rounded-full bg-amber-100 px-2 py-0.5 text-xs text-amber-800">{t("quota.exceeded")}</span>
)}
```

- [ ] **Step 5: 类型检查 + 目测**

Run: `pnpm tsc --noEmit`；`pnpm tauri dev` 打开某订阅编辑页：设 `daily=1k` 保存 → 进度条出现；发几条请求 → 进度条四色增长、达标后列表出现「已达限额」、该订阅不再被调度（Live Routing 页 / 503 摘要可见「已达 每日 token 限额」）；清空输入保存 → 卡片显示「未设置限额」。

- [ ] **Step 6: Commit**

```bash
git add src/components/SubscriptionQuotaCard.tsx src/routes/SubscriptionEdit.tsx src/routes/Subscriptions.tsx src/i18n/locales/zh.json
git commit -m "feat(quota): 订阅限额编辑卡片 + 四段堆叠用量进度条 + 列表徽标 (zh)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 10: 文档

**Files:**
- Modify: `CLAUDE.md`（「隐藏约束 → SQL & DB」与「三个核心抽象」附近）

- [ ] **Step 1: CLAUDE.md 补两条**

在 Subscription 抽象说明后加一句：「订阅可设 `token_quotas`（daily/weekly/monthly/total 四槽 JSON 列，migration 017），任一周期用量 ≥ 上限即 `is_dispatchable=false`，到下一周期自动恢复；不是 `SubscriptionState`，不走冷却。」

在「隐藏约束 → SQL & DB」加：「**`subscription_quota_usage` 是快照表，内存 `SubscriptionRuntime.quota_usage` 才是判定真值**：`request_log::run_consumer` 收到 entry 即累加内存，flush 时按快照 UPSERT；周期边界按**本地时区**（所以没复用按 UTC 切桶的 `request_stats_daily`）。加新 dispatch 路径只要保证用量进 `RequestLogEntry` 就自动纳入限额。」

- [ ] **Step 2: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: CLAUDE.md 补订阅 token 限额约束

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## 自检记录

- Spec 覆盖：A.1 语义 → T2/T3；A.2 数据模型 → T1/T2/T3；A.3 计量落库 → T4/T5/T6；A.4 判定 + 503 摘要 → T3/T7；A.5 命令与 DTO → T7/T8；A.6 前端 → T8/T9（toast 收窄为事件 + 徽标，已在全局约束说明）；A.7 测试 → 各任务 Step 1；文档 → T10。
- 类型一致性：`QuotaPeriod::label_zh` / `as_str` / `parse`、`QuotaUsage::{bucket,set_bucket,add,add_in,effective,effective_in,roll_if_needed,roll_if_needed_in,reset_total}`、`TokenQuotas::{limit,is_empty,first_exceeded,first_exceeded_in,any_exceeded}`、`store::{load_quota_usage,save_quota_usage_rows,save_quota_usage_snapshot,usage_to_rows,QuotaUsageRow}`、`request_log::{apply_entry_to_quota,run_consumer,flush_batch}` 在各任务间名称一致。
- 已知未覆盖：pipeline 层没有可构造 `AppState` 的集成测试基座（需要 `AppHandle`），端到端靠 T6/T9 的手动冒烟。
