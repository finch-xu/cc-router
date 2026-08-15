# 订阅 token 限额 + 会话亲和调度 — 设计稿

日期：2026-08-15
状态：已与维护者对齐，待实施
范围：两项互相独立的功能，共用「订阅可调度性」谓词这一个交汇点。

- A. **订阅 token 限额**（用户在 cc-router 侧给每条订阅设 token 用量上限，按日/周/月/总量）
- B. **会话亲和调度**（`RoutingMode` 第三种模式 `sticky`：同一会话钉住同一订阅，保住 prompt cache）

明确不做（本轮已否决）：代理与 UI 解耦、`/v1/chat/completions` 入站。

---

## A. 订阅 token 限额

### A.1 目标与语义

- 每条订阅可设最多 4 条限额：`daily` / `weekly` / `monthly` / `total`，每槽可空（空 = 不限）。
- 计量口径：**四项总和** = `input + output + cache_creation + cache_read`（与统计页「总 token」一致）。
- 周期边界按**本地时区日历**：daily = 本地当日 0 点；weekly = 本地周一 0 点；monthly = 本地当月 1 号 0 点；`total` 不自动重置，仅用户手动重置。
- 任一周期 `used ≥ limit` → 该订阅**不可调度**（`is_dispatchable=false`），到下一周期自动恢复。
- **软限**：只在调度时判定，已发出的请求正常完成，超出量 ≤ 最后一次请求的用量。
- 所有虚拟模型（含 fallback）一视同仁，因为限额挂在订阅上。
- 四个周期的用量**始终计量**，无论是否设了限额（用户中途设限时数字已就位）。

### A.2 数据模型

**migration 017**（一个文件 `017_add_token_quotas.sql`，两句 SQL，遵循「注释里不写 `;`」）：

```sql
ALTER TABLE subscriptions ADD COLUMN token_quotas TEXT NOT NULL DEFAULT '{}';
```

```sql
CREATE TABLE subscription_quota_usage (
  subscription_id  TEXT    NOT NULL,
  period           TEXT    NOT NULL,   -- daily / weekly / monthly / total
  period_start_ms  INTEGER NOT NULL,   -- 该周期起点(本地时区换算成 Unix ms); total = 上次重置时刻
  input_tokens          INTEGER NOT NULL DEFAULT 0,
  output_tokens         INTEGER NOT NULL DEFAULT 0,
  cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens     INTEGER NOT NULL DEFAULT 0,
  updated_at_ms    INTEGER NOT NULL,
  PRIMARY KEY (subscription_id, period)
);
```

为什么不复用 `request_stats_daily`：它按 **UTC** 日切桶，本地时区的日/周/月边界对不齐（北京用户会在早 8 点重置）。

Rust 侧：

```rust
// subscription/model.rs
#[derive(Default, Serialize, Deserialize)]  // 照 SlotEfforts 的模式: 全 Option, unknown 字段忽略, skip None
pub struct TokenQuotas { pub daily: Option<u64>, pub weekly: Option<u64>, pub monthly: Option<u64>, pub total: Option<u64> }

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaPeriod { Daily, Weekly, Monthly, Total }

#[derive(Default, Clone)]
pub struct QuotaBucket { pub period_start: DateTime<Utc>, pub input: u64, pub output: u64, pub cache_creation: u64, pub cache_read: u64 }
impl QuotaBucket { pub fn total(&self) -> u64 }

#[derive(Default, Clone)]
pub struct QuotaUsage { buckets: HashMap<QuotaPeriod, QuotaBucket> }  // 4 桶, 缺省视为 0
```

`SubscriptionRow` 加 `token_quotas: TokenQuotas`；`SubscriptionRuntime` 加 `quota_usage: QuotaUsage`。

`period_start` 计算放纯函数 `quota::period_start(period, now_local: DateTime<Local>) -> DateTime<Utc>`，测试用 `FixedOffset` 注入时区避免依赖机器时区。`Total` 的 `period_start` 来自 DB 行（首次为 0 时刻，即 UNIX epoch），重置命令改写为「现在」。

### A.3 计量与落库（单一汇聚点）

