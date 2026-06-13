/**
 * Claude Code settings.json `env` 段的 cc-router 推荐字段集.
 * 单一来源: Guide.tsx 的两份 JSON snippet, ClaudeCodeSettingsEditor 的 merge 算法,
 * 以及未来任何其他展示/写入位置都从这里读, 避免改一处忘别处.
 *
 * Rust 端 commands/proxy.rs::env_snippet 用同一份字段(shell export 形态),
 * 改这里时记得同步更新 env_snippet — 见 CLAUDE.md 第 12 字段表.
 */

/** 6 个核心字段: 决定 cc-router 接入是否生效, 应用时强制覆盖用户原值. */
export const CLAUDE_CODE_CORE_KEYS = [
  "ANTHROPIC_BASE_URL",
  "ANTHROPIC_AUTH_TOKEN",
  "ANTHROPIC_DEFAULT_FABLE_MODEL",
  "ANTHROPIC_DEFAULT_OPUS_MODEL",
  "ANTHROPIC_DEFAULT_SONNET_MODEL",
  "ANTHROPIC_DEFAULT_HAIKU_MODEL",
] as const;

/** 7 个推荐字段: 优化体验但用户可能有自己偏好, 应用时仅在不存在时插入. */
export const CLAUDE_CODE_RECOMMENDED_KEYS = [
  "API_TIMEOUT_MS",
  "ANTHROPIC_MODEL",
  "CLAUDE_CODE_SUBAGENT_MODEL",
  "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
  "CLAUDE_CODE_DISABLE_NONSTREAMING_FALLBACK",
  "CLAUDE_CODE_ATTRIBUTION_HEADER",
  "CLAUDE_CODE_EFFORT_LEVEL",
] as const;

export interface ClaudeCodeEnvSnapshot {
  /** cc-router 本地代理 URL, 含 scheme + port (由后端 ProxyStatus.base_url 提供, 不要前端硬拼). */
  baseUrl: string;
  /** cc-router auth_token. */
  token: string;
}

/** 13 字段全集 (核心 6 + 推荐 7) 的当前推荐值. */
export function buildRecommendedEnv(
  snap: ClaudeCodeEnvSnapshot,
): Record<string, string> {
  return {
    ANTHROPIC_BASE_URL: snap.baseUrl,
    ANTHROPIC_AUTH_TOKEN: snap.token,
    ANTHROPIC_DEFAULT_FABLE_MODEL: "model-fable",
    ANTHROPIC_DEFAULT_OPUS_MODEL: "model-opus",
    ANTHROPIC_DEFAULT_SONNET_MODEL: "model-sonnet",
    ANTHROPIC_DEFAULT_HAIKU_MODEL: "model-haiku",
    API_TIMEOUT_MS: "3000000",
    ANTHROPIC_MODEL: "model-opus",
    CLAUDE_CODE_SUBAGENT_MODEL: "model-opus",
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
    CLAUDE_CODE_DISABLE_NONSTREAMING_FALLBACK: "1",
    CLAUDE_CODE_ATTRIBUTION_HEADER: "0",
    CLAUDE_CODE_EFFORT_LEVEL: "max",
  };
}

/**
 * 把推荐字段 merge 进当前 settings.json 文本.
 * - 5 核心字段 Upsert (强制替换)
 * - 7 推荐字段 InsertIfAbsent (尊重用户已有值)
 * 返回 null = 当前文本不是合法 JSON Object, 不能 merge.
 */
export function mergeRecommendedEnv(
  current: string,
  snap: ClaudeCodeEnvSnapshot,
): string | null {
  let root: Record<string, unknown> = {};
  const trimmed = current.trim();
  if (trimmed) {
    let parsed: unknown;
    try {
      parsed = JSON.parse(trimmed);
    } catch {
      return null;
    }
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return null;
    root = parsed as Record<string, unknown>;
  }

  const existingEnv =
    root.env && typeof root.env === "object" && !Array.isArray(root.env)
      ? { ...(root.env as Record<string, unknown>) }
      : {};
  const rec = buildRecommendedEnv(snap);

  for (const k of CLAUDE_CODE_CORE_KEYS) {
    existingEnv[k] = rec[k];
  }
  for (const k of CLAUDE_CODE_RECOMMENDED_KEYS) {
    if (!(k in existingEnv)) {
      existingEnv[k] = rec[k];
    }
  }

  return JSON.stringify({ ...root, env: existingEnv }, null, 2);
}
