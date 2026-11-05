// 请求详情弹窗里上游响应头的「回传」标签。出站头的来源由后端记录, 不在这里推断。
// 回传白名单与 src-tauri/src/proxy/response_headers.rs (COPY_EXACT + COPY_PREFIXES) 手工同步。

const RETURNED_EXACT = new Set(["content-type", "cache-control", "retry-after", "x-should-retry", "request-id"]);
const RETURNED_PREFIXES = ["anthropic-ratelimit-"];

/** 上游响应头是否回传给客户端 (只对 Anthropic 透传订阅有意义)。 */
export function isReturnedToClient(name: string): boolean {
  const n = name.toLowerCase();
  return RETURNED_EXACT.has(n) || RETURNED_PREFIXES.some((p) => n.startsWith(p));
}
