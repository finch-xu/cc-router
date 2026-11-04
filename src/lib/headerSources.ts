// 请求详情弹窗里请求头表格的来源 / 回传标签。规则与后端常量手工同步:
// - 出站头优先级: src-tauri/src/proxy/forward.rs (透传 < 订阅配置 < 鉴权 < content-type)
// - 回传白名单: src-tauri/src/proxy/response_headers.rs (COPY_EXACT + COPY_PREFIXES)

export type OutboundSource = "forwarded" | "auth" | "configured" | "ccRouter";

/** 客户端请求头: 小写名 → 该名下全部值 (重复头按出现顺序)。 */
export type ClientHeaderValues = Map<string, string[]>;

export function clientHeaderValues(pairs: [string, string][] | null | undefined): ClientHeaderValues {
  const m: ClientHeaderValues = new Map();
  for (const [k, v] of pairs ?? []) {
    const n = k.toLowerCase();
    m.set(n, [...(m.get(n) ?? []), v]);
  }
  return m;
}

/**
 * 发往上游的一条头是怎么来的。content-type / accept / 默认 user-agent 由 cc-router 写入;
 * 与订阅鉴权头同名的是鉴权; **客户端发过同名且同值**才算透传 (同名不同值 = 被订阅配置或
 * 接入方式改写过, 如 Codex 的 UA、yaml 固定的 anthropic-version); 其余是订阅配置的头。
 */
export function outboundSource(
  name: string,
  value: string,
  client: ClientHeaderValues,
  authHeaderName: string | undefined,
): OutboundSource {
  const n = name.toLowerCase();
  if (n === "content-type" || n === "accept") return "ccRouter";
  if (n === "user-agent" && value.startsWith("cc-router/")) return "ccRouter";
  const auth = (authHeaderName || "authorization").toLowerCase();
  if (n === auth) return "auth";
  if (client.get(n)?.includes(value)) return "forwarded";
  return "configured";
}

const RETURNED_EXACT = new Set(["content-type", "cache-control", "retry-after", "x-should-retry", "request-id"]);
const RETURNED_PREFIXES = ["anthropic-ratelimit-"];

/** 上游响应头是否回传给客户端 (只对 Anthropic 透传订阅有意义)。 */
export function isReturnedToClient(name: string): boolean {
  const n = name.toLowerCase();
  return RETURNED_EXACT.has(n) || RETURNED_PREFIXES.some((p) => n.startsWith(p));
}
