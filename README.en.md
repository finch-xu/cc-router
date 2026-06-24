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
  <a href="README.md">中文</a> · <strong>English</strong> · <a href="README.ja.md">日本語</a>
</p>

<p align="center">
  <a href="https://ccrouter.app/docs/" target="_blank" rel="noopener">📖 中文文档</a> ·
  <a href="https://ccrouter.app/en/docs/" target="_blank" rel="noopener">📖 English Docs</a> ·
  <a href="https://ccrouter.app/ja/docs/" target="_blank" rel="noopener">📖 日本語ドキュメント</a> ·
  <a href="https://deepwiki.com/finch-xu/cc-router" target="_blank" rel="noopener">🤖 DeepWiki</a> ·
  <a href="https://ccrouter.app" target="_blank" rel="noopener">🌐 Official Site ccrouter.app</a>
</p>

Bundle your scattered `Token Plan`, `Coding Plan`, and LLM API quotas into a single virtual Plan, and plug it into Claude Code, Claude Desktop App, OpenClaw, OpenCode, Codex and more —— save money! save tokens! 100% local!

> ⚠️ Notice: this tool only switches between subscription plans you already own. Request bodies are passed through almost verbatim — no reverse engineering, no jailbreak, no circumvention. You are responsible for complying with each plan's terms of service. cc-router is intended for use with coding tools like Claude Code only; do not use it for anything else.
>
> Provider terms of service do not necessarily allow "routing a subscription key through a third-party proxy with multi-virtual-model dispatch" — especially for per-seat subscriptions like Coding Plans / Token Plans, where this pattern may trip risk controls. The author assumes no liability for any account being throttled, banned, or having its subscription cancelled as a result of using this tool.
>
> This software is provided As-Is, without warranty of any kind. The author is not liable for any direct or indirect damages arising from its use, including but not limited to abnormal quota consumption, data loss, or business interruption.

Highlights:

