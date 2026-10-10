This release changes the OpenAI-style model aliases to four tiers — astra / sol / terra / luna — matching fable / opus / sonnet / haiku one to one, and the Web UI card in Settings now shows the sign-in token. The alias change is breaking: `gpt-*-sol`, `gpt-*-terra` and `gpt-*-luna` each move down one tier, and bare version names such as `gpt-5.5` and `gpt-*-mini` are no longer recognized, so if you use these names in Codex or another client, update them once after upgrading using the mapping below. `model-*` and `claude-*` names are not affected.

## Features
- **OpenAI-style model aliases are now four tiers** (breaking change): the `gpt-*` aliases clients can use are now astra / sol / terra / luna, matched by the tier segment in the name regardless of version, so `gpt-6-sol`, `gpt-6.1-sol` and `openai/gpt-6-sol` all count as the sol tier.
  - `gpt-*-astra` maps to `model-fable` (new).
  - `gpt-*-sol` maps to `model-opus` (previously `model-fable`).
  - `gpt-*-terra` maps to `model-sonnet` (previously `model-opus`).
  - `gpt-*-luna` maps to `model-haiku` (previously `model-sonnet`).
  - `gpt-5.6`, `gpt-5.5`, `gpt-5.4` and `gpt-*-mini` are no longer recognized; requests using these names go to `model-fallback`. Use the tier names above instead, or write a virtual model name such as `model-opus` directly.
- **Web UI card shows the sign-in token**: under Settings → Web & TUI → Web UI, when “Login required” is on, a “Sign-in token” row appears below the access URLs and can be copied directly.
  - Previously the token was only shown in the Security & Access tab while “Token authentication” was on, and could not be found otherwise; the hint on the web sign-in page now points to the new location too.

## Other
- **New-version notes on the Updates page**: headings, lists, bold text and links are now formatted, and only the section in the current interface language is shown, instead of the full text in all three languages as one block of plain text.
- **`GET /v1/models` model list updated**: Claude aliases now come in two versions per tier, 5.5 and 6 (e.g. `claude-opus-5-5`, `claude-opus-6`), and OpenAI aliases are one per tier (e.g. `gpt-6-sol`), 32 entries in total. Older names such as `claude-opus-4-7` still route; they just no longer appear in the list.
- The model table under Setup guide → Generic Integration and the alias hints on the Live routing page have been updated to the new rules.
