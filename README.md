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
  <img src="https://img.shields.io/badge/Tailwind-3-06B6D4?style=flat-square&logo=tailwindcss&logoColor=white" alt="Tailwind CSS">
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey?style=flat-square" alt="Platform">
  <img src="https://img.shields.io/badge/providers-7-D97757?style=flat-square" alt="7 providers">
</p>

> 本地桌面 app，将多个大模型订阅的 API Key 聚合成一个 Anthropic 兼容端点，供 Claude Code 调用。核心能力：**虚拟模型映射 + 订阅自动切换**。

三个固定虚拟模型（`model-opus` / `model-sonnet` / `model-haiku`）各自绑定一组订阅，代理会按调度模式（顺序 / 轮询）挑选可用订阅转发，并在限流 / 失败时透明切换。

## 技术栈

| 层 | 选择 |
|---|---|
| 桌面外壳 | Tauri 2 (Rust) |
| 代理服务 | Axum + Tokio + Reqwest（同进程启动） |
| 数据存储 | SQLite (sqlx) + Keychain (API Key) |
| 前端 | React 19 + Vite + Tailwind + shadcn/ui |
| 拖拽 | @dnd-kit |
| 状态管理 | @tanstack/react-query |

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

## 在 Claude Code 中使用

`设置` 页会动态显示完整的 env snippet；默认端口被占用时自动 +1 递增。

```bash
export ANTHROPIC_BASE_URL=http://127.0.0.1:23456
export ANTHROPIC_DEFAULT_OPUS_MODEL=model-opus
export ANTHROPIC_DEFAULT_SONNET_MODEL=model-sonnet
export ANTHROPIC_DEFAULT_HAIKU_MODEL=model-haiku
export CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1
export ANTHROPIC_AUTH_TOKEN=dummy
```

## 测试

```bash
cd src-tauri
cargo test              # 单元测试 + proxy_e2e 集成测试
pnpm tsc --noEmit       # 前端类型检查（在项目根）
```

## 打包

```bash
# Tauri build 需要完整尺寸的 icon 集；先由一张源 PNG 生成
pnpm tauri icon src-tauri/icons/icon.png
pnpm tauri build
```

产出：`src-tauri/target/release/bundle/` 下对应平台的安装包。

## 内置 Provider

`src-tauri/providers/*.yaml` 管理，启动时按 JSON Schema 校验。当前包含：

| id | 名称 | 兼容性 |
|---|---|---|
| `anthropic` | Anthropic 官方 API（仅按量付费，不含 Max Plan） | verified |
| `zhipu` | 智谱 GLM | verified |
| `deepseek` | DeepSeek | verified |
| `moonshot` | Moonshot Kimi | verified |
| `minimax` | MiniMax（3 个 endpoint） | partial |
| `xiaomi` | 小米 MiMo（按量付费 + 3 集群订阅） | untested |
| `alibaba` | 阿里云百炼（Token Plan 团队版 + 按量付费 2 地域 + 停售的 Coding Plan） | verified |

社区可 PR 补充。

## 图标

Provider 品牌 logo 来自 [@lobehub/icons](https://github.com/lobehub/lobe-icons)（MIT）。各品牌商标归原所有者所有。

## 目录结构

```
cc-router/
├── src/                  # React 前端
├── src-tauri/
│   ├── src/              # Rust 后端（代理 + Tauri commands）
│   ├── migrations/       # SQLite 初始化 DDL
│   └── providers/        # 内置 provider YAML
└── mydata/设计稿.md       # 完整设计方案
```
