This release adds an entry point for Jev / System One decision models and connects Cloudflare Workers AI and Cloudflare Clef; the Live routing page now shows which clients are actually connected to cc-router. Chat requests behave the same after upgrading; exporting a receipt now plays a print animation by default, which you can turn off.

## Features
- **Jev / System One decision models**: New entry point `POST /v1/systemone` and virtual model `model-jev`. A decision model takes a state and a set of questions and returns a structured answer for each question; through cc-router it gets the same multi-subscription scheduling, cooldown, quotas and request logs. Requests are forwarded as-is, with no protocol translation.
  - Built-in upstreams: TypeSafe (Jev), Cloudflare Clef, and the System One endpoints of OpenRouter and Ollama (Ollama 0.35 or later).
  - `model-jev` can only be bound to System One subscriptions, never to chat subscriptions; the three chat entry points reject `model-jev` outright.
  - A System One subscription only has one optional Jev model slot. A request for `model-jev` is rewritten to that slot's model, or to the endpoint's example model when the slot is empty.
  - The Virtual models page has a new model-jev card with a ready-to-copy curl example that already includes your address and token; Setup guide → Generic Integration has a new Jev card.
  - Receipts include model-jev usage (shown only when there is usage); the terminal UI supports model-jev and System One subscriptions too.
- **Cloudflare Workers AI and Cloudflare Clef**: Two new built-in providers. Workers AI offers chat models such as gpt-oss, Kimi and GLM, translated through the OpenAI Chat Completions protocol; Clef is Cloudflare's decision model and only works with model-jev.
  - Both providers can connect directly or through AI Gateway. Enter your Account ID when adding the subscription, plus a Gateway ID when going through AI Gateway; both can be changed later when editing the subscription.
  - The API token needs the Workers AI permission, and also the AI Gateway permission when going through AI Gateway.
  - The Workers Free plan only allows some models (gpt-oss works; Kimi / GLM need a paid plan). Unavailable models return 403 and the subscription is marked as an auth failure.
- **Capability tags in the provider list**: When adding a subscription, each provider in the dropdown is tagged LLM (chat) or Jev (decision models); providers that support both show both tags. The terminal UI's new-subscription wizard shows them as well.
- **Fuzzy search in the model dropdown, and models not in the list**: Search ignores case and separators such as `-` `_` `.` `/`, so typing `glm5` finds `@cf/zai-org/glm-5.3`.
  - When the name you type is not in the model list, the dropdown offers a "Use …" option that fills it into the slot, without switching to manual mode.
  - With very long lists, up to 200 models are shown at a time; keep typing to narrow down.
  - A selected model that is not in the list is now marked "(not in list)".
- **Client access on Live routing** (#55, thanks @Wade11s): Detects from real requests which clients are connected to cc-router; client notes on the routing diagram that have never sent a request are grayed out.
  - The table below groups clients by family and shows the last request time, request count and token count.
  - Detection covers the request log retention period, so a client whose logs have been purged shows as not connected.
- **Print animation when exporting receipts** (on by default): After you click export, a receipt printer prints the slip in about 2 seconds; you can skip it at any time and the export itself is not affected. Turn it off in the "Export receipt" card on the Receipts page; when the system's reduce-motion setting is on, the finished state is shown right away.
- **Backup entry in the sidebar**: Opens the Backup & migration tab in Settings directly.

## Fixes
- **Rightmost request log columns cut off**: The default window width is raised from 1200 to 1360.
- **Linux AppImage showing a blank window on Ubuntu 26**: Also fixes the AppImage failing to start with a permission error when run as another user or under firejail.

## Other
- When importing a config, subscriptions of built-in providers now rebuild their connection address and extra request headers from this version's provider definitions instead of reusing the values in the backup file; the API key, model slots and other settings are imported from the backup as before.
- The Codex tab in the setup guide is now available: it is a three-step text guide for editing `config.toml` and `auth.json` by hand, with snippets generated from your port and access token, replacing the one-click editor that was never usable.
- Setup guide → Generic Integration now has an "On this page" table of contents at the top.
- The frontend dependency dompurify is upgraded to 3.4.16 to fix its security advisory.