所有 7 条 dispatch 路径 + SSE 都把用量装进 `RequestLogEntry` 投给 `request_log_tx`，因此计量钩子放在 **`request_log::run_consumer` 收到 entry 的瞬间**（不是 flush 时）：

1. `run_consumer` 新增入参 `subscriptions: Arc<RwLock<HashMap<Uuid, Arc<RwLock<SubscriptionRuntime>>>>>`。
2. 每收到一条 entry：找到订阅 → 内层写锁 → 对四个桶做 `roll_if_needed(now)`（周期起点变了就清零换起点）→ 累加四项 → 记录该订阅 id 到 `dirty` 集合 → 若某周期 **从未达标变为达标**（`used_before < limit && used_after >= limit`）→ 记一条 `SubscriptionStateChange`-类事件（`EventKind` 新增 `QuotaReached`，`Severity::Warn`）并 `app.emit("subscription_quota_reached", {subscription_id, period})`。
3. `flush_batch` 同事务里对 `dirty` 里的订阅**按快照写**（不是写增量）：`INSERT ... ON CONFLICT(subscription_id, period) DO UPDATE SET period_start_ms=excluded..., 四项=excluded..., updated_at_ms=excluded...`。快照写法天然处理周期滚动，不需要 delta 与 DB 行做 period 比对。
4. 启动：`store::load_quota_usage(pool)` 在 `load_runtime` 之后装填 `quota_usage`；装填时同样 `roll_if_needed(now)`（app 停机跨过周期边界的情况）。

在线判定实时（ms 级）、落库批量（沿用 50 条 / 5s），app 崩溃最多丢一批的计数（可接受，与请求日志同一保证）。

### A.4 可调度性判定

`SubscriptionRuntime::is_dispatchable(now)` 末尾追加：

```rust
if self.row.token_quotas.any_exceeded(&self.quota_usage, now) { return false; }
```

`any_exceeded` 内部对每个设了 limit 的周期：`bucket.effective_total(now)`（如果 `period_start` 已过期则视为 0，**只读不改**，避免在读锁下修改）`>= limit`。

**不引入新的 `SubscriptionState`、不走冷却定时器、不动 `state_machine`**：限额是纯派生谓词。`recheck_worker` 只扫异常状态的订阅，限额中的订阅仍是 `Healthy`，不会被 ping（ping 也不会「复活」它）。scheduler 的 `build_candidate_order` 因走 `is_dispatchable` 自动生效；`overloaded::response_with_summary` 的 503 摘要里对超限订阅显示「已达限额(每周)」而不是状态名，便于用户区分。

### A.5 命令与 DTO

- `SubscriptionDto` 加：`token_quotas: TokenQuotas`；`quota_usage: Vec<QuotaUsageDto>`（每周期一项：`period, limit: Option<u64>, input, output, cache_creation, cache_read, period_start_ms, period_end_ms: Option<i64>`（total 为 None）, `exceeded: bool`）。四个周期**始终返回**；前端只对设了 limit 的周期画进度条。
- 新 command `update_token_quotas(id, quotas: TokenQuotas)`：校验每个值 `>0`（0 或负值拒绝，清空用 None）；写列 + 更新 runtime.row。
- 新 command `reset_total_quota_usage(id)`：把 `Total` 桶清零、`period_start` = now，立即落库（不等 flush）。
- `src/types.ts` 手工同步 `TokenQuotas` / `QuotaUsageDto` / `SubscriptionDto` 字段。

### A.6 前端

- **订阅编辑页**（`SubscriptionEdit.tsx`）新增「用量限额」卡片：四行（每日 / 每周 / 每月 / 累计总量），每行一个数字输入 + 单位提示，空 = 不限；输入接受 `5M` / `100M` / `2.5B` / `500k` 快捷写法（纯前端解析成整数再提交，显示时反向格式化）。保存走 `update_token_quotas`。
- **订阅详情/卡片**新增 `SubscriptionQuotaCard`（样式与 `SubscriptionBalanceCard` 同体系）：每条已设限额一根**四段堆叠进度条**（input / output / cache_creation / cache_read 各一色，总长 = limit，四段和 = used），旁标「已用 / 限额 · 于 <period_end> 恢复」；≥80% 进度条描边变警示色，≥100% 标「已达限额」；`total` 那行多一个「重置计数」按钮（二次确认）。
- **订阅列表**：超限订阅显示「已达限额」中性徽标（不是红色错误态）；toast 监听 `subscription_quota_reached`。
- i18n **先只动 `zh.json`**，en/ja 待文案确认后同步（项目惯例：先中文，用户审过文案再同步英日）。

