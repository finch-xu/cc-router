/**
 * Codex CLI / Desktop 推荐配置生成器.
 *
 * - config.toml: 注入一个 `[model_providers.cc-router]` (wire_api = "responses") +
 *   一个 `[profiles.cc-router]`. 用户用 `codex -p cc-router` 走 cc-router.
 * - auth.json: `{ "OPENAI_API_KEY": "<cc-router token>" }`. cc-router 关鉴权时该 token 可被任意值替换.
 *
 * 只用于接入指南页展示, 用户自己复制进 ~/.codex/ (cc-router 不写这两个文件).
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
 *
 * `comment` 是文件头注释 (按界面语言, 由调用方经 i18n 传入), 每行自动加 `# `.
 */
export function buildRecommendedCodexConfig(snap: CodexSnapshot, comment: string): string {
  const header = comment
    .split("\n")
    .map((line) => `# ${line}`)
    .join("\n");
  return `${header}

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
 * 顶层只放 `OPENAI_API_KEY` 一个字段; 整份替换会覆盖原 ChatGPT 登录, 页面上提示用户先备份.
 */
export function buildRecommendedCodexAuth(snap: CodexSnapshot): string {
  return JSON.stringify({ OPENAI_API_KEY: snap.token }, null, 2) + "\n";
}
