import type { ReceiptDto, ReceiptSubItemDto, ReceiptTotalsDto } from "@/types";

export const ZERO_TOTALS: ReceiptTotalsDto = {
  request_count: 0,
  input_tokens: 0,
  output_tokens: 0,
  cache_creation_tokens: 0,
  cache_read_tokens: 0,
};

export function addTotals(a: ReceiptTotalsDto, b: ReceiptTotalsDto): ReceiptTotalsDto {
  return {
    request_count: a.request_count + b.request_count,
    input_tokens: a.input_tokens + b.input_tokens,
    output_tokens: a.output_tokens + b.output_tokens,
    cache_creation_tokens: a.cache_creation_tokens + b.cache_creation_tokens,
    cache_read_tokens: a.cache_read_tokens + b.cache_read_tokens,
  };
}

export function totalTokens(t: ReceiptTotalsDto): number {
  return (
    t.input_tokens + t.output_tokens + t.cache_creation_tokens + t.cache_read_tokens
  );
}

export interface SubscriptionModelLine {
  real_model_name: string;
  totals: ReceiptTotalsDto;
}

export interface SubscriptionGroup {
  subscription_id: string;
  /** undefined = 已删除订阅, 由渲染层兜底 i18n 文案 */
  subscription_display_name?: string;
  provider_id: string;
  provider_display_name: string;
  /** 按 token 总量降序 */
  models: SubscriptionModelLine[];
  subtotal: ReceiptTotalsDto;
}

export interface TotalsRow {
  key: string;
  /** undefined = 已删除订阅, 由渲染层兜底 i18n 文案 (仅 byCharacter=subscription 时可能为空) */
  display?: string;
  provider_id?: string;
  provider_display_name?: string;
  totals: ReceiptTotalsDto;
}

function flatten(dto: ReceiptDto): ReceiptSubItemDto[] {
  return dto.items.flatMap((it) => it.sub_items);
}

/**
 * L1=订阅, L2=该订阅下的真实模型 (合并跨虚拟模型出现的同模型条目)。
 * 按订阅 token 总量降序; 每组内模型也按 token 总量降序。
 */
export function groupBySubscription(dto: ReceiptDto): SubscriptionGroup[] {
  interface Bucket {
    subscription_id: string;
    subscription_display_name?: string;
    provider_id: string;
    provider_display_name: string;
    modelMap: Map<string, ReceiptTotalsDto>;
    subtotal: ReceiptTotalsDto;
  }
  const buckets = new Map<string, Bucket>();

  for (const sub of flatten(dto)) {
    let b = buckets.get(sub.subscription_id);
    if (!b) {
      b = {
        subscription_id: sub.subscription_id,
        subscription_display_name: sub.subscription_display_name,
        provider_id: sub.provider_id,
        provider_display_name: sub.provider_display_name,
        modelMap: new Map(),
        subtotal: { ...ZERO_TOTALS },
      };
      buckets.set(sub.subscription_id, b);
    }
    const prev = b.modelMap.get(sub.real_model_name) ?? { ...ZERO_TOTALS };
    b.modelMap.set(sub.real_model_name, addTotals(prev, sub.totals));
    b.subtotal = addTotals(b.subtotal, sub.totals);
  }

  const out: SubscriptionGroup[] = Array.from(buckets.values()).map((b) => {
    const models: SubscriptionModelLine[] = Array.from(b.modelMap, ([name, totals]) => ({
      real_model_name: name,
      totals,
    }));
    models.sort((a, c) => totalTokens(c.totals) - totalTokens(a.totals));
    return {
      subscription_id: b.subscription_id,
      subscription_display_name: b.subscription_display_name,
      provider_id: b.provider_id,
      provider_display_name: b.provider_display_name,
      models,
      subtotal: b.subtotal,
    };
  });
  out.sort((a, b) => totalTokens(b.subtotal) - totalTokens(a.subtotal));
  return out;
}

export function totalsBySubscription(dto: ReceiptDto): TotalsRow[] {
  const buckets = new Map<string, TotalsRow>();
  for (const sub of flatten(dto)) {
    let b = buckets.get(sub.subscription_id);
    if (!b) {
      b = {
        key: sub.subscription_id,
        display: sub.subscription_display_name,
        provider_id: sub.provider_id,
        provider_display_name: sub.provider_display_name,
        totals: { ...ZERO_TOTALS },
      };
      buckets.set(sub.subscription_id, b);
    }
    b.totals = addTotals(b.totals, sub.totals);
  }
  return Array.from(buckets.values()).sort(
    (a, b) => totalTokens(b.totals) - totalTokens(a.totals),
  );
}

export function totalsByRealModel(dto: ReceiptDto): TotalsRow[] {
  const buckets = new Map<string, TotalsRow>();
  for (const sub of flatten(dto)) {
    let b = buckets.get(sub.real_model_name);
    if (!b) {
      b = {
        key: sub.real_model_name,
        display: sub.real_model_name,
        totals: { ...ZERO_TOTALS },
      };
      buckets.set(sub.real_model_name, b);
    }
    b.totals = addTotals(b.totals, sub.totals);
  }
  return Array.from(buckets.values()).sort(
    (a, b) => totalTokens(b.totals) - totalTokens(a.totals),
  );
}