### A.7 测试

- `quota::period_start`：daily/weekly/monthly 在固定时区下的边界（含跨月、跨年、周日→周一）；`FixedOffset(+8)` 与 UTC 各测一组证明是本地边界。
- `QuotaUsage::roll_if_needed`：同周期累加、跨周期清零、`Total` 永不滚动。
- `TokenQuotas::any_exceeded`：无限额恒 false；仅一项设限；`used == limit` 视为超；`period_start` 过期视为 0。
- `SubscriptionRuntime::is_dispatchable`：超限 → false，其余不变（回归现有 3 条件）。
- `run_consumer` 集成：投 3 条 entry → 内存计数即时可见；flush 后表里是快照；跨阈值只发一次事件；`Total` reset 命令清零并落库。
- `store::load_quota_usage`：重启装填 + 停机期间跨周期自动清零。
- 前端：`tsc --noEmit`；快捷写法解析器单测（若项目已有 vitest；没有则不加测试框架，靠类型）。

---

## B. 会话亲和调度（`sticky`）

### B.1 依据（2026-08-15 调研结论）

- prompt cache 在 Anthropic / OpenAI / DeepSeek 都**绑定账号**（= 一条订阅），跨订阅必 miss；Anthropic 默认 5 min（命中续期，可选 1h），OpenAI 5–10 min 空闲最长 1h（gpt-5.6+ 30 min），DeepSeek 数小时到数天。
- OpenAI 官方内部就是按 `prompt_cache_key` 把请求路由到同一机器；Codex CLI 以 thread id 作 `prompt_cache_key`。
- LiteLLM 的内容哈希方案对 Claude Code 失效（每轮 breakpoint 后移 → 哈希变 → 漂移，issue #19755），TTL 5 min 过短（#28427）。claude-relay-service 采用会话键（`metadata.user_id` 内 session_id）+ 1h TTL。
- Claude Code ≥ 2.1.86 每个请求带 `X-Claude-Code-Session-Id` 头；`metadata.user_id` 有两种格式（旧 `user_<64hex>_account_<uuid>_session_<uuid>`，2.1.78+ 为 JSON 串 `{"device_id","account_uuid","session_id"}`）。
- Codex CLI 发 `session_id` 头与 `prompt_cache_key`（body）。
- Anthropic：并发请求要等第一个响应开始后缓存才可用 → CC 并发子代理钉同一订阅可共享缓存。

### B.2 模式定义

`RoutingMode` 加 `Sticky`（serde `snake_case` = `"sticky"`；`virtual_model/store.rs` 的 `load`/`save_mode` 字符串映射各加一行；DB `mode` 列是 TEXT，**无需 migration**）。

行为：
- 提取会话键（B.3）。有键且亲和表里有**可调度**的钉住订阅 → 该订阅排第一，其余候选按轮询序（从 `last_used_index+1` 起）跟在后面作 retry 兜底；本次**不前进** `last_used_index`。
- 无键 / 未钉 / 钉住的不可调度 → 按轮询规则选一家并前进 `last_used_index`（均衡的是**会话数**不是请求数）。
- pipeline 每次把请求交给某个候选（首选或 retry 切下家）时，**立即**把 `(vm, key) → 该订阅` 写入亲和表（含 retry 改钉；不弹回）。全部失败时保留最后一次钉住值，无害。
- 钉住的订阅超限额 / 冷却 / 禁用 → `is_dispatchable=false` → 自动改钉，无需特殊处理。

### B.3 会话键提取（新纯函数模块 `proxy/session_key.rs`）

按优先级，命中即返回：
1. 请求头 `x-claude-code-session-id`
2. `body.metadata.user_id`（**整串作为不透明键**，不解析格式）
3. `/v1/responses` 入站：`body.prompt_cache_key` → 请求头 `session_id`（`handler::responses` 在翻译成 Anthropic body 前提取，随 `ClientContext` 传给 dispatch）
4. `messages` 里**第一条 `role=user` 消息**的文本拼接（string 或 text 块）→ SHA-256 前 32 hex（**不用** system：CC 的 system 首块在所有会话完全相同，会把全部会话钉到一家）
5. 都没有 → `None`，本请求不钉。

