/**
 * 订阅列表的派生状态: 一个订阅「现在算什么状态」「要不要人处理」, 排序与筛选都从这里取,
 * 避免页面各处重复判断 (后端的 `is_dispatchable` 是调度器口径, 这里只是把它拆成人话)。
 */
import type { CustomVirtualModelDto, SubscriptionDto, VirtualModelDto } from "@/types";

export type StatusKind =
  /** 可调度 */
  | "ok"
  /** 冷却中 (会自己恢复) */
  | "cool"
  /** 其他不可调度但会自愈的状态 (临时错误等) */
  | "warn"
  /** 凭证失效 */
  | "auth"
  /** 本地 token 限额已满 */
  | "full"
  /** 还没填 API Key */
  | "key"
  /** 已停用 */
  | "off";

export const isOAuth = (s: SubscriptionDto) => s.auth_type === "chatgpt_oauth" || s.auth_type === "kiro_oauth";

export function statusOf(s: SubscriptionDto, now: number): StatusKind {
  if (!s.has_api_key && !isOAuth(s)) return "key";
  if (!s.enabled) return "off";
  if (s.state === "auth_failed") return "auth";
  if (s.quota_usage.some((q) => q.exceeded)) return "full";
  if (s.cooldown_until && s.cooldown_until > now) return "cool";
  if (s.state !== "healthy" || !s.is_dispatchable) return "warn";
  return "ok";
}

export type AttentionKind = "key" | "auth" | "full" | "balance";

/** 只列要人动手的; 冷却 / 临时错误会自己恢复, 不算 */
export function attentionOf(s: SubscriptionDto, kind: StatusKind): AttentionKind | null {
  if (kind === "key" || kind === "auth" || kind === "full") return kind;
  if (kind !== "off" && balanceSeverity(s) !== "normal" && balanceSeverity(s) != null) return "balance";
  return null;
}

export function balanceSeverity(s: SubscriptionDto): "normal" | "low" | "critical" | null {
  const snap = s.balance_supported ? s.balance_cache?.snapshot : undefined;
  const entry = snap?.entries[0];
  if (!snap || !entry) return null;
  return snap.is_available === false ? "critical" : entry.severity;
}

/** 默认排序: 需处理 → 可调度 → 冷却 → 停用 */
export const RANK: Record<StatusKind, number> = { key: 0, auth: 0, full: 0, ok: 1, cool: 2, warn: 2, off: 3 };

/** 汇总条上一格的颜色 */
export function cellTone(kind: StatusKind): "" | "warn" | "err" | "off" {
  if (kind === "ok") return "";
  if (kind === "auth") return "err";
  if (kind === "off" || kind === "key") return "off";
  return "warn";
}

export function quotaUsed(q: SubscriptionDto["quota_usage"][number]): number {
  return q.input + q.output + q.cache_creation + q.cache_read;
}

/** 用量最接近上限的那个周期 */
export function tightestQuota(s: SubscriptionDto) {
  let best: { q: SubscriptionDto["quota_usage"][number]; ratio: number } | null = null;
  for (const q of s.quota_usage) {
    if (!q.limit) continue;
    const ratio = quotaUsed(q) / q.limit;
    if (!best || ratio > best.ratio) best = { q, ratio };
  }
  return best;
}

export interface VmUse {
  /** 去掉 `model-` 前缀的短名 */
  name: string;
  full: string;
  /** 在该虚拟模型订阅列表里的顺位 (1 起); 拿不到时 null */
  pos: number | null;
}

/** 订阅 id → 它出现在哪些虚拟模型里、排第几 */
export function buildUses(
  subs: readonly SubscriptionDto[],
  vms: readonly VirtualModelDto[] | undefined,
  custom: readonly CustomVirtualModelDto[] | undefined,
): Map<string, VmUse[]> {
  const pos = new Map<string, number>();
  for (const vm of vms ?? []) vm.subscription_ids.forEach((id, i) => pos.set(`${vm.name}|${id}`, i + 1));
  for (const vm of custom ?? []) vm.targets.forEach((tg, i) => {
    const k = `${vm.name}|${tg.subscription_id}`;
    if (!pos.has(k)) pos.set(k, i + 1);
  });
  const out = new Map<string, VmUse[]>();
  for (const s of subs) {
    out.set(
      s.id,
      s.referenced_by.map((full) => ({ full, name: full.replace(/^model-/, ""), pos: pos.get(`${full}|${s.id}`) ?? null })),
    );
  }
  return out;
}

export function hostOf(url: string): string {
  try {
    return new URL(url).host || url;
  } catch {
    return url;
  }
}
