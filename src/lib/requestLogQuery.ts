/**
 * 请求日志的筛选状态全部放在 URL search params 里: 列表 → 详情 → 返回时原样恢复,
 * 实时路由也能用链接直达 (`/request-logs?range=1h&session=…`)。详情页用同一份参数算「上一条 / 下一条」。
 */
import type { CustomVirtualModelName, RequestLogFilters, RequestPurposeFilter, RequestStatus } from "@/types";

export const RANGES = ["1h", "today", "7d", "30d", "all"] as const;
export type LogRange = (typeof RANGES)[number];
export const DEFAULT_RANGE: LogRange = "all";

export const PURPOSE_FILTERS = ["all", "main", "agent", "bg", "fail"] as const;
export type PurposeChip = (typeof PURPOSE_FILTERS)[number];

/** URL 参数名 */
const P = {
  range: "range",
  purpose: "purpose",
  session: "session",
  vm: "vm",
  provider: "provider",
  status: "status",
  client: "client",
  subscription: "subscription_id",
  q: "q",
  page: "page",
} as const;

export interface LogQuery {
  range: LogRange;
  purpose: PurposeChip;
  session: string | null;
  vm: CustomVirtualModelName | null;
  provider: string | null;
  status: RequestStatus | null;
  client: string | null;
  subscription: string | null;
  q: string;
  page: number;
}

const isRange = (v: string | null): v is LogRange => !!v && (RANGES as readonly string[]).includes(v);
const isPurpose = (v: string | null): v is PurposeChip => !!v && (PURPOSE_FILTERS as readonly string[]).includes(v);
const isStatus = (v: string | null): v is RequestStatus => v === "success" || v === "error" || v === "timeout";

export function parseLogQuery(sp: URLSearchParams): LogQuery {
  const range = sp.get(P.range);
  const purpose = sp.get(P.purpose);
  const status = sp.get(P.status);
  const page = Number(sp.get(P.page));
  return {
    range: isRange(range) ? range : DEFAULT_RANGE,
    purpose: isPurpose(purpose) ? purpose : "all",
    session: sp.get(P.session) || null,
    vm: (sp.get(P.vm) || null) as CustomVirtualModelName | null,
    provider: sp.get(P.provider) || null,
    status: isStatus(status) ? status : null,
    client: sp.get(P.client) || null,
    subscription: sp.get(P.subscription) || null,
    q: sp.get(P.q) ?? "",
    page: Number.isInteger(page) && page > 1 ? page : 1,
  };
}

/** 只写非默认值, 保持 URL 干净 */
export function toSearch(q: LogQuery): URLSearchParams {
  const sp = new URLSearchParams();
  if (q.range !== DEFAULT_RANGE) sp.set(P.range, q.range);
  if (q.purpose !== "all") sp.set(P.purpose, q.purpose);
  if (q.session) sp.set(P.session, q.session);
  if (q.vm) sp.set(P.vm, q.vm);
  if (q.provider) sp.set(P.provider, q.provider);
  if (q.status) sp.set(P.status, q.status);
  if (q.client) sp.set(P.client, q.client);
  if (q.subscription) sp.set(P.subscription, q.subscription);
  if (q.q.trim()) sp.set(P.q, q.q.trim());
  if (q.page > 1) sp.set(P.page, String(q.page));
  return sp;
}

/** 本地日历: today = 今天 0 点, 7d / 30d = 往前数 6 / 29 天的 0 点 (与统计页口径一致, 含今天) */
export function rangeSince(range: LogRange, now: number): number | undefined {
  if (range === "all") return undefined;
  if (range === "1h") return Math.floor(now / 60_000) * 60_000 - 3_600_000; // 按分钟取整, 避免查询键每次渲染都变
  const d = new Date(now);
  d.setHours(0, 0, 0, 0);
  const back = range === "today" ? 0 : range === "7d" ? 6 : 29;
  d.setDate(d.getDate() - back);
  return d.getTime();
}

export function toFilters(q: LogQuery, now: number): RequestLogFilters {
  return {
    virtual_model_name: q.vm ?? undefined,
    provider_id: q.provider ?? undefined,
    status: q.status ?? undefined,
    client_tool: q.client ?? undefined,
    subscription_id: q.subscription ?? undefined,
    since_ms: rangeSince(q.range, now),
    purpose: q.purpose === "all" ? undefined : (q.purpose as RequestPurposeFilter),
    session_id: q.session ?? undefined,
    q: q.q.trim() || undefined,
  };
}

export function hasNarrowing(q: LogQuery): boolean {
  return (
    q.purpose !== "all" || !!q.session || !!q.vm || !!q.provider || !!q.status || !!q.client || !!q.subscription || !!q.q.trim()
  );
}
