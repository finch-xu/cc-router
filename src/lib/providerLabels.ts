/**
 * 自定义 provider 的 id → 友好展示信息映射 (范式同 virtualModels.ts 的 VM_META)。
 *
 * 自定义订阅的 provider_id 是 subscription/model.rs 的 CUSTOM_*_SOURCE_MARKER
 * (custom / custom-gemini / ...), 不在内置 providers yaml 列表里, 各页面
 * `providers.find(p => p.id === id)` 反查不到时会裸显 id —— 用这里的映射兜底。
 */

interface CustomProviderMeta {
  /** i18n key, 展示成「自定义 (OpenAI Responses)」等 */
  labelKey: string;
  /** ProviderIcon 的 iconId (custom / openai / google), 与创建订阅时写入的 provider_icon 一致 */
  iconId: string;
}

const CUSTOM_PROVIDER_META: Record<string, CustomProviderMeta> = {
  custom: { labelKey: "provider.custom.anthropic", iconId: "custom" },
  "custom-gemini": { labelKey: "provider.custom.gemini", iconId: "google" },
  "custom-gemini-interactions": {
    labelKey: "provider.custom.geminiInteractions",
    iconId: "google",
  },
  "custom-openai": { labelKey: "provider.custom.openaiResponses", iconId: "openai" },
  "custom-openai-chat": { labelKey: "provider.custom.openaiChat", iconId: "openai" },
};

/** migration 014 之前的旧格式别名, 兜底任何未迁移的残留展示场景 */
const LEGACY_ALIASES: Record<string, string> = {
  __custom__: "custom",
  __custom_gemini__: "custom-gemini",
  __custom_gemini_interactions__: "custom-gemini-interactions",
  __custom_openai__: "custom-openai",
  __custom_openai_chat__: "custom-openai-chat",
};

function metaOf(providerId: string): CustomProviderMeta | undefined {
  return CUSTOM_PROVIDER_META[LEGACY_ALIASES[providerId] ?? providerId];
}

/** 自定义 provider id → 友好名; 非自定义 id 返回 undefined (调用方走原有 display_name 逻辑) */
export function customProviderLabel(
  providerId: string,
  t: (key: string) => string,
): string | undefined {
  const meta = metaOf(providerId);
  return meta ? t(meta.labelKey) : undefined;
}

/** provider_id → ProviderIcon 可识别的 iconId; 非自定义 id 原样返回 */
export function providerIconId(providerId: string): string {
  return metaOf(providerId)?.iconId ?? providerId;
}
