/**
 * 实时路由页的纯函数: 时间窗筛选、分桶、分位数、token 合计、客户端 / 用途归类。
 * 组件只负责画, 口径全在这里, 改统计规则只动这一个文件。
 */
import type { LucideIcon } from "lucide-react";
import { Bot, Cog, Globe, HelpCircle, MessageSquare, Radio, ShieldCheck, Shrink, Tag } from "lucide-react";
import { CLIENT_TOOLS_BY_ID } from "@/lib/clientTools";
import type { ClientToolId, LiveRequest, PurposeKind } from "@/types";

export const totalTokens = (r: LiveRequest) =>
  r.input_tokens + r.output_tokens + r.cache_read_tokens + r.cache_write_tokens;

export const isOk = (r: LiveRequest) => r.status === "success";

export function inWindow(records: readonly LiveRequest[], now: number, windowMs: number): LiveRequest[] {
  const from = now - windowMs;
  // records 按时间升序: 从尾部往前找起点
  let i = records.length;
  while (i > 0 && records[i - 1].timestamp >= from) i -= 1;
  return records.slice(i);
}

export function buckets(rows: readonly LiveRequest[], now: number, windowMs: number, n: number): LiveRequest[][] {
  const size = windowMs / n;
  const from = now - windowMs;
  const out: LiveRequest[][] = Array.from({ length: n }, () => []);
  for (const r of rows) {
    const i = Math.floor((r.timestamp - from) / size);
    if (i >= 0 && i < n) out[i].push(r);
  }
  return out;
}

export function median(values: number[]): number | null {
  if (!values.length) return null;
  const s = [...values].sort((a, b) => a - b);
  return s[Math.floor(s.length / 2)];
}

export interface TokenParts {
  cacheRead: number;
  cacheWrite: number;
  input: number;
  output: number;
}

export function sumTokens(rows: readonly LiveRequest[]): TokenParts {
  const t: TokenParts = { cacheRead: 0, cacheWrite: 0, input: 0, output: 0 };
  for (const r of rows) {
    t.cacheRead += r.cache_read_tokens;
    t.cacheWrite += r.cache_write_tokens;
    t.input += r.input_tokens;
    t.output += r.output_tokens;
  }
  return t;
}

export const partsTotal = (t: TokenParts) => t.cacheRead + t.cacheWrite + t.input + t.output;

/** 缓存命中率 = 缓存读 / 全部输入 */
export function cacheHitRatio(t: TokenParts): number | null {
  const all = t.cacheRead + t.cacheWrite + t.input;
  return all ? t.cacheRead / all : null;
}

/** 路由图左列的客户端节点 key: 已知客户端用自己的 id, 其余归「其他客户端」 */
export const OTHER_CLIENT = "other";
export function clientKey(tool: string | null | undefined): string {
  return tool && tool in CLIENT_TOOLS_BY_ID ? tool : OTHER_CLIENT;
}
export function clientMeta(key: string) {
  return key === OTHER_CLIENT ? null : CLIENT_TOOLS_BY_ID[key as ClientToolId];
}

/* ---------------- 请求用途 ---------------- */

export type PurposeTone = "main" | "agent" | "bg" | "unknown";

export const PURPOSE_META: Record<PurposeKind, { icon: LucideIcon; tone: PurposeTone }> = {
  main: { icon: MessageSquare, tone: "main" },
  agent: { icon: Bot, tone: "agent" },
  compact: { icon: Shrink, tone: "bg" },
  guard: { icon: ShieldCheck, tone: "bg" },
  title: { icon: Tag, tone: "bg" },
  fetch: { icon: Globe, tone: "bg" },
  probe: { icon: Radio, tone: "bg" },
  background: { icon: Cog, tone: "bg" },
  unknown: { icon: HelpCircle, tone: "unknown" },
};

export const PURPOSE_ORDER: PurposeKind[] = ["main", "agent", "compact", "guard", "title", "fetch", "probe", "background", "unknown"];

export const isBackground = (kind: PurposeKind) => PURPOSE_META[kind].tone === "bg";

/** 事件流顶部的用途筛选 */
export type FeedFilter = "all" | "main" | "agent" | "bg" | "fail";

export function matchFilter(r: LiveRequest, f: FeedFilter): boolean {
  switch (f) {
    case "all":
      return true;
    case "main":
      return r.purpose.kind === "main";
    case "agent":
      return r.purpose.kind === "agent";
    case "bg":
      return isBackground(r.purpose.kind);
    case "fail":
      return !isOk(r);
  }
}

/** 同一会话固定一个颜色 (按 id 哈希), 浅色 / 暗色下都够区分 */
const SESSION_COLORS = ["#d4553a", "#2f8f7a", "#6b5bd2", "#b7791f", "#2f6fb8", "#a8508a"];
export function sessionColor(id: string): string {
  let h = 0;
  for (let i = 0; i < id.length; i += 1) h = (h * 31 + id.charCodeAt(i)) | 0;
  return SESSION_COLORS[Math.abs(h) % SESSION_COLORS.length];
}

/** 工具名计数: ["Edit","Edit","Bash"] → "Edit ×2、Bash" 的原料 */
export function countNames(names: readonly string[]): [string, number][] {
  const m = new Map<string, number>();
  for (const n of names) m.set(n, (m.get(n) ?? 0) + 1);
  return [...m.entries()];
}

/* ---------------- 格式化 ---------------- */

export function fmtTok(n: number): string {
  if (n >= 1e9) return (n / 1e9).toFixed(2) + "B";
  if (n >= 1e6) return (n / 1e6).toFixed(n >= 1e7 ? 1 : 2) + "M";
  if (n >= 1e3) return (n / 1e3).toFixed(n >= 1e5 ? 0 : 1) + "k";
  return String(Math.round(n));
}

export function fmtMs(ms: number | null | undefined): string {
  if (ms == null) return "—";
  return ms >= 1000 ? (ms / 1000).toFixed(1) + " s" : Math.round(ms) + " ms";
}

export function fmtClock(ms: number): string {
  const d = new Date(ms);
  return [d.getHours(), d.getMinutes(), d.getSeconds()].map((x) => String(x).padStart(2, "0")).join(":");
}
