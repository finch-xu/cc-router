---
name: new-provider
description: 用于在 cc-router 仓库新增一个 LLM provider（即在 src-tauri/providers/ 下添加 YAML 描述符并完成 5 处同步改动）。当用户说「加 provider」「接入 XX 厂商」「新增订阅源」「provider YAML」「让 cc-router 支持 OpenRouter/Together/Groq/Ollama 之类」时必须触发本 skill；即便用户只甩了一个厂商名或一个文档 URL，只要看起来在 cc-router 仓库内做新增 provider，就走本 skill 的工作流，不要绕过。本 skill 只覆盖「描述符层」扩展（YAML + 配置 + 测试 + 文档），不涉及调度/状态机的 Rust 改动。
---

# 新增 Provider 工作流

## 这个 skill 在做什么

cc-router 的 Provider 抽象 = 「YAML 描述符」。把一个新厂商接入路由层不需要写 Rust——只需要一份遵循 `providers/_schema.json` 的 YAML，加上 4 处机械同步改动（bundle resources / 测试白名单 / README / 可选图标）。

这份 skill 的价值在于：

1. **决策清单**：哪些字段是「研究上游文档才能填对」的关键字段（auth、base_url、/models 端点）
2. **同步检查清单**：5 处改动一处不漏（漏一处会导致 release 包加载失败 / 测试断言失败 / 文档失同步）
3. **常见陷阱**：哪些上游 API 设计会让默认假设崩塌（无 /models、key 被忽略、messages 与 /models 不同域）

## 触发条件

走本 skill 当且仅当用户在 cc-router 仓库内做「新增 provider」类工作。如果只是改既有 YAML 字段（如调 endpoint 顺序、改 description）则不必走完整流程，直接编辑即可。

## 五步工作流（顺序执行）

### Step 1：研究上游文档，决定 YAML 字段

**先查清楚 6 件事**（用 WebFetch 或问用户）：

| 字段 | 关键问题 |
|---|---|
| `endpoints[].base_url` + `messages_path` | Anthropic 兼容端点完整 URL？是否多区域/多 endpoint？ |
| `auth.header_format` | `x-api-key` raw（仅 Anthropic 系）还是 `Authorization: Bearer`？ |
| `auth.header_name` | 多数家是 `Authorization`，少数是 `x-api-key`/自定义 |
| `required_headers` | 是否要 `anthropic-version`？是否要其他厂商专属 header？ |
| `model_discovery` | 是否有 Anthropic 风格 `/v1/models` 端点？路径？是否与 messages **同域**？需要独立 URL 时用 `model_discovery.url` 字段（完整 URL 覆盖，不走 base_url 拼接） |
| 是否需 API Key | 极少数厂商（如 Ollama 本地）不校验 key——仍要保留字段，文档里说明 |

**判断 `compatibility` 字段**：
- `verified`：自己跑通过实际请求 + SSE 流式
- `partial`：有限制（如无 /models、流式有兼容 quirks）
- `untested`：仅按文档接入未实测

### Step 2：写 YAML 文件

**位置**：`src-tauri/providers/<id>.yaml`

**id 命名**：小写英文/数字/下划线（schema 强制 `^[a-z0-9_]+$`）。优先用厂商英文短名（`anthropic`、`deepseek`、`zhipu`），不要带版本号或地域后缀。

**模板骨架**：

```yaml
id: <provider_id>
display_name: "<厂商展示名>"
icon: ""  # 没有 lucide brand icon 时留空走 Bot 兜底; 有则填 BRAND_MAP key
description: "<一句话描述>"
homepage: "<主页 URL>"
docs_url: "<API 文档 URL>"
api_key_url: "<控制台密钥页面 URL>"

compatibility: untested  # 或 partial/verified
compatibility_notes: |
  <需要用户知道的限制：流式 quirks、模型列表问题、特殊计费等>

endpoints:
  - id: <endpoint_id>
    label: "<UI 显示的人话名称, 含「订阅/按量付费/国内版/国际版」等区分>"
    description: "<细节说明>"
    base_url: "<https://...>"
    messages_path: "/v1/messages"
    region: <china|global|local>
    billing: <subscription|pay_as_you_go|free>

default_endpoint: <endpoint_id>  # 必须是上面 endpoints[].id 之一

auth:
  type: api_key
  header_name: "Authorization"      # 或 "x-api-key"
  header_format: bearer             # 或 raw

required_headers:
  anthropic-version: "2023-06-01"   # 大部分厂商都接受这个 header

forward_headers: []

model_discovery:
  enabled: true                     # 无 /models 接口则填 false
  path: "/v1/models"                # 或 url: "https://..." 完整覆盖
  cache_ttl_hours: 24
  example_models:                   # enabled: false 时作为 UI 输入提示
    - "<示例模型 ID>"
```

**关键决策点（写之前对照参考表）**：

