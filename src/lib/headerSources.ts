// 请求详情弹窗里请求头表格的来源 / 回传标签。规则与后端常量手工同步:
// - 出站头优先级: src-tauri/src/proxy/forward.rs (透传 < 订阅配置 < 鉴权 < content-type)
// - 回传白名单: src-tauri/src/proxy/response_headers.rs (COPY_EXACT + COPY_PREFIXES)

export type OutboundSource = "forwarded" | "auth" | "configured" | "ccRouter";

/**
 * 发往上游的一条头是怎么来的。content-type 由 cc-router 固定写入; 与订阅鉴权头同名的是鉴权;
 * 客户端发过同名头的算透传 (同名被订阅配置覆盖时仍显示透传, 值以表里为准); 其余是订阅配置的头。
 */
export function outboundSource(
  name: string,
  clientNames: Set<string>,
  authHeaderName: string | undefined,
): OutboundSource {
  const n = name.toLowerCase();
  if (n === "content-type" || n === "accept") return "ccRouter";
  const auth = (authHeaderName || "authorization").toLowerCase();
  if (n === auth) return "auth";
  if (clientNames.has(n)) return "forwarded";
  return "configured";
}

const RETURNED_EXACT = new Set(["content-type", "cache-control", "retry-after", "x-should-retry", "request-id"]);
const RETURNED_PREFIXES = ["anthropic-ratelimit-"];

/** 上游响应头是否回传给客户端 (只对 Anthropic 透传订阅有意义)。 */
export function isReturnedToClient(name: string): boolean {
  const n = name.toLowerCase();
  return RETURNED_EXACT.has(n) || RETURNED_PREFIXES.some((p) => n.startsWith(p));
}
