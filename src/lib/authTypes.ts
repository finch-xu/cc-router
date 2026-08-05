import type { AuthType } from "@/types";

/**
 * Anthropic 透传类判断: `api_key` 是 7 种 AuthType 里唯一不走协议翻译的。
 * fallback 虚拟模型的「未配置兜底槽 = 透传原始 model」语义只对透传类成立;
 * 翻译类订阅必须配置兜底槽才能参与 fallback (否则 dispatch 层直接跳过该候选)。
 */
export function isAnthropicPassthrough(authType: AuthType): boolean {
  return authType === "api_key";
}