```
auth.header_format 选哪个？
├─ x-api-key raw → 仅 anthropic / ollama 这种「Anthropic 同款」
└─ Authorization bearer → 其余几乎所有第三方

model_discovery.enabled?
├─ true（path 同 base_url 域）→ alibaba / anthropic
├─ true（url 完整覆盖, 跨域）→ deepseek / zhipu / moonshot / xiaomi
└─ false（无端点, 手动输入）→ minimax / ollama

endpoints 数量？
├─ 1 个 → 只有单一访问入口（anthropic / ollama）
├─ 2-4 个 → 区分订阅 vs 按量、国内 vs 国际、不同区域集群
```

**已有 provider 是最好的参考**：写之前先 `Read` 一个最相似的现有 YAML（按 auth + model_discovery 组合匹配），照葫芦画瓢比从模板硬写更可靠。

### Step 3：注册到 bundle.resources（不能漏）

**位置**：`src-tauri/tauri.conf.json::bundle.resources`

```json
"resources": [
  "providers/_schema.json",
  ...
  "providers/<existing>.yaml",
  "providers/<new_id>.yaml",   ← 新增此行
  "migrations/001_init.sql",
  "../LICENSE"
]
```

**为什么必须**：Tauri release 打包只把显式列出的文件打进 bundle。漏一行 → dev 模式没事（read from working dir），release 模式 `resource_dir().join("providers")` 扫不到 → fatal。这是 cc-router 最容易踩的坑之一。

### Step 4：更新测试白名单 + 总数 assert

**位置**：`src-tauri/tests/proxy_e2e.rs::provider_loader_loads_builtin_providers`

```rust
for expected in [..., "<new_id>"] {  // 加到数组末尾
    assert!(ids.contains_key(expected), "missing provider: {expected}");
}
assert_eq!(providers.len(), N+1);    // 数字递增 1
```

**为什么是双更新**：白名单查存在，`providers.len()` 锁总数。后者能在 YAML 文件被加进目录但漏写 `bundle.resources` 时炸出错——这是它存在的意义。两者必须同步改。

### Step 5：README 表格 + 可选图标

**README**（位置：`README.md` 「内置 Provider」表）：

```markdown
| `<new_id>` | <一句话描述厂商特征> | <verified/partial/untested> |
```

**ProviderIcon BRAND_MAP**（仅当 `@lobehub/icons` 有该品牌图标时）：

位置：`src/components/ProviderIcon.tsx`

```tsx
import NewBrand from "@lobehub/icons/es/NewBrand";
const BRAND_MAP: Record<string, BrandIcon> = {
  ...
  <new_id>: NewBrand as unknown as BrandIcon,
};
```

并把 YAML 的 `icon: ""` 改成 `icon: <new_id>`（必须和 BRAND_MAP key 一致）。

`@lobehub/icons` 没有的品牌（如 Ollama / 小厂中转）保持 `icon: ""`，UI 自动用 `Bot` lucide 图标兜底——不要为了好看强行映射到不相关的图标。

## 验证

执行最小验证集：

```bash
cd src-tauri && cargo test --test proxy_e2e provider_loader
```

通过 = 5 步同步无错。失败 99% 是漏改 Step 3 的 bundle.resources 或 Step 4 的总数 assert。

可选：`pnpm tsc --noEmit` 确认 BRAND_MAP 导入没拼错（Step 5 改动时）。

## 不做什么

下面这些**都不需要为新 provider 做改动**——cc-router 的 Provider 抽象就是为了避免这些工作而存在的：

- 改调度器（`virtual_model/scheduler.rs`）
- 改状态机（`virtual_model/state_machine.rs`）
- 改 SSE 流式处理（`proxy/sse.rs`）
- 改 reqwest 上游调用（`proxy/upstream.rs`）
- 加 migration（`db/migrations/`）

如果你发现确实需要改这些地方，那说明这个 provider 不是简单的 Anthropic 兼容端点——先停下来跟用户对齐，可能是 schema 设计有缺口（例如某厂商需要特殊请求体改写、或非标准认证流程），需要扩展 `_schema.json` 而非绕过。

## 决策提示词

写完 YAML 草稿、执行 Step 3-5 之前，主动向用户确认这 3 件事——它们没有客观正确答案：

1. **endpoints 数量**：单端点够还是要列国内/国际/订阅/按量多组？
2. **API Key 字段**：厂商是否真的需要 key？某些（如 Ollama）不校验，要在 `compatibility_notes` 写清楚
3. **example_models**：当 `model_discovery.enabled: false` 时这是 UI 唯一提示，常用模型放前面

不要替用户拍板这些决策——它们关系到用户的实际使用偏好。

## 流程结束

5 步走完 + `cargo test` 通过 = 工作完成。**不要**主动提议提交 commit / 发 PR——cc-router 维护者偏好确认改动后自己提交。如果用户明确要求 commit，再走 commit 流程。
