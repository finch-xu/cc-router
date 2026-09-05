<p align="center">
  <img src="assets/icon.png" alt="cc-router logo" width="160" height="160" />
</p>

<h1 align="center">cc-router</h1>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg?style=flat-square" alt="License: MIT"></a>
  <img src="https://img.shields.io/badge/Tauri-2-FFC131?style=flat-square&logo=tauri&logoColor=white" alt="Tauri 2">
  <img src="https://img.shields.io/badge/Rust-1.88+-DEA584?style=flat-square&logo=rust&logoColor=white" alt="Rust 1.88+">
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

A locally-running LLM aggregation gateway with a desktop GUI, zero-code setup: bundle your scattered `Token Plan`, `Coding Plan`, and LLM API quotas into a single virtual Plan, and plug it into Claude Code, Claude Desktop App, OpenClaw, OpenCode, Codex and more —— save money! save tokens! 100% local!

> ⚠️ Notice: this tool only switches between subscription plans you already own. Request bodies are passed through almost verbatim — no reverse engineering, no jailbreak, no circumvention. You are responsible for complying with each plan's terms of service. cc-router is intended for use with coding tools like Claude Code only; do not use it for anything else.
>
> Provider terms of service do not necessarily allow "routing a subscription key through a third-party proxy with multi-virtual-model dispatch" — especially for per-seat subscriptions like Coding Plans / Token Plans, where this pattern may trip risk controls. The author assumes no liability for any account being throttled, banned, or having its subscription cancelled as a result of using this tool.
>
> This software is provided As-Is, without warranty of any kind. The author is not liable for any direct or indirect damages arising from its use, including but not limited to abnormal quota consumption, data loss, or business interruption.

Architecture and request flow at a glance:

```text
 Claude Code    OpenCode    OpenClaw   pi ...   Codex ...      Open WebUI / Cherry Studio ...
      |             |           |         |         |                         |
      -------------------------------------         |                         |
                        |                           |                         |
                    Anthropic                    OpenAI                    OpenAI
                  Messages API                Responses API         Chat Completions API
                 (/v1/messages)              (/v1/responses)       (/v1/chat/completions)
                        |                           |                         |
                        -------------------------------------------------------
                                                  |  inbound · virtual models
                                                  |
                                              cc-router
                                        (local 127.0.0.1:23456)
                                                  |
                                                  |  outbound · real models
           -----------------------------------------------------------------------------
           |            |            |            |            |            |          |
       DeepSeek        GLM         Kimi       Anthropic     OpenAI       Gemini     ......
          API        Coding       Coding      Messages    Responses &      API
                      Plan         Plan          API      Completions
```

Highlights:

