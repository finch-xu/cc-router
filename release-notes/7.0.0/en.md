This is a major release introducing the Quiet theme as the new default, redesigning Live routing, Request logs, Subscriptions, and Statistics, and adding custom virtual models, desktop Agent Monitoring, and network timing breakdowns. Users who have not manually picked a theme switch to Quiet on upgrade (saved choices remain unchanged), and Agent Monitoring is disconnected by default.

## Features
- **Quiet theme as default**: Adds the flat, layered Quiet theme and enables it by default for new installs and users without a saved theme; existing selections stay intact, and you can switch back to Sketch, Classic, or Win2000 from Settings or the sidebar.
- **Custom virtual models**: In addition to built-in models, create custom virtual models starting with `model-` under Virtual models, mapping multiple subscription routes and upstream models across the same or different subscriptions.
  - Included in configuration backup export and import; when a name conflicts on import, the local custom virtual model is preserved with a notice.
- **Live routing redesign**: Rebuilt around real traffic with a live flow graph (clients → virtual models → subscriptions), 5 / 15 / 60-minute window switching, pause control, core KPIs, request purpose classification, token usage, and subscription load views.
  - Backfills from the database so the live list is no longer empty after a restart; routing reference details move to Setup guide.
- **Request logs redesign and full-page details**: Groups entries by day using the shared Live routing row layout, folding retry attempts under the final request; adds time range, purpose, and session filters, keyword search, URL-preserved filter state, and purpose, session, and trace ID columns in CSV export.
  - Replaces the details dialog with a full-page view supporting newer/older navigation within the active filter, showing the retry chain, timing breakdown, tokens, neighboring session requests, and all three header tables.
- **Network timing breakdown**: Records local prep, DNS, connect (TCP and TLS), time to first response header, connection reuse, proxy usage, and upstream address per attempt, combining network and model segments into a single timing bar on the details page. List rows highlight network times over 500 ms, and the Terminal UI (TUI) details add a network section; older logs keep the two-segment bar.
- **Subscriptions and Statistics redesign**: Reworks both pages for denser, clearer information hierarchy.
  - Subscriptions adds a top summary bar and Needs attention banner, single-row cards showing cooldown status, bound virtual models, 24-hour activity strips, tightest quota, and inline enable toggles, plus in-place expansion for slot models and connection info; adds Duplicate subscription to clone connection settings and API Key without routing bindings.
  - Statistics unifies under a page-level range with six period-over-period KPIs, switchable request/token/latency charts, and breakdowns by virtual model/subscription/client, request purpose, subscription performance, top tools, and latency distribution, each linking straight into filtered Request logs.
- **Agent Monitoring and client sessions** (desktop only, disconnected by default): Adds an Agent Monitoring page in the sidebar to connect, pause, or clear local session monitoring plugins for Claude Code and Codex CLI; reopen the client after connecting, and trust the plugin in `/hooks` on first use in Codex.
  - Once enabled, Client sessions at the bottom of Live routing displays live session status, context window usage, sub-agents, prompt cache hit rate, client limits, and cumulative usage.
- **ChatGPT Codex browser sign-in and caching**: Adds Browser sign-in (Recommended) using OAuth PKCE with a local `localhost:1455` callback alongside Device code fallback; stably forwards `prompt_cache_key` on OpenAI Responses and Codex requests (plus the `session_id` header for Codex) to improve prompt cache hits, and explicitly sets `supports_websockets = false` in the recommended Codex `config.toml` template.
- **Expanded client identification**: Broadens request client fingerprinting to automatically recognize OpenAI Python/JS SDKs, curl, browsers, and common HTTP clients and scripting languages (Go, Rust, Python, Java, Node.js, Bun, Deno, PowerShell, PHP, Ruby, and more) across Live routing, Request logs, and Statistics.

## Fixes
- **ChatGPT token rotation reliability**: Fixes an issue where ChatGPT OAuth refresh token rotation could drop the stored account ID and email or reuse a stale refresh token during concurrent refreshes.

## Other
- **Dependency updates**: Updates SQLite, Tauri, and core frontend and backend dependencies.
