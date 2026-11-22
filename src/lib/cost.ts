/**
 * 费用显示 (计价插件, 设计 §5)。金额是 micros (该币种的百万分之一); 两种币种分开、
 * 真实花费与「等价按量费用」(包月订阅) 分开, 不换算不相加。零依赖 (小票字段表也用)。
 */
import type { CostLine, CostTotals } from "../types";

const SYMBOL: Record<string, string> = { USD: "$", CNY: "¥" };

/** ≥ 1 两位小数; 不到 1 最多四位 (去掉末尾 0, 至少两位); 0 显示为 0.00 */
export function formatMoney(micros: number, currency: string): string {
  const v = micros / 1_000_000;
  const sym = SYMBOL[currency] ?? `${currency} `;
  if (v === 0) return `${sym}0.00`;
  const abs = Math.abs(v);
  let text = abs >= 1 ? abs.toFixed(2) : abs.toFixed(4).replace(/0{1,2}$/, "");
  if (abs > 0 && Number(text) === 0) text = "<0.0001";
  return `${v < 0 ? "-" : ""}${sym}${text}`;
}

export function formatCostLine(c: CostLine): string {
  return `${c.equivalent ? "≈" : ""}${formatMoney(c.micros, c.currency)}`;
}

/** 真实花费的几段 (各币种), 不含等价费用 */
export function spendParts(t: CostTotals): string[] {
  const out: string[] = [];
  if (t.usd_micros) out.push(formatMoney(t.usd_micros, "USD"));
  if (t.cny_micros) out.push(formatMoney(t.cny_micros, "CNY"));
  return out;
}

/** 等价按量费用的几段 (包月订阅) */
export function equivalentParts(t: CostTotals): string[] {
  const out: string[] = [];
  if (t.equiv_usd_micros) out.push(formatMoney(t.equiv_usd_micros, "USD"));
  if (t.equiv_cny_micros) out.push(formatMoney(t.equiv_cny_micros, "CNY"));
  return out;
}

export const EMPTY_COST: CostTotals = { usd_micros: 0, cny_micros: 0, equiv_usd_micros: 0, equiv_cny_micros: 0 };

export function addCost(t: CostTotals, c: CostLine | undefined): CostTotals {
  if (!c) return t;
  const key =
    c.currency === "CNY" ? (c.equivalent ? "equiv_cny_micros" : "cny_micros") : c.equivalent ? "equiv_usd_micros" : "usd_micros";
  return { ...t, [key]: t[key] + c.micros };
}

export function sumCost(a: CostTotals, b: CostTotals): CostTotals {
  return {
    usd_micros: a.usd_micros + b.usd_micros,
    cny_micros: a.cny_micros + b.cny_micros,
    equiv_usd_micros: a.equiv_usd_micros + b.equiv_usd_micros,
    equiv_cny_micros: a.equiv_cny_micros + b.equiv_cny_micros,
  };
}

export const hasAnyCost = (t: CostTotals) =>
  t.usd_micros !== 0 || t.cny_micros !== 0 || t.equiv_usd_micros !== 0 || t.equiv_cny_micros !== 0;