- **Three inbound protocols, any tool plugs in** — Anthropic Messages / OpenAI Responses / OpenAI Chat Completions are exposed side by side, so Claude Code, Codex, OpenClaw, Hermes Agent, Kimi Code, ZCode, Cherry Studio and the like connect without any changes
- **Three outbound protocols, every subscription in one router** — 24 built-in provider presets (DeepSeek, Qwen, Kimi, MiMo, MiniMax, GLM, Claude, OpenAI, Gemini, …), plus any Anthropic / OpenAI / Gemini-compatible endpoint you bring yourself
- **Pool every token you have** — sequential / round-robin / session-affinity dispatch with automatic switching and failover
- **Usage receipts** — export your token usage as a "supermarket receipt" in one click, handy for sharing or keeping records
- **Fully translated UI** — 简体中文 / English / 日本語, follows your system locale or pick manually in Settings
- **Virtual model aliases** — each of fable / opus / sonnet / haiku accepts multiple names; for opus that's `model-opus` / `claude-opus-4-7` / `anthropic/model-opus` / `anthropic/claude-opus-4-7`, all routed to the same virtual model — pick whatever naming your tool prefers
- **Local HTTPS** — generate a self-signed CA and server cert in one click so HTTPS-only clients can talk to cc-router too; see the [setup guide](https://ccrouter.app/docs/claude-desktop-integration/)
- **Claude Desktop App support** — combine local HTTPS with the virtual-model aliases above and Anthropic's official desktop app can route through cc-router's aggregated subscriptions; see the [setup guide](https://ccrouter.app/docs/claude-desktop-integration/)

<table align="center">
  <tr>
    <td width="40%"><img src="assets/screenshot-routing.png" alt="cc-router live routing page" /></td>
    <td width="40%"><img src="assets/screenshot-models.png" alt="cc-router virtual model configuration page" /></td>
    <td width="20%" rowspan="2"><img src="assets/screenshot-receipts.png" alt="cc-router usage receipts long screenshot" /></td>
  </tr>
  <tr>
    <td width="40%"><img src="assets/screenshot-receipts-page.png" alt="cc-router usage receipts page" /></td>
    <td width="40%"><img src="assets/screenshot-logs.png" alt="cc-router request logs page" /></td>
  </tr>
</table>

## Integration Guide

Every AI Agent / Coding Agent tool below can connect to cc-router and use all the LLM plans you own:

<p>
<a href="https://ccrouter.app/docs/getting-started/" target="_blank" rel="noopener">Claude Code cli</a> · 
<a href="https://ccrouter.app/docs/claude-desktop-integration/" target="_blank" rel="noopener">Claude Desktop App</a> · 
<a href="https://ccrouter.app/docs/codex-integration/" target="_blank" rel="noopener">OpenAI Codex cli</a> · 
<a href="https://ccrouter.app/docs/codex-integration/" target="_blank" rel="noopener">OpenAI Codex Desktop App</a> · OpenCode · OpenClaw · Kimi code cli · pi coding agent, and many more.
</p>

## Quick Start

1. Download the installer for your platform from Releases and run it.
2. Add subscriptions from your providers, bind real models to the virtual models, and pick a dispatch mode.
3. Paste the generated config into Claude Code or any other tool and you're done.

## Using with Claude Code

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
|  `model-fable` |  `anthropic/model-fable` `anthropic/claude-fable*` `claude-fable*` `gpt-5.6` `gpt-*-sol` `openai/gpt-5.6` `openai/gpt-*-sol` |
|  `model-opus` |  `anthropic/model-opus` `anthropic/claude-opus*` `claude-opus*` `gpt-5.5` `gpt-*-terra` `openai/gpt-5.5` `openai/gpt-*-terra` |
|  `model-sonnet` |  `anthropic/model-sonnet` `anthropic/claude-sonnet*` `claude-sonnet*` `gpt-5.4` `gpt-*-luna` `openai/gpt-5.4` `openai/gpt-*-luna` |
|  `model-haiku` |  `anthropic/model-haiku` `anthropic/claude-haiku*` `claude-haiku*`  `gpt-*-mini` `openai/gpt-*-mini` |

> `claude-opus*` is a wildcard (prefix match): you can pass any model name that fits the pattern and it will be normalized to the `model-opus` virtual model — e.g. `claude-opus-4-8`, `claude-opus-4-7-20260101`, and `claude-opus-100` all work. `gpt-*-sol`-style aliases match by tier segment: `gpt-5.6-sol`, `gpt-6-sol`, and `gpt-5.6-sol-20261201` all hit the sol tier (same for terra/luna/mini).

## Inbound & Outbound

cc-router sits between your tools and the LLM providers: tools connect on the **inbound** side, requests leave through the **outbound** side. Each side speaks three mainstream LLM APIs, and any combination works — for example, Codex comes in through the OpenAI Responses inbound and is ultimately answered by DeepSeek's Anthropic endpoint.

### Inbound: how your tools connect to cc-router

All three inbound endpoints share the same subscriptions, virtual models, quotas and session affinity; the "Entry endpoint" column in the request log shows which one each request came through. Expand the section matching the protocol your tool speaks:

<details>
<summary><b>Anthropic Messages</b> <code>/v1/messages</code> — Claude Code, Claude Desktop, OpenCode, OpenClaw, pi, Kimi code cli, etc.</summary>

| Setting | Value |
|---|---|
| Base URL | `http://127.0.0.1:23456` (no `/v1` — the tool appends `/v1/messages` itself) |
| Auth | `x-api-key: <token>` or `Authorization: Bearer <token>`, i.e. Claude Code's `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` |
| Model name | `model-fable` / `model-opus` / `model-sonnet` / `model-haiku`, or any alias from the table above (including the `anthropic/` prefix) |

- This is the primary inbound: requests are passed through verbatim with no protocol translation, so thinking, `output_config.effort`, `cache_control`, images and tool calls all keep their native Anthropic semantics.
- The full Claude Code env example is in "Using with Claude Code" above; Claude Desktop needs local HTTPS, see the [setup guide](https://ccrouter.app/docs/claude-desktop-integration/).
- Session affinity keys on the `x-claude-code-session-id` header first, then `metadata.user_id`.
- With "Forward client headers" enabled on a subscription's edit page, whitelisted headers such as `anthropic-beta` / `anthropic-version` are forwarded to that upstream as-is; set per subscription, off by default.

</details>

<details>
<summary><b>OpenAI Responses</b> <code>/v1/responses</code> — Codex CLI, Codex Desktop App and other Responses clients</summary>

| Setting | Value |
|---|---|
| Base URL | `http://127.0.0.1:23456/v1` |
| API Key | the token from cc-router's Settings page; Codex reads it from `OPENAI_API_KEY` or `~/.codex/auth.json` |
| Model name | `gpt-5.6` / `gpt-5.5` / `gpt-5.4` / `gpt-5.4-mini`, or the `openai/` prefix and `gpt-*-sol/terra/luna/mini` tier names, mapping to fable / opus / sonnet / haiku respectively; the `model-*` form is accepted too |

`~/.codex/config.toml` snippet (the "Integrations" tab in Settings can write it for you and backs up the original file; then launch with `codex -p cc-router`):

```toml
[model_providers.cc-router]
name = "cc-router"
base_url = "http://127.0.0.1:23456/v1"
wire_api = "responses"
env_key = "OPENAI_API_KEY"

[profiles.cc-router]
model_provider = "cc-router"
model = "model-sonnet"
```

- Requests are translated internally into Anthropic Messages: `instructions` and developer messages are merged into system, `reasoning.effort` maps to the thinking budget, `max_output_tokens` maps to `max_tokens` (default 4096, automatically raised to cover the thinking budget).
- Reasoning is bidirectional: upstream thinking comes back as a signed reasoning item; send it back unchanged on the next turn to keep multi-turn reasoning context.
- Image input is not supported, nor are OpenAI-only tools such as `file_search` / `web_search` / `computer_use`; `parallel_tool_calls` is ignored.
- Session affinity keys on `prompt_cache_key`, then the `session_id` header; Codex sends both.
- Step-by-step instructions are in the [setup guide](https://ccrouter.app/docs/codex-integration/).

</details>

<details>
<summary><b>OpenAI Chat Completions</b> <code>/v1/chat/completions</code> — Open WebUI, Cherry Studio, Cline, LobeChat, etc.</summary>

For tools that only speak OpenAI Chat Completions — Open WebUI, Cherry Studio, Cline, LobeChat and the like — point their "OpenAI-compatible" endpoint at cc-router:

| Setting | Value |
|---|---|
| Base URL | `http://127.0.0.1:23456/v1` (some tools want it without `/v1`; follow the tool's hint) |
| API Key | the token from cc-router's Settings page (any non-empty value when auth is disabled) |
| Model name | `model-fable` / `model-opus` / `model-sonnet` / `model-haiku`, or aliases such as `gpt-5.6` / `gpt-5.5` / `gpt-5.4` / `gpt-5.4-mini`; `GET /v1/models` lists them |

Behavior notes:

- Requests are translated internally into Anthropic Messages and go through the same dispatch, so subscriptions, virtual models, quotas and session affinity all apply; the request log shows `/v1/chat/completions` in the "Entry endpoint" column.
- Upstream thinking is returned in the `reasoning_content` field (the DeepSeek convention; mainstream clients render it collapsed). Any `reasoning_content` the client sends back in the history is dropped, without affecting the conversation.
- Images work with both `data:` base64 and `http(s)` `image_url`; tool calls work in both directions; streaming responses always end with a `usage` frame.
- The legacy `functions` / `function_call` fields are not supported and return 400; use `tools` / `tool_choice` instead.
- `n>1`, `logprobs` and JSON-Schema enforcement via `response_format` are silently ignored.
- Session affinity (sticky) keys on the `user` field first, then the `x-session-id` header, falling back to the first user message.
- When the response contains tool calls, `finish_reason` is always `tool_calls`, so clients can rely on it to decide whether to run tools.

</details>

### Outbound: how cc-router connects to providers

Outbound is grouped into three protocol families, plus a fourth group of OAuth-based subscription accounts. Built-in provider presets and custom endpoints take the same path — the presets just come with the address, auth scheme and model list pre-filled. The authoritative list of built-in providers is the "Add subscription" page in the app; the descriptor files live in [`src-tauri/providers/`](src-tauri/providers/), and PRs are welcome.

<details>
<summary><b>Anthropic Messages compatible</b> — primary path, requests passed through verbatim</summary>

- Built-in: Anthropic official, DeepSeek, Zhipu GLM, Moonshot Kimi, MiniMax, Xiaomi MiMo, Alibaba Cloud Bailian, Volcengine Ark, Tencent Cloud, Baidu Qianfan, Stepfun, ModelScope, UCloud, Fireworks, OpenRouter, xAI Grok, Aiberm, Shenma relay, Ollama and more, covering each vendor's Token Plan / Coding Plan / Agent Plan subscriptions as well as pay-as-you-go APIs
- Custom: any Anthropic Messages-compatible endpoint (relays, self-hosted gateways, …) — just a Base URL and a key
- No protocol translation: thinking, `output_config.effort`, `cache_control`, images and tool calls all keep their native Anthropic semantics. **If a vendor offers a native Anthropic endpoint, prefer this path** — the translated paths always lose something

</details>

<details>
<summary><b>OpenAI compatible</b> <code>/v1/responses</code> · <code>/v1/chat/completions</code> — protocol translation</summary>

- Built-in: OpenAI official API (GPT-5 / o3 / 4.1 and other reasoning models)
- Custom: any OpenAI Responses or Chat Completions-compatible endpoint, e.g. one-api / new-api relays, Groq, Together, local vLLM / llama.cpp
- cc-router translates Anthropic Messages into the target protocol before sending: Anthropic thinking ↔ OpenAI reasoning are mapped in both directions with multi-turn reasoning context fed back automatically; `reasoning_content` from Chat Completions (DeepSeek R1, etc.) is handed to Claude Code as thinking blocks
- Anything the translation layer cannot express (such as `cache_control`) is dropped, so vendors with a native Anthropic endpoint belong in the group above, not here

</details>

<details>
<summary><b>Gemini compatible</b> <code>generateContent</code> · <code>/v1beta/interactions</code> — protocol translation</summary>

- Built-in: Google AI Studio (generateContent, pay-as-you-go + free quota) and Google Gemini Interactions API (the new unified endpoint)
- Custom: any Gemini generateContent-compatible endpoint (`messages_path` uses the `{model}` placeholder), or an Interactions-compatible endpoint (the model goes in the request body, no placeholder needed)
- Thinking is mapped in both directions, and thought signatures are carried automatically across tool-call round trips

</details>

<details>
<summary><b>Subscription-account outbound (OAuth)</b> — Codex (ChatGPT Plus/Pro), Kiro (AWS)</summary>

- No API key: sign in via OAuth device code and use your ChatGPT subscription / Kiro's free Claude quota as an outbound
- **Grey area with account-suspension risk; not recommended as your main path** — use it only as a fallback or on a secondary account. The author assumes no liability for any resulting throttling, bans or subscription cancellation

</details>

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
<summary>Scheduling mode: sequential, round-robin, or session affinity?</summary>

- **Sequential** — drain account A first, then switch to B. Better cache hit rate; ideal for **squeezing every token out of two small GLM Coding Plans**
- **Round-robin** — both accounts share the load. Caveat: cross-account caches are independent, so you'll burn slightly more quota in exchange for true load balancing
- **Session affinity** — each Claude Code session sticks to one subscription while different sessions are dealt out round-robin. Balanced like round-robin, but the prompt cache survives, and parallel sub-agents share it too. Automatically moves to another subscription when the pinned one is rate-limited or fails. **Recommended when several subscriptions are healthy and you care about cache hits.**

</details>

## Development

- Tauri 2
- Tailwind 4
- React 19

Prerequisites: Node.js ≥ 20 (pnpm recommended), Rust ≥ 1.88 (the latest stable via rustup is recommended), Xcode Command Line Tools (macOS).

```bash
pnpm install
pnpm tauri dev      # runs frontend + Rust backend + proxy in one process
```

First launch opens the onboarding flow:

1. Add a subscription (pick provider → endpoint → paste API key → auto-fetch the model list).
2. Bind the subscription to all four virtual models in one click.
3. Copy the generated env snippet into your `~/.claude/settings.json`.

## Build

```bash
pnpm tauri build
```

Artifacts land in `src-tauri/target/release/bundle/` under per-platform subfolders.

## Windows Development Notes

<details>
<summary>Expand when you hit trouble developing or building on Windows (can save you hours)</summary>

**1. The MSVC toolchain is required — GNU will not work**

Tauri's Windows bundling depends on MSVC, and CI builds with `x86_64-pc-windows-msvc` too. Verify:

```powershell
rustup show          # the active toolchain should end with -msvc
rustc --version      # should be >= 1.88
```

**2. Visual Studio Build Tools are required**

The MSVC toolchain needs the C++ compiler and the Windows SDK; without them you get `link.exe not found`. Install [Build Tools for Visual Studio](https://visualstudio.microsoft.com/downloads/) with the **"Desktop development with C++"** workload, then reopen your terminal.

**3. `rustc --version` disagrees with `rustup show`? Check PATH**

If you ever installed a standalone Rust via msi / scoop / a Visual Studio component, it may sit ahead of the rustup shim on PATH, silently making `rustup default stable` a no-op:

```powershell
Get-Command rustc, cargo -All | Select-Object Source
```

`Source` should point at `%USERPROFILE%\.cargo\bin`. If it does not, move that directory to the front of PATH (`rundll32 sysdm.cpl,EditEnvironmentVariables`) or uninstall the standalone copy.

**4. Leftover dev server on port 1420**

Ctrl+C often fails to kill the whole `tauri dev` process tree on Windows, so the next run reports `Port 1420 is already in use` (`strictPort: true` deliberately refuses to fall back to another port, because `devUrl` is pinned to 1420):

```powershell
Get-NetTCPConnection -LocalPort 1420 -State Listen |
  Select-Object -ExpandProperty OwningProcess -Unique |
  ForEach-Object { Stop-Process -Id $_ -Force }
```

</details>

## Adding a new provider

If you use **Claude Code**, this repo ships a `SKILL` named `new-provider`. Run it with the official docs URL or endpoint info of the target provider, and it will scaffold the YAML and wire up the related changes for you.

## License

Released under the [MIT](LICENSE) license.

Fonts: the 9 fonts used by the receipt themes are all under the SIL Open Font License 1.1; attributions and the full license text are in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

Icons: provider brand logos come from [@lobehub/icons](https://github.com/lobehub/lobe-icons) (MIT). All trademarks belong to their respective owners.
