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
  <a href="README.md">中文</a> · <a href="README.en.md">English</a> · <strong>日本語</strong>
</p>

<p align="center">
  <a href="https://finch-xu.github.io/docs/zh/cc-router/getting-started/">📖 中文文档</a> · <a href="https://finch-xu.github.io/docs/cc-router/getting-started/">📖 English Docs</a> · <a href="https://deepwiki.com/finch-xu/cc-router">🤖 DeepWiki</a> · <a href="https://ccrouter.app">ccrouter.app</a>
</p>

複数の LLM ベンダーのサブスクリプションを契約しているのに、Claude Code は 1 社しか指せない——cc-router は DeepSeek・Qwen（通義千問）・Kimi・MiMo・MiniMax・GLM・Claude の Token Plan / Coding Plan / 従量課金 API を 1 つの仮想プランに統合します。opus / sonnet / haiku の 3 スロットに自由に割り当て、順次（sequential）またはラウンドロビン（round_robin）でディスパッチ。レート制限や障害時には自動でフォールバックするので、契約したクォータを余すことなく使い切れます。

> ⚠️ 注意: 本ツールは「すでに保有しているサブスクリプションプラン間の自動切り替え」のみを目的としています。リクエストボディはほぼそのまま透過するだけで、リバースエンジニアリングや脱獄、回避行為は一切含みません。各プランの利用規約は利用者ご自身で遵守してください。Claude Code などのコーディングツール用途専用であり、それ以外の用途には使用しないでください。
>
> 各プロバイダの利用規約が「サブスクリプションキーをサードパーティのプロキシ経由でルーティングし、複数仮想モデルでディスパッチする」用途を明示的に許可しているとは限りません。特に Coding Plan / Token Plan のような per-seat サブスクリプションでは、リスク管理機構に検知される可能性があります。本ツールの使用に起因するアカウントのレート制限、BAN、サブスクリプション解約等について、作者は一切の責任を負いません。
>
> 本ソフトウェアは As-Is（現状有姿）で提供され、明示・黙示を問わずいかなる保証もしません。クォータの異常消費、データ損失、業務中断を含む直接・間接の損害について作者は責任を負いません。

機能ハイライト：

