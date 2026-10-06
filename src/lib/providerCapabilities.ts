import type { ProviderInfo } from "@/types";

/** 厂商能接哪类虚拟模型: 有对话端点 → LLM, 有 System One 端点 → Jev。
 *  与 TUI `Provider::capabilities` 是同一条规则, 改一边必须改另一边 (tui_contract.rs 锁住)。 */
export interface ProviderCapabilities {
  llm: boolean;
  jev: boolean;
}

export function providerCapabilities(p: Pick<ProviderInfo, "endpoints">): ProviderCapabilities {
  return {
    llm: p.endpoints.some((e) => e.protocol === "messages"),
    jev: p.endpoints.some((e) => e.protocol === "systemone"),
  };
}

/** 自定义入口 (5 种协议) 都是对话类。 */
export const CUSTOM_CAPABILITIES: ProviderCapabilities = { llm: true, jev: false };
