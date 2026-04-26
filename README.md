<p align="center">
  <img src="assets/icon.png" alt="cc-router logo" width="160" height="160" />
</p>

<h1 align="center">cc-router</h1>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg?style=flat-square" alt="License: MIT"></a>
  <img src="https://img.shields.io/badge/Tauri-2-FFC131?style=flat-square&logo=tauri&logoColor=white" alt="Tauri 2">
  <img src="https://img.shields.io/badge/Rust-1.77+-DEA584?style=flat-square&logo=rust&logoColor=white" alt="Rust 1.77+">
  <img src="https://img.shields.io/badge/React-19-61DAFB?style=flat-square&logo=react&logoColor=white" alt="React 19">
  <img src="https://img.shields.io/badge/TypeScript-5-3178C6?style=flat-square&logo=typescript&logoColor=white" alt="TypeScript 5">
  <img src="https://img.shields.io/badge/Tailwind-4-06B6D4?style=flat-square&logo=tailwindcss&logoColor=white" alt="Tailwind CSS">
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey?style=flat-square" alt="Platform">
</p>

<p align="center">
  <strong>中文</strong> · <a href="README.en.md">English</a>
</p>

订阅买多了 Claude Code 却只能用一家？cc-router 把 DeepSeek、Qwen、Kimi、MiMo、MiniMax、GLM、Claude 的 Token Plan、Coding Plan、API 额度合并成一个虚拟 Plan，任意搭配 opus / sonnet / haiku 三槽位，按顺序或轮询调度，限流、失败自动切换——把每一份额度榨到最后一个 token。

<p align="center">
  <img src="assets/screenshot-models.png" alt="cc-router 虚拟模型配置页截图" width="900" />
  <br />
  <img src="assets/screenshot-logs.png" alt="cc-router 请求日志页截图" width="900" />
</p>

## 支持的编程套餐和API

| id | 名称 | Token Plan | API | 兼容性 |
|---|---|---|---|---|
| `anthropic` | Anthropic 官方 API（仅按量付费，不含 Max Plan） | ❌ | ✅ | verified |
| `zhipu` | 智谱 GLM | ✅ | ✅ | verified |
| `deepseek` | DeepSeek | ❌ | ✅ | verified |
| `moonshot` | Moonshot Kimi | ❌ | ✅ | verified |
| `minimax` | MiniMax（3 个 endpoint） | ✅ | ✅ | partial |
| `xiaomi` | 小米 MiMo（按量付费 + 3 集群订阅） | ✅ | ✅ | untested |
| `alibaba` | 阿里云百炼（Token Plan 团队版 + 按量付费 2 地域 + 停售的 Coding Plan） | ✅ | ✅ | verified |
| `volcengine` | 火山方舟（Coding Plan 订阅 + 按量付费） | ✅ | ✅ | untested |
| `openrouter` | OpenRouter 聚合平台（500+ 模型路由） | ❌ | ✅ | untested |
| `tencent` | 腾讯云大模型（Token Plan 订阅 + TokenHub 按量付费境内/境外） | ✅ | ✅ | untested |
| `aiberm` | Aiberm（按量付费 API，模型按 token group 动态返回） | ❌ | ✅ | untested |
| `whatai` | 神马中转 API（按量付费，OpenAI/Anthropic 双协议中转，仅用 Anthropic 路径） | ❌ | ✅ | untested |
| `ollama` | Ollama 本地推理（仅 localhost:11434，含云端模型 tag 如 `glm-4.7:cloud`） | ❌ | ✅| partial |
| `fireworks` | Fireworks AI（按量付费，覆盖 DeepSeek / Qwen / Llama / Kimi 等开源模型），支持Fire Pass订阅 | ✅ | ✅ | verified |
| `stepfun` | 阶跃星辰（Step Plan 订阅 + 按量付费 API） | ✅ | ✅ | untested |
| `baidu` | 百度千帆（Coding Plan 订阅，模型手动填写） | ✅ | ❌ | untested |
| `modelscope` | 魔搭 ModelScope（按量付费，OpenAI/Anthropic 双协议，仅用 Anthropic 路径，覆盖 Qwen / DeepSeek / Kimi / MiniMax 等开源模型） | ❌ | ✅ | partial |

> Token Plan 列包含各厂商的套餐订阅形态（Token Plan / Coding Plan 等）；API 列指按量付费的 Anthropic Messages 兼容端点。

社区可 PR 补充。

## 技术栈

- Tauri 2
- Tailwind 4
- React 19

## 安装使用

1. 在release里下载客户端并安装。
2. 配置多厂商的大模型，配置虚拟模型对应的真实模型和调度模式。
3. 配置到Claude Code中使用。

## 在 Claude Code 中使用

`设置` 页会动态显示完整的 env snippet；默认端口被占用时自动 +1 递增。

```json
{
  "env": {
    "ANTHROPIC_BASE_URL": "http://127.0.0.1:23456",
    "ANTHROPIC_AUTH_TOKEN": "do-not-need",
    "API_TIMEOUT_MS": "3000000",
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
    "ANTHROPIC_MODEL": "model-opus",
    "ANTHROPIC_DEFAULT_SONNET_MODEL": "model-sonnet",
    "ANTHROPIC_DEFAULT_OPUS_MODEL": "model-opus",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL": "model-haiku"
  }
}
```

## 开发

依赖：Node.js ≥ 20（推荐 pnpm），Rust ≥ 1.77，Xcode CLT（macOS）。

```bash
pnpm install
pnpm tauri dev      # 启动开发模式（同时运行前端 + Rust 后端 + 代理）
```

首次启动 app 会进入 onboarding：

1. 添加一个订阅（选厂商 → 选接入点 → 填 API Key → 自动抓取模型列表）
2. 一键把订阅绑定到三个虚拟模型
3. 复制 Claude Code 环境变量配置，粘到你的 `~/.claude/settings.json`

## 添加新provider

如果你使用`Claude Code`，我提供了一个`SKILL`，可以执行`new-provider`并附加provider的官方文档或接口地址等信息，能够自动创建provider的配置。

## 打包

```bash
pnpm tauri build
```

产出：`src-tauri/target/release/bundle/` 下对应平台的安装包。

## 图标

Provider 品牌 logo 来自 [@lobehub/icons](https://github.com/lobehub/lobe-icons)（MIT）。各品牌商标归原所有者所有。

## 证书

MIT