- **18 プロバイダーを 1 つのルーターで** —— DeepSeek・Qwen・Kimi・MiMo・MiniMax・GLM・Claude などの Token Plan / Coding Plan / 従量課金 API を内蔵対応。opus / sonnet / haiku の 3 スロットに自由に割り当て、順次（sequential）またはラウンドロビン（round_robin）で自動切替
- **任意のエンドポイントを追加可能** —— 内蔵プロバイダーで足りない場合、Anthropic Messages 互換 API なら何でも直接接続でき、内蔵サブスクと同等にディスパッチ
- **利用レシート** —— トークン消費スナップショットを PNG / PDF / HTML へワンクリックでエクスポート。モノクロ / カラーの 2 モード、既定では料金非表示で利用量のみ、フッターの QR コードからリポジトリへジャンプ
- **3 言語完全翻訳** —— 简体中文 / English / 日本語、システム言語追従または設定画面で手動切替
- **仮想モデルのエイリアス対応** —— opus / sonnet / haiku の各スロットが複数の命名を識別。opus を例にすると `model-opus` / `claude-opus-4-7` / `anthropic/model-opus` / `anthropic/claude-opus-4-7` がすべて同じ仮想モデルにルーティングされ、ツール側の命名規約に左右されません
- **ローカル HTTPS** —— ワンクリックで自己署名 CA とサーバー証明書を生成し、HTTPS しか受け付けないクライアントからも cc-router を呼び出せます。詳細は[設定ガイド](https://ccrouter.app/docs/claude-desktop-integration/)を参照
- **Claude Desktop App 対応** —— ローカル HTTPS と仮想モデルエイリアスを組み合わせることで、Anthropic 公式デスクトップアプリから cc-router で集約した複数サブスクへ直接接続できます。詳細は[設定ガイド](https://ccrouter.app/docs/claude-desktop-integration/)を参照

<table align="center">
  <tr>
    <td width="60%"><img src="assets/screenshot-models.png" alt="cc-router 仮想モデル設定画面" /></td>
    <td width="40%" rowspan="2"><img src="assets/screenshot-receipts.png" alt="cc-router 利用レシート 縦長スクリーンショット" /></td>
  </tr>
  <tr>
    <td width="60%"><img src="assets/screenshot-logs.png" alt="cc-router リクエストログ画面" /></td>
  </tr>
</table>

## 対応プラン・API 一覧

| id | 名称 | Token Plan | API | 動作確認 |
|---|---|---|---|---|
| `anthropic` | Anthropic 公式 API（従量課金のみ、サブスクリプションプラン非対応） | ❌ | ✅ | verified |
| `zhipu` | 智譜 GLM（従量課金 / 中国サブスク） | ✅ | ✅ | verified |
| `deepseek` | DeepSeek（従量課金） | ❌ | ✅ | verified |
| `moonshot` | Moonshot Kimi（従量課金 / 中国サブスク / グローバルサブスク） | ✅ | ✅ | untested |
| `minimax` | MiniMax（従量課金 / 中国サブスク / グローバルサブスク） | ✅ | ✅ | verified |
| `xiaomi` | Xiaomi MiMo（従量課金 / 中国サブスク / グローバルサブスク） | ✅ | ✅ | untested |
| `alibaba` | Alibaba Cloud Bailian（チーム版 Token Plan + 2 リージョン従量課金 + 販売終了の Coding Plan） | ✅ | ✅ | verified |
| `volcengine` | バイトダンス 火山方舟 Volcengine Ark（Coding Plan サブスクリプション + Agent Plan サブスクリプション + 従量課金） | ✅ | ✅ | untested |
| `openrouter` | OpenRouter アグリゲーター（500+ モデルをルーティング） | ❌ | ✅ | untested |
| `tencent` | Tencent Cloud LLM（Token Plan サブスクリプション + TokenHub 従量課金、中国本土/海外） | ✅ | ✅ | untested |
| `aiberm` | Aiberm（従量課金 API、token group ごとに動的にモデル返却） | ❌ | ✅ | untested |
| `whatai` | 神馬中継 API（従量課金、OpenAI/Anthropic デュアルプロトコル中継、Anthropic 経路のみ使用） | ❌ | ✅ | untested |
| `ollama` | Ollama ローカル推論（localhost:11434 のみ、`glm-4.7:cloud` のようなクラウドタグも含む） | ❌ | ✅ | partial |
| `fireworks` | Fireworks AI（従量課金 / Fire Pass グローバルサブスク） | ✅ | ✅ | verified |
| `stepfun` | 階躍星辰 Stepfun（従量課金 / 中国サブスク / グローバルサブスク） | ✅ | ✅ | untested |
| `baidu` | 百度千帆（従量課金 / 中国サブスク） | ✅ | ✅ | untested |
| `modelscope` | ModelScope 魔搭（従量課金） | ❌ | ✅ | partial |
| `ucloud` | 優雲智算 UCloud Modelverse（Coding Plan サブスク + 従量課金 API、中国国内/海外） | ✅ | ✅ | untested |
| `openai_codex` | **OpenAI Codex（ChatGPT Plus/Pro サブスクリプション）** — アカウント停止リスクあり、推奨しません | ✅ | ❌ | untested |
| `kiro` | **Kiro IDE（AWS）** — Claude サブスクリプション無料枠、アカウント停止リスクあり、推奨しません | ✅ | ❌ | untested |
| `カスタム` | Anthropic プロトコル準拠の任意の API を自前で追加 | ✅ | ✅ | verified |

> 「Token Plan」列はサブスクリプション形式のクォータ全般（Token Plan / Coding Plan / Agent Plan 等）を指し、「API」列は従量課金の Anthropic Messages 互換エンドポイントを指します。

コミュニティからの PR 歓迎です。

## 技術スタック

- Tauri 2
- Tailwind 4
- React 19

## クイックスタート

1. Releases からインストーラをダウンロードして実行します。
2. LLM サブスクリプションを追加し、仮想モデルに紐付けてディスパッチモードを選択します。
3. 下記の env スニペットで Claude Code を cc-router に向けます。

## Claude Code での利用

**設定** ページが完全な env スニペットを動的に表示します。デフォルトポートが使用中の場合は、最大 100 回まで自動でインクリメントして空きを探します。

```json
{
  "env": {
    "ANTHROPIC_BASE_URL": "http://127.0.0.1:23456",
    "ANTHROPIC_AUTH_TOKEN": "your token, show in this app settings",
    "API_TIMEOUT_MS": "3000000",
    "ANTHROPIC_MODEL": "model-opus",
    "ANTHROPIC_DEFAULT_OPUS_MODEL": "model-opus",
    "ANTHROPIC_DEFAULT_SONNET_MODEL": "model-sonnet",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL": "model-haiku",
    "CLAUDE_CODE_SUBAGENT_MODEL": "model-opus",
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
    "CLAUDE_CODE_DISABLE_NONSTREAMING_FALLBACK": "1",
    "CLAUDE_CODE_EFFORT_LEVEL": "max"
  }
}
```

`OPUS_MODEL` が `1m` コンテキストに対応している場合、`model-opus[1m]` に設定すると Claude Code のロングコンテキストをフルに活用できます。

LiteLLM 形式の `anthropic/` プレフィックスにも対応しています: `anthropic/model-opus` / `anthropic/model-sonnet` / `anthropic/model-haiku` はプレフィックスなしの記法と等価で、Anthropic プロトコルを認識させるためにプロバイダプレフィックスが必要なツールとの連携が容易になります。

仮想モデルとエイリアス:

| 仮想モデル | エイリアス |
|---|---|
| `model-opus` | `anthropic/model-opus` `anthropic/claude-opus-4-7` `claude-opus-4-7` |
| `model-sonnet` | `anthropic/model-sonnet` `anthropic/claude-sonnet-4-6` `claude-sonnet-4-6` |
| `model-haiku` | `anthropic/model-haiku` `anthropic/claude-haiku-4-5` `claude-haiku-4-5` |

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
<summary>スケジューリング: 順次とラウンドロビン、どちらを選ぶ？</summary>

- **順次** —— アカウント A を使い切ってから B に切り替え。キャッシュヒット率が高く、**小枠 GLM Coding Plan 2 つを使い切るシナリオに最適**
- **ラウンドロビン** —— 両アカウントが均等に負荷を分担。ただしアカウント跨ぎのキャッシュは独立しているので、若干余分に枠を消費する代わりに真のロードバランスが得られる

</details>

## 開発

前提条件: Node.js ≥ 20（pnpm 推奨）、Rust ≥ 1.77、Xcode Command Line Tools（macOS）。

```bash
pnpm install
pnpm tauri dev      # フロントエンド + Rust バックエンド + プロキシを単一プロセスで起動
```

初回起動時は onboarding フローが表示されます:

1. サブスクリプションを追加（プロバイダ選択 → エンドポイント選択 → API Key 入力 → モデル一覧を自動取得）
2. ワンクリックで 3 つの仮想モデルすべてに紐付け
3. 生成された env スニペットを `~/.claude/settings.json` に貼り付け

## 新しいプロバイダの追加

**Claude Code** を使用している場合、本リポジトリには `new-provider` という `SKILL` が同梱されています。対象プロバイダの公式ドキュメント URL またはエンドポイント情報を渡して実行すると、YAML のスキャフォールディングと関連箇所の修正を自動で行います。

## ビルド

```bash
pnpm tauri build
```

成果物は `src-tauri/target/release/bundle/` 配下のプラットフォーム別サブフォルダに出力されます。

## アイコン

プロバイダのブランドロゴは [@lobehub/icons](https://github.com/lobehub/lobe-icons)（MIT）を使用しています。各商標は各権利者に帰属します。

## ライセンス

MIT
