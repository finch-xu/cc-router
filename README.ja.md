<p align="center">
  <img src="assets/icon.png" alt="cc-router logo" width="160" height="160" />
</p>

<h1 align="center">cc-router</h1>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg?style=flat-square" alt="License: MIT"></a>
  <img src="https://img.shields.io/badge/Tauri-2-FFC131?style=flat-square&logo=tauri&logoColor=white" alt="Tauri 2">
  <img src="https://img.shields.io/badge/Rust-1.90+-DEA584?style=flat-square&logo=rust&logoColor=white" alt="Rust 1.90+">
  <img src="https://img.shields.io/badge/React-19-61DAFB?style=flat-square&logo=react&logoColor=white" alt="React 19">
  <img src="https://img.shields.io/badge/TypeScript-5-3178C6?style=flat-square&logo=typescript&logoColor=white" alt="TypeScript 5">
  <img src="https://img.shields.io/badge/Tailwind-4-06B6D4?style=flat-square&logo=tailwindcss&logoColor=white" alt="Tailwind CSS">
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows%20%7C%20Linux-lightgrey?style=flat-square" alt="Platform">
</p>

<p align="center">
  <a href="README.md">中文</a> · <a href="README.en.md">English</a> · <strong>日本語</strong>
</p>

<p align="center">
  <a href="https://ccrouter.app/docs/" target="_blank" rel="noopener">📖 中文文档</a> ·
  <a href="https://ccrouter.app/en/docs/" target="_blank" rel="noopener">📖 English Docs</a> ·
  <a href="https://ccrouter.app/ja/docs/" target="_blank" rel="noopener">📖 日本語ドキュメント</a> ·
  <a href="https://deepwiki.com/finch-xu/cc-router" target="_blank" rel="noopener">🤖 DeepWiki</a> ·
  <a href="https://ccrouter.app" target="_blank" rel="noopener">🌐 公式サイト ccrouter.app</a>
</p>

ローカルで動く LLM 集約ゲートウェイ。GUI デスクトップアプリ、ノーコードで導入。バラバラの `Token Plan`、`Coding Plan`、LLM API クォータを 1 つの仮想 Plan にまとめ、Claude Code、Claude Desktop App、OpenClaw、OpenCode、Codex などのツールにワンクリックで接続——コスト削減！トークン節約！完全ローカル動作！

> ⚠️ 注意: 本ツールは「すでに保有しているサブスクリプションプラン間の自動切り替え」のみを目的としています。リクエストボディはほぼそのまま透過するだけで、リバースエンジニアリングや脱獄、回避行為は一切含みません。各プランの利用規約は利用者ご自身で遵守してください。Claude Code などのコーディングツール用途専用であり、それ以外の用途には使用しないでください。
>
> 各プロバイダの利用規約が「サブスクリプションキーをサードパーティのプロキシ経由でルーティングし、複数仮想モデルでディスパッチする」用途を明示的に許可しているとは限りません。特に Coding Plan / Token Plan のような per-seat サブスクリプションでは、リスク管理機構に検知される可能性があります。本ツールの使用に起因するアカウントのレート制限、BAN、サブスクリプション解約等について、作者は一切の責任を負いません。
>
> 本ソフトウェアは As-Is（現状有姿）で提供され、明示・黙示を問わずいかなる保証もしません。クォータの異常消費、データ損失、業務中断を含む直接・間接の損害について作者は責任を負いません。
>
> cc-router は独立したサードパーティ製のオープンソースプロジェクトです。Anthropic とは一切関係がなく、同社の許諾・推奨を受けたものでも、Claude / Claude Code の公式アプリでもありません。記載している Anthropic、Claude、Claude Code は Anthropic の商標であり、互換対象を示す目的でのみ使用しています。

アーキテクチャとリクエストの流れ：

```text
 Claude Code  OpenCode   pi ...      Codex ...           Open WebUI ...       自作コード / Agent
      |           |         |            |                      |                      |
      -----------------------            |                      |                      |
                 |                       |                      |                      |
             Anthropic                OpenAI                 OpenAI                   Jev
           Messages API            Responses API      Chat Completions API        System One
          (/v1/messages)          (/v1/responses)    (/v1/chat/completions)     (/v1/systemone)
                 |                       |                      |                      |
                 -----------------------------------------------------------------------
                                                    |  入口 · 仮想モデル
                                                    |
                                                cc-router
                                       (ローカル 127.0.0.1:23456)
                                                    |
                                                    |  出口 · 実モデル
              ------------------------------------------------------------------------------
              |          |          |          |          |          |          |          |
          DeepSeek      GLM       Kimi     Anthropic   OpenAI     Gemini       Jev      ......
             API      Coding     Coding    Messages  Responses &    API    System One
                       Plan       Plan        API    Completions
```

機能ハイライト：