键统一加前缀标记来源便于日志（`hdr:` / `meta:` / `pck:` / `sid:` / `msg:`），最长截断 256 字节。

### B.4 亲和表（新模块 `virtual_model/affinity.rs`）

```rust
pub struct AffinityTable { map: HashMap<(VirtualModelName, String), Pin>, }
struct Pin { sub_id: Uuid, last_seen: Instant }
const IDLE_TTL: Duration = 1h;   // 与 Anthropic/OpenAI 缓存上限对齐; 每次命中刷新
const MAX_ENTRIES: usize = 10_000;  // 超出按 last_seen 最旧淘汰
pub fn get(&mut self, vm, key, now) -> Option<Uuid>   // 过期视为不存在并顺手删除
pub fn pin(&mut self, vm, key, sub_id, now)
pub fn sweep(&mut self, now)   // 每次 pin 时若距上次 sweep > 5 min 就跑一遍
```

放在 `AppState.session_affinity: Arc<Mutex<AffinityTable>>`（`std::sync::Mutex`，临界区极短）。**内存不持久化**（与 `last_used_index` 同一取舍；重启后各会话冷一次）。订阅被删除时不主动清表，`get` 命中后 pipeline 发现 sub 不在 map 里就当未钉处理。

### B.5 调度器改动

`build_candidate_order(vm, all_subs, now, pinned: Option<Uuid>)`：

- `Sequential` / `RoundRobin`：忽略 `pinned`，行为不变。
- `Sticky`：若 `pinned` 在 `vm.subscription_ids` 中且 `is_dispatchable` → `candidate_ids = [pinned] + 轮询序其余可调度`，`chosen_index = None`（不前进）；否则退化为 `RoundRobin` 分支（`chosen_index = Some(..)`）。

pipeline：`dispatch` 开头提取会话键 → `affinity.get` → 传入 scheduler → 循环里每次选定候选就 `affinity.pin`。fallback 虚拟模型同样适用（键空间按 vm 隔离）。

### B.6 前端

- 虚拟模型页模式下拉加「会话亲和」，hint 文案：「同一会话固定同一订阅，跨会话轮询；保住 prompt cache、并发子代理共享缓存，失败时自动切换并改钉」。
- `types.ts` 的 `RoutingMode` union 加 `"sticky"`；`lib/virtualModels.ts` 的模式列表加项。
- i18n 先 zh。

### B.7 测试

- `session_key`：五级优先级各一条 + 都缺失 → None；截断；system 相同、首条 user 不同 → 键不同。
- `AffinityTable`：get/pin/刷新 last_seen；空闲 1h 过期；MAX_ENTRIES 淘汰最旧；sweep。
- `build_candidate_order(Sticky)`：钉住可用 → 排首且 `chosen_index=None`；钉住不可用 → 走轮询且前进；`pinned` 不属于该 vm → 走轮询；两种老模式回归不变。
- pipeline 集成（`tests/integrations_claude_code.rs` 风格 mock 上游）：同 session 两次请求命中同一订阅；首选 5xx 后切下家且改钉；不同 session 轮询分配；`last_used_index` 只在新会话前进。

---

## 交汇点与顺序

两项唯一交汇是 `is_dispatchable`：限额让钉住订阅不可调度时，sticky 自然改钉。建议实施顺序 **A（限额）→ B（sticky）**：A 改动面在订阅侧、B 在调度侧，B 的集成测试可以顺带覆盖「钉住订阅超限自动改钉」。

## 文档同步

- CLAUDE.md：三层抽象里 `RoutingMode` 加 `sticky` 说明；隐藏约束加「`subscription_quota_usage` 按本地时区、快照写、内存为准」；开头「唯一客户端是 CC」一句改为「主客户端是 CC，另有 Codex CLI 经 `/v1/responses` 入站」（本轮顺手修正）。
- README×3 的「调度模式」FAQ 加会话亲和一条（zh 先行）。
