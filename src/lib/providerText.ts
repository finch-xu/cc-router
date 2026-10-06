import type { Locale } from "@/i18n";
import type { ProviderInfo, SubscriptionDto } from "@/types";

/**
 * 把 provider yaml 的上屏文字换成界面语言。后端顶层字段是中文、英日文在 translations 里,
 * 且三语齐全 (yaml 解析时强制), 所以这里不做回退。
 */
export function localizeProvider(p: ProviderInfo, locale: Locale): ProviderInfo {
  if (locale === "zh") return p;
  const text = p.translations[locale];
  return {
    ...p,
    display_name: text.display_name,
    description: text.description,
    compatibility_notes: text.compatibility_notes,
    endpoints: p.endpoints.map((e) => {
      const t = text.endpoints[e.id];
      return t ? { ...e, label: t.label, description: t.description } : e;
    }),
    url_params: p.url_params.map((u) => {
      const tx = text.url_params?.[u.id];
      return tx ? { ...u, label: tx.label, placeholder: tx.placeholder } : u;
    }),
  };
}

/** 订阅的厂商名: 内置厂商按界面语言取, 自定义订阅 / yaml 已删的厂商用创建时的快照。 */
export function providerName(
  sub: Pick<SubscriptionDto, "provider_names" | "provider_display_name">,
  locale: Locale,
): string {
  return sub.provider_names?.[locale] ?? sub.provider_display_name;
}