- **入口は 3 プロトコル、どのツールもそのまま接続** —— Anthropic Messages / OpenAI Responses / OpenAI Chat Completions の 3 エンドポイントを同時に公開。Claude Code、Codex、OpenClaw、Hermes Agent、Kimi Code、ZCode、Cherry Studio などが改造なしで接続できます。さらに `/v1/systemone` 入口は Jev（System One）意思決定モデル専用で、独立した仮想モデル `model-jev` を使います
- **出口は 3 プロトコル、全サブスクを一括ディスパッチ** —— 24 社のプロバイダプリセットを内蔵（DeepSeek・Qwen・Kimi・MiMo・MiniMax・GLM・Claude・OpenAI・Gemini など）。Anthropic / OpenAI / Gemini 互換のエンドポイントなら何でも追加可能
- **手持ちのトークンをすべて集約** —— 順次 / ラウンドロビン / セッション親和のディスパッチ、自動切替とフェイルオーバー
- **利用レシート** —— トークン使用量を「スーパーのレシート」風の画像にワンクリックで書き出し。共有にも記録にも便利
- **3 言語完全翻訳** —— 简体中文 / English / 日本語、システム言語追従または設定画面で手動切替
- **デスクトップ / Web / ターミナルの 3 つの UI** —— デスクトップ GUI に加えて、デスクトップと同じ管理画面をブラウザで開ける [Web UI](#web-uiオプションデフォルト無効) や、コマンドラインからサブスクリプションの管理・リアルタイムルーティング・リクエストログの確認ができる[ターミナル UI（TUI）](#ターミナル-uiオプションデフォルト無効) も使えます
- **仮想モデルのエイリアス対応** —— fable / opus / sonnet / haiku の各スロットが複数の命名を識別。opus を例にすると `model-opus` / `claude-opus-4-7` / `anthropic/model-opus` / `anthropic/claude-opus-4-7` がすべて同じ仮想モデルにルーティングされ、ツール側の命名規約に左右されません
- **ローカル HTTPS** —— ワンクリックで自己署名 CA とサーバー証明書を生成し、HTTPS しか受け付けないクライアントからも cc-router を呼び出せます。詳細は[設定ガイド](https://ccrouter.app/docs/claude-desktop-integration/)を参照
- **Claude Desktop App 対応** —— ローカル HTTPS と仮想モデルエイリアスを組み合わせることで、Anthropic 公式デスクトップアプリから cc-router で集約した複数サブスクへ直接接続できます。詳細は[設定ガイド](https://ccrouter.app/docs/claude-desktop-integration/)を参照

<table align="center">
  <tr>
    <td align="center" width="50%"><img src="assets/screenshots/ja/routing.png" alt="cc-router リアルタイムルーティング画面" /><br /><sub><b>リアルタイムルーティング</b> · リクエストの流れを一目で</sub></td>
    <td align="center" width="50%"><img src="assets/screenshots/ja/models.png" alt="cc-router 仮想モデル設定画面" /><br /><sub><b>仮想モデル</b> · ドラッグでサブスクの優先度を設定</sub></td>
  </tr>
  <tr>
    <td align="center" width="50%"><img src="assets/screenshots/ja/logs.png" alt="cc-router リクエストログ画面" /><br /><sub><b>リクエストログ</b> · リクエストごとのモデル・レイテンシ・トークン</sub></td>
    <td align="center" width="50%"><img src="assets/screenshots/ja/receipts.png" alt="cc-router 利用レシート画面" /><br /><sub><b>利用レシート</b> · トークン使用量をレシートで書き出し</sub></td>
  </tr>
  <tr>
    <td align="center" width="50%"><img src="assets/screenshots/ja/tui-overview.png" alt="cc-router ターミナル UI 概要画面" /><br /><sub><b>ターミナル UI 概要</b> · サブスクの状態と上限を一画面で</sub></td>
    <td align="center" width="50%"><img src="assets/screenshots/ja/tui-log-detail.png" alt="cc-router ターミナル UI リクエスト詳細画面" /><br /><sub><b>ターミナル UI リクエスト詳細</b> · 1 件のリクエストの全記録</sub></td>
  </tr>
</table>

## 連携ガイド

以下の AI Agent / Coding Agent ツールはいずれも cc-router に接続でき、ご契約中のすべての LLM プランを利用できます：

<p>
<a href="https://ccrouter.app/docs/getting-started/" target="_blank" rel="noopener">Claude Code cli</a> · 
<a href="https://ccrouter.app/docs/claude-desktop-integration/" target="_blank" rel="noopener">Claude Desktop App</a> · 
<a href="https://ccrouter.app/docs/codex-integration/" target="_blank" rel="noopener">OpenAI Codex cli</a> · 
<a href="https://ccrouter.app/docs/codex-integration/" target="_blank" rel="noopener">OpenAI Codex Desktop App</a> · OpenCode · OpenClaw · Kimi code cli · pi coding agent など、ほかにも多数。
</p>

## クイックスタート

1. 下表からお使いのプラットフォーム向けインストーラをダウンロードして実行します。
2. 各プロバイダのサブスクリプションを追加し、仮想モデルに実モデルを紐付けてディスパッチモードを選択します。
3. 生成された設定を Claude Code などのツールに貼り付ければ完了です。

| OS | アーキテクチャ | パッケージ | ダウンロード |
|---|---|---|---|
| macOS | Apple Silicon | `cc-router_macOS-arm64.dmg` | [グローバル](https://github.com/finch-xu/cc-router/releases/latest/download/cc-router_macOS-arm64.dmg) · [中国ミラー](https://d.cc-router.catonthe.top/latest/cc-router_macOS-arm64.dmg) |
| macOS | Intel | `cc-router_macOS-x64.dmg` | [グローバル](https://github.com/finch-xu/cc-router/releases/latest/download/cc-router_macOS-x64.dmg) · [中国ミラー](https://d.cc-router.catonthe.top/latest/cc-router_macOS-x64.dmg) |
| Windows | x64 | `cc-router_windows-x64-setup.exe` | [グローバル](https://github.com/finch-xu/cc-router/releases/latest/download/cc-router_windows-x64-setup.exe) · [中国ミラー](https://d.cc-router.catonthe.top/latest/cc-router_windows-x64-setup.exe) |
| Windows | x64 (MSI) | `cc-router_windows-x64.msi` | [グローバル](https://github.com/finch-xu/cc-router/releases/latest/download/cc-router_windows-x64.msi) · [中国ミラー](https://d.cc-router.catonthe.top/latest/cc-router_windows-x64.msi) |
| Windows | arm64 | `cc-router_windows-arm64-setup.exe` | [グローバル](https://github.com/finch-xu/cc-router/releases/latest/download/cc-router_windows-arm64-setup.exe) · [中国ミラー](https://d.cc-router.catonthe.top/latest/cc-router_windows-arm64-setup.exe) |
| Linux | x64 | `cc-router_linux-x64.AppImage` | [グローバル](https://github.com/finch-xu/cc-router/releases/latest/download/cc-router_linux-x64.AppImage) · [中国ミラー](https://d.cc-router.catonthe.top/latest/cc-router_linux-x64.AppImage) |
| Linux | x64 | `cc-router_linux-x64.deb` | [グローバル](https://github.com/finch-xu/cc-router/releases/latest/download/cc-router_linux-x64.deb) · [中国ミラー](https://d.cc-router.catonthe.top/latest/cc-router_linux-x64.deb) |
| Linux | arm64 | `cc-router_linux-arm64.AppImage` | [グローバル](https://github.com/finch-xu/cc-router/releases/latest/download/cc-router_linux-arm64.AppImage) · [中国ミラー](https://d.cc-router.catonthe.top/latest/cc-router_linux-arm64.AppImage) |
| Linux | arm64 | `cc-router_linux-arm64.deb` | [グローバル](https://github.com/finch-xu/cc-router/releases/latest/download/cc-router_linux-arm64.deb) · [中国ミラー](https://d.cc-router.catonthe.top/latest/cc-router_linux-arm64.deb) |

> 2 つのリンクは同一ファイルです。中国本土のユーザーは中国ミラーの方が高速です。リンクは常に最新版を指し、過去のバージョンは [Releases](https://github.com/finch-xu/cc-router/releases) ページにあります。Linux では AppImage を推奨します（アプリ内自動更新に対応）。deb は手動で再ダウンロードして更新してください。

## Claude Code での利用

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

`OPUS_MODEL` が `1m` コンテキストに対応している場合、`model-opus[1m]` に設定すると Claude Code のロングコンテキストをフルに活用できます。

LiteLLM 形式の `anthropic/` プレフィックスにも対応しています: `anthropic/model-opus` / `anthropic/model-sonnet` / `anthropic/model-haiku` はプレフィックスなしの記法と等価で、Anthropic プロトコルを認識させるためにプロバイダプレフィックスが必要なツールとの連携が容易になります。

仮想モデルとエイリアス:

| 仮想モデル | エイリアス |
|---|---|
|  `model-fable` |  `anthropic/model-fable` `anthropic/claude-fable*` `claude-fable*` `gpt-*-astra` `openai/gpt-*-astra` |
|  `model-opus` |  `anthropic/model-opus` `anthropic/claude-opus*` `claude-opus*` `gpt-*-sol` `openai/gpt-*-sol` |
|  `model-sonnet` |  `anthropic/model-sonnet` `anthropic/claude-sonnet*` `claude-sonnet*` `gpt-*-terra` `openai/gpt-*-terra` |
|  `model-haiku` |  `anthropic/model-haiku` `anthropic/claude-haiku*` `claude-haiku*` `gpt-*-luna` `openai/gpt-*-luna` |
|  `model-jev` |  `anthropic/model-jev` `openai/model-jev`（`/v1/systemone` 入口でのみ有効。会話用の入口では 400 を返します） |

> `claude-opus*` はワイルドカード（前方一致）です。パターンに一致するモデル名を渡せば、すべて仮想モデル `model-opus` に正規化されます。例えば `claude-opus-4-8`、`claude-opus-4-7-20260101`、`claude-opus-100` などはすべて問題なく動作します。`gpt-*-sol` 系のエイリアスはティアセグメントで一致します: `gpt-6-sol`、`gpt-6.1-sol`、`gpt-5.6-sol-20261201` はいずれも sol ティアに一致します（astra/terra/luna も同様）。認識するのはティア名だけです。`gpt-5.5` のようなバージョン番号のみの名前や `gpt-*-mini` は対応付けられず、`model-fallback` に回ります。

## 入口と出口

cc-router はツールと LLM プロバイダの間に入ります。ツールは**入口**から接続し、リクエストは**出口**からプロバイダへ送られます。入口・出口それぞれが主要 3 種類の LLM API に対応しており、組み合わせは自由です——たとえば Codex が OpenAI Responses の入口から入り、最終的に DeepSeek の Anthropic エンドポイントが応答する、といった構成も可能です。このほか Jev（System One）意思決定モデル専用の経路があり、入口・出口ともに `/v1/systemone` で、そのまま透過します。

### 入口：ツールから cc-router への接続

3 つの会話用入口はサブスクリプション・仮想モデル・クォータ・セッション親和を共有します。4 つ目の入口 `/v1/systemone` は Jev 意思決定モデル専用で、`model-jev` のみを使います。リクエストログの「受信エンドポイント」列で、各リクエストがどの入口から来たかを確認できます。お使いのツールが対応するプロトコルのセクションを展開してください：

<details>
<summary><b>Anthropic Messages</b> <code>/v1/messages</code> —— Claude Code、Claude Desktop、OpenCode、OpenClaw、pi、Kimi code cli など</summary>

| 設定項目 | 値 |
|---|---|
| Base URL | `http://127.0.0.1:23456`（`/v1` は付けない。ツール側が `/v1/messages` を補います） |
| 認証 | `x-api-key: <token>` または `Authorization: Bearer <token>`。Claude Code の `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` に相当 |
| モデル名 | `model-fable` / `model-opus` / `model-sonnet` / `model-haiku`、または上記エイリアス表の任意の記法（`anthropic/` プレフィックス含む） |

- これがメインの入口です。リクエストはプロトコル変換なしでそのまま透過され、thinking、`output_config.effort`、`cache_control`、画像、ツール呼び出しはすべて Anthropic ネイティブの意味論で動作します。
- Claude Code の完全な env 例は上記「Claude Code での利用」を参照。Claude Desktop はローカル HTTPS が必要です。[設定ガイド](https://ccrouter.app/docs/claude-desktop-integration/)を参照してください。
- セッション親和は `x-claude-code-session-id` ヘッダーを優先し、次に `metadata.user_id` でセッションを識別します。
- サブスクリプション編集画面で「クライアントヘッダーの転送」を有効にすると、`anthropic-beta` / `anthropic-version` などのホワイトリストヘッダーがその上流へそのまま転送されます。サブスクリプションごとの設定で、既定では無効です。

</details>

<details>
<summary><b>OpenAI Responses</b> <code>/v1/responses</code> —— Codex CLI、Codex Desktop App、その他の Responses クライアント</summary>

| 設定項目 | 値 |
|---|---|
| Base URL | `http://127.0.0.1:23456/v1` |
| API Key | cc-router 設定画面の token。Codex は `OPENAI_API_KEY` または `~/.codex/auth.json` から読み込みます |
| モデル名 | `gpt-*-astra` / `gpt-*-sol` / `gpt-*-terra` / `gpt-*-luna` のティア名（例: `gpt-6-sol`、`openai/` プレフィックスも可）。それぞれ fable / opus / sonnet / haiku に対応。`model-*` 記法も受け付けます |

`~/.codex/config.toml` の断片（詳しい手順はアプリの「セットアップガイド → Codex」にあります。その後 `codex -p cc-router` で起動）：

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

- リクエストは内部で Anthropic Messages に変換されます: `instructions` と developer メッセージは system に統合、`reasoning.effort` は thinking 予算に、`max_output_tokens` は `max_tokens` にマッピング（既定 4096、thinking 予算をカバーするよう自動で引き上げ）。
- reasoning は双方向: 上流の thinking は署名付き reasoning item として返され、次のターンでそのまま送り返せばマルチターンの推論コンテキストが維持されます。
- 画像入力は非対応。`file_search` / `web_search` / `computer_use` などの OpenAI 専用ツールも非対応。`parallel_tool_calls` は無視されます。
- セッション親和は `prompt_cache_key`、次に `session_id` ヘッダーで識別します。Codex はどちらも自動で付与します。
- 詳しい手順は[設定ガイド](https://ccrouter.app/docs/codex-integration/)を参照。

</details>

<details>
<summary><b>OpenAI Chat Completions</b> <code>/v1/chat/completions</code> —— Open WebUI、Cherry Studio、Cline、LobeChat など</summary>

Open WebUI、Cherry Studio、Cline、LobeChat など OpenAI Chat Completions しか対応していないツールは、「OpenAI 互換」エンドポイントを cc-router に向けるだけで使えます：

| 設定項目 | 値 |
|---|---|
| Base URL | `http://127.0.0.1:23456/v1`（ツールによっては `/v1` なしを要求するので、ツールの案内に従ってください） |
| API Key | cc-router 設定画面の token（認証を無効にしている場合は空でない任意の値） |
| モデル名 | `model-fable` / `model-opus` / `model-sonnet` / `model-haiku`、または `gpt-6-astra` / `gpt-6-sol` / `gpt-6-terra` / `gpt-6-luna` などのティア別名。`GET /v1/models` で一覧を取得できます |

動作について：

- リクエストは cc-router 内部で Anthropic Messages に変換され、同じディスパッチを通ります。サブスクリプション・仮想モデル・クォータ・セッション親和はすべて有効で、リクエストログの「受信エンドポイント」には `/v1/chat/completions` と表示されます。
- 上流の thinking は `reasoning_content` フィールドで返されます（DeepSeek の慣例で、主要クライアントは折りたたみ表示に対応）。会話履歴でクライアントが送り返す `reasoning_content` は破棄されますが、以降の会話には影響しません。
- 画像は `data:` base64 と `http(s)` の両形式の `image_url` に対応。ツール呼び出しは双方向対応。ストリーミング応答の末尾には必ず `usage` フレームが付きます。
- 旧式の `functions` / `function_call` フィールドは非対応で、送ると 400 を返します。`tools` / `tool_choice` を使ってください。
- `n>1`、`logprobs`、`response_format` による JSON Schema 強制は無視されます（エラーにはなりません）。
- セッション親和（sticky）は `user` フィールドを優先し、次に `x-session-id` ヘッダー、どちらもなければ最初のユーザーメッセージの内容で識別します。
- 応答にツール呼び出しが含まれる場合、`finish_reason` は常に `tool_calls` になります。クライアントはこれを頼りにツール実行の要否を判断できます。

</details>

<details>
<summary><b>Jev System One</b> <code>/v1/systemone</code> —— 自作のコード、エージェントフレームワーク、または HTTP リクエストを送れる任意のプログラム</summary>

Jev（System One）は構造化された意思決定モデルです。リクエストで状態 `state` と質問の集合 `questions` を渡すと、レスポンスの `answers` に質問ごとの構造化された回答が返り、会話テキストは生成しません。cc-router は複数の上流のキーを独立した仮想モデル `model-jev` にまとめ、プロトコルを変換せずにそのまま透過します。

| 設定項目 | 値 |
|---|---|
| エンドポイント URL | `http://127.0.0.1:23456/v1/systemone`（フルパスで指定） |
| 認証 | `Authorization: Bearer <token>`。`x-api-key: <token>` でも構いません |
| モデル名 | `model-jev`（`anthropic/` / `openai/` プレフィックス可）、または上流の実際のモデル名 |

コマンドラインで確認：

```bash
curl http://127.0.0.1:23456/v1/systemone \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer <token>" \
  -d '{
  "model": "model-jev",
  "state": "Hello World",
  "questions": {
    "says_hello": {
      "type": "noul",
      "instructions": "Does the state text contain a greeting?"
    }
  }
}'
```

- `model-jev` を指定すると、cc-router は選ばれたサブスクリプションに設定された Jev モデルに書き換えます。未設定ならそのエンドポイントの例示モデル（`clef-flash`、`jev-latest` など）を使い、レスポンスの `model` は `model-jev` のまま返ります。実際のモデル名を指定した場合は、Jev モデルが設定されていれば書き換え、未設定ならそのまま透過します。
- `model-jev` にバインドされ、エンドポイントが System One のサブスクリプションだけを使います。会話系サブスクリプションはここでは使われず、System One サブスクリプションが他の仮想モデルに入ることもありません。会話用の 3 つの入口は `model-jev` を受け取ると 400 を返します。
- 非ストリーミングのみ対応。ある上流が 4xx を返すと次の上流に切り替えます（上流ごとに検証ルールが少しずつ異なるため）。すべて失敗した場合、リクエスト自体の誤りなら最後の上流のエラーをそのまま返し、それ以外は 503 を返します。
- セッション親和性は `x-session-id` ヘッダーのみで識別し、ない場合はラウンドロビンで割り当てます。
- 使用量は「統計」と「利用レシート」に計上されます。

</details>

### 出口：cc-router からプロバイダへの接続

出口はプロトコルごとに 3 分類、加えて OAuth ログインを使うサブスクリプションアカウントと、`model-jev` 専用の System One エンドポイントがあります。内蔵プロバイダプリセットもカスタムエンドポイントも同じ経路を通り、違いはプリセットがアドレス・認証方式・モデル一覧をあらかじめ埋めてくれる点だけです。内蔵プロバイダの完全な一覧はアプリ内「サブスクリプションを追加」画面が正となり、記述ファイルは [`src-tauri/providers/`](src-tauri/providers/) にあります。PR 歓迎です。

<details>
<summary><b>Anthropic Messages 互換</b> —— メイン経路、リクエストをそのまま透過</summary>

- 内蔵: Anthropic 公式、DeepSeek、智譜 GLM、Moonshot Kimi、MiniMax、Xiaomi MiMo、Alibaba Cloud Bailian、Volcengine Ark、Tencent Cloud、百度千帆、Stepfun、ModelScope、UCloud、Fireworks、OpenRouter、Requesty、xAI Grok、Aiberm、Ollama など。各社の Token Plan / Coding Plan / Agent Plan サブスクリプションと従量課金 API をカバー
- カスタム: Anthropic Messages 互換の任意のエンドポイント（中継、自前ゲートウェイなど）。Base URL と Key を入力するだけ
- プロトコル変換なし。thinking、`output_config.effort`、`cache_control`、画像、ツール呼び出しはすべて Anthropic ネイティブの意味論で動作します。**プロバイダがネイティブの Anthropic エンドポイントを提供しているなら、この経路を優先してください。** 変換経路では多かれ少なかれ情報が失われます

</details>

<details>
<summary><b>OpenAI 互換</b> <code>/v1/responses</code> · <code>/v1/chat/completions</code> —— プロトコル変換</summary>

- 内蔵: OpenAI 公式 API（GPT-5 / o3 / 4.1 などの reasoning モデル）
- カスタム: OpenAI Responses または Chat Completions 互換の任意のエンドポイント。one-api / new-api 中継、Groq、Together、ローカルの vLLM / llama.cpp など
- cc-router は Anthropic Messages を対応プロトコルに変換してから送信します: Anthropic thinking ↔ OpenAI reasoning を双方向にマッピングし、マルチターンの推論コンテキストを自動で送り返します。Chat Completions が返す `reasoning_content`（DeepSeek R1 など）は thinking ブロックとして Claude Code に渡されます
- 変換層で表現できない内容（`cache_control` など）は破棄されます。ネイティブの Anthropic エンドポイントを持つプロバイダは上の分類に登録し、ここには登録しないでください

</details>

<details>
<summary><b>Gemini 互換</b> <code>generateContent</code> · <code>/v1beta/interactions</code> —— プロトコル変換</summary>

- 内蔵: Google AI Studio（generateContent、従量課金 + 無料枠）、Google Gemini Interactions API（新しい統合エンドポイント）
- カスタム: Gemini generateContent 互換の任意のエンドポイント（messages_path に `{model}` プレースホルダを使用）、または Interactions 互換エンドポイント（model はリクエスト body 内にあるため、プレースホルダ不要）
- thinking は双方向マッピング。ツール呼び出しの往復時に thought signature を自動で引き継ぎます

</details>

<details>
<summary><b>サブスクリプションアカウント出口（OAuth）</b> —— Codex（ChatGPT Plus/Pro）、Kiro（AWS）</summary>

- API Key 不要。OAuth のデバイスコードでログインし、ChatGPT サブスクリプション / Kiro の無料 Claude 枠を出口として使います
- **グレーゾーンでアカウント停止のリスクがあるため、メインとしての利用は推奨しません。** フォールバックやサブアカウントとしての利用に留めてください。これに起因するレート制限、BAN、サブスクリプション解約について作者は一切責任を負いません

</details>

<details>
<summary><b>Jev System One</b> <code>/v1/systemone</code> —— <code>model-jev</code> 専用、そのまま透過</summary>

- 内蔵：TypeSafe、OpenRouter（System One エンドポイント）、Ollama 0.35+（ローカルの System One エンドポイント）、Cloudflare Clef。「サブスクリプションを追加」画面で各プロバイダの System One エンドポイントを選ぶだけです
- このサブスクリプションには任意の Jev モデルスロットが 1 つだけあり、空欄の場合は入口のセクションで説明した書き換え / 透過のルールに従います
- サブスクリプションのプロトコルは作成後に固定され、会話用エンドポイントと System One エンドポイントの間で切り替えることはできません。変更したい場合は新しく作成してください

</details>

### Web UI（オプション、デフォルト無効）

設定 → Web UI → 有効化後、ブラウザで `http://127.0.0.1:23456/ui/` を開くとデスクトップと完全に同じ管理画面が使えます。LAN 内の他のデバイスからアクセスするにはプロキシのリッスンアドレスを `0.0.0.0` に切り替えてください。設定ページに利用可能な URL が表示されます。ログインにはプロキシのアクセストークンを使います。「ログイン認証」スイッチは無効にもできます（無効にすると同一ネットワーク上のデバイスがログインなしで設定を変更できるため、注意して使ってください）。

### ターミナル UI（オプション、デフォルト無効）

`cc-router-tui` を使うと、起動中の cc-router をターミナルから管理できます。設定 → セキュリティとアクセス → ターミナル UI で「ターミナル UI を有効化」をオンにすると使えます（ローカル接続のみ受け付け、デスクトップ app が起動している必要があります）。同じカードから完全な起動パスをコピーできます。「PATH に追加」をクリックすると、以降はどのターミナルからも `cc-router-tui` と入力するだけで起動できます。macOS は `/usr/local/bin` にシンボリックリンクを作成（システムの認証ダイアログが表示されることがあります）、Windows はインストール先をユーザーの PATH に追加（開いているターミナルは開き直してください）、AppImage は `~/.local/bin` にコピー（このコピーは app の更新に追従しないため、更新後にもう一度クリックしてください）、deb はシステムに同梱されているので最初から PATH にあります。

- **できること** —— 概要、サブスクリプション（有効化・無効化 / 接続テスト / モデル一覧と残高の更新 / スロット編集 / 新規作成 / 削除）、仮想モデル、リアルタイムルーティング、リクエストログ。`?` ですべてのキー操作を表示します。表示言語はデスクトップ app の言語設定に従います（変更後はターミナル UI を開き直すと反映されます）
- **既知の制限** —— ターミナルは 80×24 以上が必要です。一部の記号は「曖昧幅」文字で、東アジア言語の環境では 2 列幅で表示されてレイアウトがずれることがあります。その場合はターミナルの設定で「曖昧幅の文字を半角（1 列幅）で表示する」類のオプションをオンにしてください。OAuth 系サブスクリプションの新規作成と API Key の変更は、引き続きデスクトップ app で行います

<table align="center">
  <tr>
    <td width="33%"><img src="assets/screenshots/ja/tui-overview.png" alt="cc-router ターミナル UI 概要画面" /></td>
    <td width="33%"><img src="assets/screenshots/ja/tui-subscriptions.png" alt="cc-router ターミナル UI サブスクリプション画面" /></td>
    <td width="33%"><img src="assets/screenshots/ja/tui-log-detail.png" alt="cc-router ターミナル UI リクエスト詳細画面" /></td>
  </tr>
</table>

## FAQ・ユースケース

<details>
<summary>cc-router は何を解決する？</summary>

**cc-router なし**: AI エージェント（Claude Code / OpenCode 等）は一度にひとつのベンダーしか使えず、小枠サブスクは肝心な場面で枯渇。設定ファイルを手で切り替える羽目になり、体験が悪い。

**cc-router あり**: エージェント → cc-router → ベンダー A + B + C。自動ロードバランス・自動フェイルオーバーで、3 つのサブスクをまるで 1 つのように使える。

得られるもの:

- **コスト削減** —— 高額な上位 Coding Plan を買わなくても、安い小枠 2 つで仕事が回る
- **中断ゼロ** —— レート制限や失敗で自動切替、エージェント側からは透過的
- **トップモデルを混ぜる** —— GLM-5.1 / DeepSeek-V4-Pro / MiniMax-2.7 / MiMo-V2.5-Pro を同時に活用、Claude Opus や GPT-5.5 のような純正 API も投入可能
- **使用量を一画面で** —— 全サブスクの token 消費を一目で確認、レシートとしてエクスポート可能

</details>

<details>
<summary><code>model-opus</code> / <code>model-sonnet</code> / <code>model-haiku</code> という 3 つの仮想モデルは何のため？</summary>

Claude Code はタスク難易度ごとに 3 段階のモデルを使い分けます: opus はプランニング、sonnet はコーディング、haiku はツール呼び出し。

cc-router はこの 3 段階を `model-opus` / `model-sonnet` / `model-haiku` という仮想スロットに抽象化。各スロットには実モデルのリストとスケジューリングモードを割り当てます:

- `model-opus` → DeepSeek-V4-Pro + GLM-5.1（ラウンドロビン）
- `model-sonnet` → MiniMax-M2.7 + MiMo-V2.5-Pro（ラウンドロビン）
- `model-haiku` → GLM-4-Flash

CC からのリクエストはこのマッピングに従って転送されるので、`~/.claude/settings.json` を頻繁に書き換える必要はありません。

</details>

<details>
<summary>複数の Coding Plan をどう組み合わせる？</summary>

例: サブスク A = GLM-5 / MiniMax-2.7 / DeepSeek-Flash、サブスク B = DeepSeek-V4-Pro / MiniMax-2.7 / GLM-5。

- **手堅い派** —— 両サブスクの同等性能のモデルを同じスロットにまとめてバインド。挙動が一貫し、フェイルオーバーも安定
- **攻めの派** —— 両サブスクのフラッグシップを `model-opus` のラウンドロビンに投入。クロス活用で `1 + 1 ≥ 2` になりやすい

</details>

<details>
<summary>スケジューリング: 順次・ラウンドロビン・セッション親和、どれを選ぶ？</summary>

- **順次** —— アカウント A を使い切ってから B に切り替え。キャッシュヒット率が高く、**小枠 GLM Coding Plan 2 つを使い切るシナリオに最適**
- **ラウンドロビン** —— 両アカウントが均等に負荷を分担。ただしアカウント跨ぎのキャッシュは独立しているので、若干余分に枠を消費する代わりに真のロードバランスが得られる
- **セッション親和** —— 同じ Claude Code セッションは同じサブスクに固定し、別セッションはラウンドロビンで振り分け。ラウンドロビン並みに均等でありながら prompt cache を失わず、並列サブエージェントもキャッシュを共有できる。固定先がレート制限・失敗したら自動で別のサブスクへ切り替え。**複数サブスクが健全で、キャッシュヒットを重視するなら推奨。**

</details>

## 開発

- Tauri 2
- Tailwind 4
- React 19

前提条件: Node.js ≥ 20（pnpm 推奨）、Rust ≥ 1.90（rustup の最新 stable 推奨）、Xcode Command Line Tools（macOS）。

```bash
pnpm install
pnpm tauri dev      # フロントエンド + Rust バックエンド + プロキシを単一プロセスで起動
```

初回起動時は onboarding フローが表示されます:

1. サブスクリプションを追加（プロバイダ選択 → エンドポイント選択 → API Key 入力 → モデル一覧を自動取得）
2. ワンクリックで 4 つの仮想モデルすべてに紐付け
3. 生成された env スニペットを `~/.claude/settings.json` に貼り付け

## ビルド

```bash
pnpm tauri build
```

成果物は `src-tauri/target/release/bundle/` 配下のプラットフォーム別サブフォルダに出力されます。

## Windows 開発環境の注意点

<details>
<summary>Windows での開発・ビルドで問題が起きたら展開（数時間の節約になります）</summary>

**1. MSVC toolchain が必須（GNU は不可）**

Tauri の Windows バンドルは MSVC に依存しており、CI も `x86_64-pc-windows-msvc` でビルドしています。確認方法:

```powershell
rustup show          # active toolchain の末尾が -msvc であること
rustc --version      # 1.90 以上であること
```

**2. Visual Studio Build Tools が必要**

MSVC toolchain は C++ コンパイラと Windows SDK を必要とし、無い場合は `link.exe not found` になります。[Build Tools for Visual Studio](https://visualstudio.microsoft.com/downloads/) をインストールする際に「**C++ によるデスクトップ開発**」ワークロードを選択し、完了後にターミナルを開き直してください。

**3. `rustc --version` と `rustup show` が食い違う場合は PATH を確認**

msi / scoop / Visual Studio コンポーネント経由でスタンドアロンの Rust を導入したことがある場合、それが rustup の shim より前に PATH へ並び、`rustup default stable` が無効化されることがあります:

```powershell
Get-Command rustc, cargo -All | Select-Object Source
```

`Source` が `%USERPROFILE%\.cargo\bin` を指しているはずです。そうでなければ該当ディレクトリを PATH の先頭へ移動する（`rundll32 sysdm.cpl,EditEnvironmentVariables`）か、スタンドアロン版をアンインストールしてください。

**4. dev server のプロセス残留**

Windows では Ctrl+C で `tauri dev` のプロセスツリーを終了しきれないことが多く、次回起動時に `Port 1420 is already in use` となります（`devUrl` が 1420 固定のため、`strictPort: true` は意図的に別ポートへフォールバックしません）:

```powershell
Get-NetTCPConnection -LocalPort 1420 -State Listen |
  Select-Object -ExpandProperty OwningProcess -Unique |
  ForEach-Object { Stop-Process -Id $_ -Force }
```

</details>

## 新しいプロバイダの追加

**Claude Code** を使用している場合、本リポジトリには `new-provider` という `SKILL` が同梱されています。対象プロバイダの公式ドキュメント URL またはエンドポイント情報を渡して実行すると、YAML のスキャフォールディングと関連箇所の修正を自動で行います。

## ライセンス

本プロジェクトは [MIT](LICENSE) ライセンスで公開しています。

フォント: レシートテーマで使用する 9 書体はすべて SIL Open Font License 1.1 です。帰属表示とライセンス全文は [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) を参照してください。

アイコン: プロバイダのブランドロゴは [@lobehub/icons](https://github.com/lobehub/lobe-icons)（MIT）を使用しています。各商標は各権利者に帰属します。
