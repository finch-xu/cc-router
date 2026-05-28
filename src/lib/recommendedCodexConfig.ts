/**
 * Codex CLI / Desktop 推荐配置生成器.
 *
 * - config.toml: 注入一个 `[model_providers.cc-router]` (wire_api = "responses") +
 *   一个 `[profiles.cc-router]`. 用户用 `codex -p cc-router` 走 cc-router.
 * - auth.json: 写入 `{ "OPENAI_API_KEY": "<cc-router token>" }`. cc-router 关鉴权时
 *   该 token 可被任意值替换, 这里写入它的好处是 token 轮换时一并同步.
 *
 * 字段必须与 src-tauri/src/integrations/codex.rs::ConfigSnapshot 的判定 (provider_name="cc-router",
 * wire_api="responses", base_url=cc-router/v1) 严格对齐 — 写出来的文件要能立刻被 inspect 判 in_sync.
 */

export interface CodexSnapshot {
  /** cc-router 本地代理 URL, 含 scheme + port (由后端 ProxyStatus.base_url 提供, 不要硬拼). */
  baseUrl: string;
  /** cc-router auth_token. */
  token: string;
}

/**
 * 生成完整的 config.toml 推荐内容 (含注释).
 * 注意 base_url 后缀必须是 `/v1`, Codex 会在此基础上拼 `/responses` 走 OpenAI Responses 协议.
 */
export function buildRecommendedCodexConfig(snap: CodexSnapshot): string {
  return `# cc-router 推荐配置 — 由 cc-router 接入指南生成
# 使用: codex -p cc-router "你的提问"
# 修改后保存即可生效, codex 下次启动会读取此文件.

[model_providers.cc-router]
name = "cc-router"
base_url = "${snap.baseUrl}/v1"
wire_api = "responses"
env_key = "OPENAI_API_KEY"

[profiles.cc-router]
model_provider = "cc-router"
model = "model-sonnet"
`;
}

/**
 * 生成完整的 auth.json 推荐内容.
 * 顶层只放 `OPENAI_API_KEY` 一个字段, 不动用户原 ChatGPT OAuth 结构 — 但写入会覆盖, 所以
 * Rust 端 write_auth_in 在旧文件含 tokens.access_token 时会先备份 .cc-router.bak.
 */
export function buildRecommendedCodexAuth(snap: CodexSnapshot): string {
  return JSON.stringify({ OPENAI_API_KEY: snap.token }, null, 2) + "\n";
}
