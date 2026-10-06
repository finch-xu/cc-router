import type { ProviderCapabilities } from "@/lib/providerCapabilities";

/** 下拉项右侧的能力标签。文字是协议名, 三语都不翻译。 */
export function ProviderCapabilityTags({ caps }: { caps: ProviderCapabilities }) {
  if (!caps.llm && !caps.jev) return null;
  return (
    <span className="cap-tags">
      {caps.llm && <span className="pill tag">LLM</span>}
      {caps.jev && <span className="pill tag cap-jev">Jev</span>}
    </span>
  );
}