- **19 providers, one router** — built-in routing for DeepSeek, Qwen, Kimi, MiMo, MiniMax, GLM, Claude, Gemini, etc., across Token Plans / Coding Plans / pay-as-you-go APIs; mix and match opus / sonnet / haiku slots with sequential or round-robin dispatch
- **Bring your own endpoint** — when the built-in providers don't cover what you need, plug any Anthropic Messages-compatible, Gemini generateContent / Gemini Interactions-compatible, or OpenAI Responses / Chat Completions-compatible API in directly, dispatched alongside the built-in subscriptions
- **Usage receipts** — export your token-spend snapshot as PNG / PDF / HTML in one click; mono / color modes, no pricing shown by default (usage only), QR code at the bottom links back to the repo
- **Fully translated UI** — 简体中文 / English / 日本語, follows your system locale or pick manually in Settings
- **Virtual model aliases** — each of fable / opus / sonnet / haiku accepts multiple names; for opus that's `model-opus` / `claude-opus-4-7` / `anthropic/model-opus` / `anthropic/claude-opus-4-7`, all routed to the same virtual model — pick whatever naming your tool prefers
- **Local HTTPS** — generate a self-signed CA and server cert in one click so HTTPS-only clients can talk to cc-router too; see the [setup guide](https://ccrouter.app/docs/claude-desktop-integration/)
- **Claude Desktop App support** — combine local HTTPS with the virtual-model aliases above and Anthropic's official desktop app can route through cc-router's aggregated subscriptions; see the [setup guide](https://ccrouter.app/docs/claude-desktop-integration/)
- **Dual-protocol ingress** — `Anthropic /v1/messages` and `OpenAI /v1/responses` are exposed side by side, so clients across both ecosystems — Claude Code, Codex and the like — plug into the same router with a single config

<table align="center">
  <tr>
    <td width="60%"><img src="assets/screenshot-models.png" alt="cc-router virtual model configuration page" /></td>
    <td width="40%" rowspan="2"><img src="assets/screenshot-receipts.png" alt="cc-router usage receipts long screenshot" /></td>
  </tr>
  <tr>
    <td width="60%"><img src="assets/screenshot-logs.png" alt="cc-router request logs page" /></td>
  </tr>
</table>

## Integration Guide

The AI Agent / Coding Agent tools listed below can all connect to cc-router and use every LLM plan you have.

<p>
<a href="https://ccrouter.app/docs/getting-started/" target="_blank" rel="noopener">Claude Code cli</a> · 
<a href="https://ccrouter.app/docs/claude-desktop-integration/" target="_blank" rel="noopener">Claude Desktop App</a> · 
<a href="https://ccrouter.app/docs/codex-integration/" target="_blank" rel="noopener">OpenAI Codex cli</a> · 
<a href="https://ccrouter.app/docs/codex-integration/" target="_blank" rel="noopener">OpenAI Codex Desktop App</a> · OpenCode · OpenClaw · Kimi code cli · pi coding agent and many more.
</p>

## Supported Coding Plans and APIs

| id | Name | Token Plan | API | Status |
|---|---|---|---|---|
| `anthropic` | Anthropic official API (pay-as-you-go only, no subscription plan) | ❌ | ✅ | verified |
| `openai_codex` | **OpenAI Codex (ChatGPT Plus/Pro subscription)** — account-suspension risk; not recommended | ✅ | ❌ | tested |
| `kiro` | **Kiro IDE (AWS)** — free Claude subscription quota; account-suspension risk; not recommended | ✅ | ❌ | tested |
| `google_ai_studio` | **Google AI Studio (Gemini)** pay-as-you-go + free quota | ❌ | ✅ | verified |
| `google_gemini_interactions` | **Google Gemini (Interactions API)** new unified `/v1beta/interactions` endpoint (protocol translation) | ❌ | ✅ | partial |
| `zhipu` | Zhipu GLM (pay-as-you-go / China subscription) | ✅ | ✅ | verified |
| `deepseek` | DeepSeek (pay-as-you-go) | ❌ | ✅ | verified |
| `moonshot` | Moonshot Kimi (pay-as-you-go / China subscription / global subscription) | ✅ | ✅ | untested |
| `minimax` | MiniMax (pay-as-you-go / China subscription / global subscription) | ✅ | ✅ | verified |
| `xiaomi` | Xiaomi MiMo (pay-as-you-go / China subscription / global subscription) | ✅ | ✅ | verified |
| `alibaba` | Alibaba Cloud Bailian (team Token Plan + 2-region pay-as-you-go + discontinued Coding Plan) | ✅ | ✅ | verified |
| `volcengine` | ByteDance Volcengine Ark (Coding Plan subscription + Agent Plan subscription + pay-as-you-go) | ✅ | ✅ | untested |
| `openrouter` | OpenRouter aggregator (500+ models routed) | ❌ | ✅ | untested |
| `tencent` | Tencent Cloud LLM (Token Plan subscription + TokenHub pay-as-you-go, mainland / overseas) | ✅ | ✅ | untested |
| `aiberm` | Aiberm (pay-as-you-go API, models returned dynamically by token group) | ❌ | ✅ | untested |
| `whatai` | Shenma relay API (pay-as-you-go, OpenAI/Anthropic dual-protocol relay, Anthropic path only) | ❌ | ✅ | untested |
| `ollama` | Ollama local inference (localhost:11434 only, includes cloud tags like `glm-4.7:cloud`) | ❌ | ✅ | partial |
| `fireworks` | Fireworks AI (pay-as-you-go / Fire Pass global subscription) | ✅ | ✅ | verified |
| `stepfun` | Stepfun (pay-as-you-go / China subscription / global subscription) | ✅ | ✅ | untested |
| `baidu` | Baidu Qianfan (pay-as-you-go / China subscription) | ✅ | ✅ | untested |
| `modelscope` | ModelScope (pay-as-you-go) | ❌ | ✅ | partial |
| `ucloud` | UCloud Modelverse (Coding Plan subscription + pay-as-you-go API in CN/global) | ✅ | ✅ | untested |
| `openai` | **OpenAI official API** (pay-as-you-go; includes GPT-5 / o3 / 4.1 reasoning models; auto-translates Anthropic thinking ↔ OpenAI reasoning) | ❌ | ✅ | untested |
| `Custom` | Bring your own Anthropic-compatible endpoint | ✅ | ✅ | verified |
| `Custom (Gemini compatible)` | Bring your own Gemini generateContent-compatible endpoint (relay, etc.); `messages_path` must contain the `{model}` placeholder | ❌ | ✅ | tested |
| `Custom (Gemini Interactions compatible)` | Bring your own Gemini Interactions API `/v1beta/interactions`-compatible endpoint (Google's new unified API / compatible relay); auto protocol translation; unlike the legacy generateContent, the model goes in the request body, so no `{model}` placeholder is needed | ❌ | ✅ | partial |
| `Custom (OpenAI Responses compatible)` | Bring your own OpenAI `/v1/responses`-compatible endpoint (one-api / new-api relays, etc.); auto protocol translation | ❌ | ✅ | tested |
| `Custom (OpenAI Chat Completions compatible)` | Bring your own OpenAI `/v1/chat/completions`-compatible endpoint (DeepSeek, Together, Groq, Ollama, one-api / new-api relays, etc.); auto protocol translation; DeepSeek R1's `reasoning_content` is surfaced as Claude Code thinking blocks | ❌ | ✅ | tested |

> The "Token Plan" column covers any subscription-style quota (Token Plan / Coding Plan / Agent Plan, etc.); "API" denotes pay-as-you-go Anthropic Messages-compatible endpoints.

Community PRs welcome.

## Tech Stack

- Tauri 2
- Tailwind 4
- React 19

## Quick Start

1. Download the installer from Releases and run it.
2. Add your LLM subscriptions, bind them to virtual models, pick a dispatch mode.
3. Point Claude Code at cc-router via the env snippet below.

## Using with Claude Code

The **Settings** page renders the full env snippet dynamically — if the default port is taken, cc-router probes upward up to 100 times.

```json
{
  "env": {
    "ANTHROPIC_BASE_URL": "http://127.0.0.1:23456",
    "ANTHROPIC_AUTH_TOKEN": "your token, show in this app settings",
    "API_TIMEOUT_MS": "3000000",
    "ANTHROPIC_MODEL": "model-fable",
    "ANTHROPIC_DEFAULT_FABLE_MODEL": "model-fable",
    "ANTHROPIC_DEFAULT_OPUS_MODEL": "model-opus",
    "ANTHROPIC_DEFAULT_SONNET_MODEL": "model-sonnet",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL": "model-haiku",
    "CLAUDE_CODE_SUBAGENT_MODEL": "model-opus",
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
    "CLAUDE_CODE_DISABLE_NONSTREAMING_FALLBACK": "1",
    "CLAUDE_CODE_ATTRIBUTION_HEADER": "0",
    "CLAUDE_CODE_EFFORT_LEVEL": "max"
  }
}
```

When the `OPUS_MODEL` supports a `1m` context window, set it to `model-opus[1m]` to get Claude Code's full long-context support.

The LiteLLM-style `anthropic/` prefix is also accepted: `anthropic/model-opus` / `anthropic/model-sonnet` / `anthropic/model-haiku` are equivalent to the prefix-less forms, making it easy to integrate with tools that require a provider prefix to recognize the Anthropic protocol.

Virtual models and aliases:

| Virtual model | Aliases |
|---|---|
|  `model-fable` |  `anthropic/model-fable` `anthropic/claude-fable*` `claude-fable*` `gpt-5.6` `openai/gpt-5.6` |
|  `model-opus` |  `anthropic/model-opus` `anthropic/claude-opus*` `claude-opus*` `gpt-5.5` `openai/gpt-5.5` |
|  `model-sonnet` |  `anthropic/model-sonnet` `anthropic/claude-sonnet*` `claude-sonnet*` `gpt-5.4` `openai/gpt-5.4` |
|  `model-haiku` |  `anthropic/model-haiku` `anthropic/claude-haiku*` `claude-haiku*`  `gpt-5.4-mini` `openai/gpt-5.4-mini` |

> `claude-opus*` is a wildcard (prefix match): you can pass any model name that fits the pattern and it will be normalized to the `model-opus` virtual model — e.g. `claude-opus-4-8`, `claude-opus-4-7-20260101`, and `claude-opus-100` all work.

## FAQ & Use Cases

<details>
<summary>What problem does cc-router solve?</summary>

**Without cc-router**: your AI agent (Claude Code / OpenCode / etc.) can only talk to one vendor at a time. Small-quota plans run out at the worst moment, and you end up swapping config files by hand — bad experience.

**With cc-router**: agent → cc-router → vendor A + B + C, with automatic load balancing and failover. Three subscriptions behave like one.

What you get:

- **Save money** — no need for an expensive top-tier Coding Plan; two cheap small-quota plans get the job done
- **No interruptions** — rate limits / failures trigger automatic switching, transparent to the agent
- **Mix top models** — GLM-5.1, DeepSeek-V4-Pro, MiniMax-2.7, MiMo-V2.5-Pro all on the table at once, plus native APIs like Claude Opus or GPT-5.5
- **Unified usage view** — every subscription's token spend on a single screen, exportable as a receipt

</details>

<details>
<summary>What are the <code>model-opus</code> / <code>model-sonnet</code> / <code>model-haiku</code> virtual models?</summary>

Claude Code uses three model tiers by task difficulty: opus for planning, sonnet for coding, haiku for tool calls.

cc-router abstracts those tiers as the virtual slots `model-opus` / `model-sonnet` / `model-haiku`. Each slot is bound to a list of real models plus a scheduling mode:

- `model-opus` → DeepSeek-V4-Pro + GLM-5.1 (round-robin)
- `model-sonnet` → MiniMax-M2.7 + MiMo-V2.5-Pro (round-robin)
- `model-haiku` → GLM-4-Flash

When CC sends a request, cc-router routes by the mapping — no more hand-editing `~/.claude/settings.json`.

</details>

<details>
<summary>How should I combine multiple Coding Plans?</summary>

Example: subscription A = GLM-5 / MiniMax-2.7 / DeepSeek-Flash; subscription B = DeepSeek-V4-Pro / MiniMax-2.7 / GLM-5.

- **Conservative** — bind same-tier models from both sides into the matching slot for consistent behavior and good failover
- **Aggressive** — put each side's flagship model into `model-opus` on round-robin; cross-pollination often gives you `1 + 1 ≥ 2`

</details>

<details>
<summary>Scheduling mode: sequential or round-robin?</summary>

- **Sequential** — drain account A first, then switch to B. Better cache hit rate; ideal for **squeezing every token out of two small GLM Coding Plans**
- **Round-robin** — both accounts share the load. Caveat: cross-account caches are independent, so you'll burn slightly more quota in exchange for true load balancing

</details>

## Development

Prerequisites: Node.js ≥ 20 (pnpm recommended), Rust ≥ 1.77, Xcode Command Line Tools (macOS).

```bash
pnpm install
pnpm tauri dev      # runs frontend + Rust backend + proxy in one process
```

First launch opens the onboarding flow:

1. Add a subscription (pick provider → endpoint → paste API key → auto-fetch the model list).
2. Bind the subscription to all three virtual models in one click.
3. Copy the generated env snippet into your `~/.claude/settings.json`.

## Adding a new provider

If you use **Claude Code**, this repo ships a `SKILL` named `new-provider`. Run it with the official docs URL or endpoint info of the target provider, and it will scaffold the YAML and wire up the related changes for you.

## Build

```bash
pnpm tauri build
```

Artifacts land in `src-tauri/target/release/bundle/` under per-platform subfolders.

## Icons

Provider brand logos come from [@lobehub/icons](https://github.com/lobehub/lobe-icons) (MIT). All trademarks belong to their respective owners.

## License

MIT
