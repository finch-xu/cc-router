/**
 * 请求日志列表 (设计稿 design/request-logs-v1.html ①)。
 * 行组件与实时路由共用 (components/requests/RequestRow), 数据是同一份摘要 (`items[].summary`);
 * 同一次入站请求里失败的尝试 (同 trace_id) 缩进挂在最终那次下面。
 * 筛选与页码全在 URL 里 (lib/requestLogQuery.ts)。
 */
import { Fragment, useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router";
import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { Download, RefreshCw, ScrollText, Search, X } from "lucide-react";
import { api } from "@/api/tauri";
import { runtime } from "@/runtime";
import { EmptyState } from "@/components/EmptyState";
import { Pagination } from "@/components/Pagination";
import { RequestDetailDialog } from "@/components/RequestDetailDialog";
import { PurposeChips, RequestRow } from "@/components/requests/RequestRow";
import { useLiveSessions } from "@/components/live/useLiveSessions";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { useCustomVirtualModels } from "@/hooks/useVirtualModels";
import { REQUESTS_KEY } from "@/hooks/useRequests";
import { useProviders } from "@/hooks/useProviders";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useT, type TFunction } from "@/i18n";
import { CLIENT_TOOLS } from "@/lib/clientTools";
import { localDayKey } from "@/lib/localDay";
import {
  hasNarrowing,
  parseLogQuery,
  PURPOSE_FILTERS,
  RANGES,
  toFilters,
  toSearch,
  type LogQuery,
  type PurposeChip,
} from "@/lib/requestLogQuery";
import { VM_META } from "@/lib/virtualModels";
import {
  CLIENT_TOOL_UNKNOWN_SENTINEL,
  type CustomVirtualModelName,
  type RequestLogDto,
  type RequestStatus,
} from "@/types";

const PAGE_SIZE = 50;
const ALL = "__all__";
const STATUSES: RequestStatus[] = ["success", "timeout", "error"];
const ZERO_COUNTS: Record<PurposeChip, number> = { all: 0, main: 0, agent: 0, bg: 0, fail: 0 };

/** 一条显示单元: 最终那次 + 挂在下面的失败尝试 (同 trace, 本页内) */
interface Group {
  head: RequestLogDto;
  attempts: RequestLogDto[];
}

function groupByTrace(items: readonly RequestLogDto[]): Group[] {
  const groups: Group[] = [];
  const byTrace = new Map<string, Group>();
  for (const it of items) {
    const trace = it.summary.trace_id;
    const g = trace ? byTrace.get(trace) : undefined;
    if (g) {
      g.attempts.push(it); // 列表是时间倒序: 先遇到的是最终那次
      continue;
    }
    const ng: Group = { head: it, attempts: [] };
    groups.push(ng);
    if (trace) byTrace.set(trace, ng);
  }
  return groups;
}

const dayOf = (ts: number) => localDayKey(new Date(ts));

function dayLabel(ts: number, t: TFunction): string {
  const key = dayOf(ts);
  const yesterday = new Date();
  yesterday.setDate(yesterday.getDate() - 1);
  // <html lang> 跟着界面语言走 (i18n/index.tsx), 日期格式与界面一致
  const date = new Date(ts).toLocaleDateString(document.documentElement.lang || undefined, {
    month: "long",
    day: "numeric",
    weekday: "short",
  });
  if (key === dayOf(Date.now())) return `${t("requestLogs.day.today")} · ${date}`;
  if (key === localDayKey(yesterday)) return `${t("requestLogs.day.yesterday")} · ${date}`;
  return date;
}

export function RequestLogsPage() {
  const { t } = useT();
  const [searchParams, setSearchParams] = useSearchParams();
  const query = useMemo(() => parseLogQuery(searchParams), [searchParams]);
  const customModels = useCustomVirtualModels();
  const subs = useSubscriptions();
  const providers = useProviders();
  const sessions = useLiveSessions();
  const [activeRequest, setActiveRequest] = useState<RequestLogDto | null>(null);
  const [exporting, setExporting] = useState(false);
  const [exportMsg, setExportMsg] = useState<string | null>(null);
  const [search, setSearch] = useState(query.q);

  // 「1 小时」按分钟滑动; 其余时间窗的起点只随日期变化
  const [minute, setMinute] = useState(() => Math.floor(Date.now() / 60_000));
  useEffect(() => {
    const id = window.setInterval(() => setMinute(Math.floor(Date.now() / 60_000)), 15_000);
    return () => window.clearInterval(id);
  }, []);
  const filters = useMemo(() => toFilters(query, minute * 60_000), [query, minute]);
  const live = query.page === 1; // 只有第 1 页自动刷新 (与 TUI 同规则)

  const list = useQuery({
    queryKey: [REQUESTS_KEY, "list", query.page, PAGE_SIZE, filters],
    queryFn: () => api.listRequests(query.page, PAGE_SIZE, filters),
    placeholderData: keepPreviousData,
    refetchInterval: live ? 5_000 : false,
  });
  const facets = useQuery({
    queryKey: [REQUESTS_KEY, "facets", filters],
    queryFn: () => api.getRequestFacets(filters),
    placeholderData: keepPreviousData,
    refetchInterval: live ? 5_000 : false,
  });

  const update = (patch: Partial<LogQuery>, keepPage = false) => {
    const next = { ...query, ...patch, page: keepPage ? (patch.page ?? query.page) : 1 };
    setSearchParams(toSearch(next), { replace: true });
  };

  // 搜索框: 停止输入 300ms 后写进 URL
  useEffect(() => setSearch(query.q), [query.q]);
  useEffect(() => {
    if (search === query.q) return;
    const id = window.setTimeout(() => update({ q: search }), 300);
    return () => window.clearTimeout(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [search]);

  const total = list.data?.total ?? 0;
  const totalPages = Math.max(1, Math.ceil(total / PAGE_SIZE));
  useEffect(() => {
    if (list.data && total > 0 && query.page > totalPages) update({ page: totalPages }, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [list.data, total, totalPages, query.page]);

  const groups = useMemo(() => groupByTrace(list.data?.items ?? []), [list.data]);
  const subsById = useMemo(() => new Map((subs.data ?? []).map((s) => [s.id, s])), [subs.data]);
  const subName = (id: string) => subsById.get(id)?.display_name;
  const narrowed = hasNarrowing(query);
  const subFilterLabel = query.subscription ? (subName(query.subscription) ?? query.subscription) : null;

  async function exportCsv() {
    try {
      const date = new Date().toISOString().slice(0, 10).replace(/-/g, "");
      const filename = `cc-router-requests-${date}.csv`;
      setExporting(true);
      if (runtime.kind === "web") {
        const text = await api.exportRequestsCsvText(filters);
        runtime.downloadText(filename, text, "text/csv;charset=utf-8");
        // 行数 = 换行数 - 1 (表头); 文本以 BOM 开头不影响计数
        const count = Math.max(0, text.split("\n").filter(Boolean).length - 1);
        setExportMsg(t("requestLogs.export.success", { count }));
      } else {
        const path = await runtime.pickSavePath({
          defaultName: filename,
          filters: [{ name: "CSV", extensions: ["csv"] }],
        });
        if (!path) return; // 用户取消
        const count = await api.exportRequestsCsv(path, filters);
        setExportMsg(t("requestLogs.export.success", { count }));
      }
    } catch (err) {
      console.warn("export csv failed", err);
      setExportMsg(t("requestLogs.export.failed"));
    } finally {
      setExporting(false);
      window.setTimeout(() => setExportMsg(null), 4500);
    }
  }

  const sessionChips = (facets.data?.sessions ?? []).map((s) => ({
    id: s.id,
    label: sessions.get(s.id)?.project ?? s.id.slice(0, 8),
  }));
  // 当前选中的会话不在「最近 8 个」里时也要显示出来, 否则无法取消
  if (query.session && !sessionChips.some((s) => s.id === query.session)) {
    sessionChips.unshift({ id: query.session, label: sessions.get(query.session)?.project ?? query.session.slice(0, 8) });
  }

  let lastDay = "";
  return (
    <>
      <div className="page-actions">
        <div className="page-header" style={{ margin: 0 }}>
          <h1>{t("requestLogs.title")}</h1>
          <div className="subtitle">
            {t("requestLogs.subtitleRange", { range: t(`requestLogs.range.${query.range}`), total })}
          </div>
        </div>
        <div className="rq-actions">
          {exportMsg && <span className="field-hint" style={{ fontSize: 11 }}>{exportMsg}</span>}
          <div className="range-tabs" role="tablist" aria-label={t("requestLogs.rangeLabel")}>
            {RANGES.map((r) => (
              <button
                key={r}
                type="button"
                role="tab"
                aria-selected={query.range === r}
                className={"range-tab" + (query.range === r ? " active" : "")}
                onClick={() => update({ range: r })}
              >
                {t(`requestLogs.range.${r}`)}
              </button>
            ))}
          </div>
          <button className="btn sm" onClick={() => void exportCsv()} disabled={exporting} type="button">
            <Download size={12} />
            {exporting ? t("requestLogs.export.exporting") : t("requestLogs.export.button")}
          </button>
          <button className="btn sm" onClick={() => void list.refetch()} disabled={list.isFetching} type="button">
            <RefreshCw size={12} className={list.isFetching ? "spin" : undefined} />
            {t("requestLogs.refresh")}
          </button>
        </div>
      </div>

      <section className="card rq-card">
        <div className="card-body">
          <PurposeChips
            filters={PURPOSE_FILTERS}
            value={query.purpose}
            counts={facets.data?.purpose ?? ZERO_COUNTS}
            onChange={(purpose) => update({ purpose })}
            sessions={sessionChips}
            session={query.session}
            onSession={(session) => update({ session })}
            t={t}
          />
          <div className="rq-filters">
            <Select value={query.vm ?? ALL} onValueChange={(v) => update({ vm: v === ALL ? null : (v as CustomVirtualModelName) })}>
              <SelectTrigger className="w-auto min-w-[130px]"><SelectValue /></SelectTrigger>
              <SelectContent>
                <SelectItem value={ALL}>{t("requestLogs.filter.allVm")}</SelectItem>
                {[...Object.keys(VM_META), ...(customModels.data ?? []).map((m) => m.name)].map((name) => (
                  <SelectItem key={name} value={name}>{name}</SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Select value={query.provider ?? ALL} onValueChange={(v) => update({ provider: v === ALL ? null : v })}>
              <SelectTrigger className="w-auto min-w-[130px]"><SelectValue /></SelectTrigger>
              <SelectContent>
                <SelectItem value={ALL}>{t("requestLogs.filter.allProvider")}</SelectItem>
                {providers.data?.map((p) => <SelectItem key={p.id} value={p.id}>{p.display_name}</SelectItem>)}
              </SelectContent>
            </Select>
            <Select value={query.status ?? ALL} onValueChange={(v) => update({ status: v === ALL ? null : (v as RequestStatus) })}>
              <SelectTrigger className="w-auto min-w-[120px]"><SelectValue /></SelectTrigger>
              <SelectContent>
                <SelectItem value={ALL}>{t("requestLogs.filter.allStatus")}</SelectItem>
                {STATUSES.map((s) => <SelectItem key={s} value={s}>{t(`requestLogs.status.${s}`)}</SelectItem>)}
              </SelectContent>
            </Select>
            <Select value={query.client ?? ALL} onValueChange={(v) => update({ client: v === ALL ? null : v })}>
              <SelectTrigger className="w-auto min-w-[130px]"><SelectValue /></SelectTrigger>
              <SelectContent>
                <SelectItem value={ALL}>{t("requestLogs.filter.allClient")}</SelectItem>
                {CLIENT_TOOLS.map((c) => <SelectItem key={c.id} value={c.id}>{t(c.i18nKey)}</SelectItem>)}
                <SelectItem value={CLIENT_TOOL_UNKNOWN_SENTINEL}>{t("requestLogs.filter.clientUnknown")}</SelectItem>
              </SelectContent>
            </Select>
            {subFilterLabel && (
              <span className="pill accent rq-filter-pill">
                <span className="dot" />
                {subFilterLabel}
                <button type="button" onClick={() => update({ subscription: null })} aria-label={t("requestLogs.clearFilter")}>
                  <X size={12} />
                </button>
              </span>
            )}
            <label className="rq-search">
              <Search size={13} />
              <input
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder={t("requestLogs.searchPlaceholder")}
                aria-label={t("requestLogs.searchPlaceholder")}
              />
              {search && (
                <button type="button" onClick={() => setSearch("")} aria-label={t("requestLogs.clearFilter")}>
                  <X size={12} />
                </button>
              )}
            </label>
          </div>

          {list.isLoading && <div className="field-hint">{t("common.loading")}</div>}
          {list.data && total === 0 && (
            <EmptyState
              icon={ScrollText}
              message={narrowed || query.range !== "all" ? t("requestLogs.empty.filtered") : t("requestLogs.empty.none")}
              action={
                narrowed ? (
                  <button className="btn sm" onClick={() => setSearchParams(toSearch({ ...parseLogQuery(new URLSearchParams()), range: query.range }))} type="button">
                    {t("requestLogs.clearFilter")}
                  </button>
                ) : undefined
              }
            />
          )}
          {groups.length > 0 && (
            <div className="rq-list">
              {groups.map(({ head, attempts }) => {
                const day = dayOf(head.timestamp);
                const sep = day !== lastDay ? <div className="rq-day">{dayLabel(head.timestamp, t)}</div> : null;
                lastDay = day;
                // 失败的尝试按时间正序排在最终那次下面; 「改走」= 下一次尝试的订阅
                const chain = [...attempts].reverse();
                const order = [...chain, head];
                const total = head.summary.retry_count + 1;
                return (
                  <Fragment key={head.id}>
                    {sep}
                    <RequestRow
                      r={head.summary}
                      sub={subsById.get(head.subscription_id)}
                      session={head.summary.session ? sessions.get(head.summary.session.id) : undefined}
                      mode="link"
                      onActivate={() => setActiveRequest(head)}
                      t={t}
                    />
                    {chain.map((a, i) => (
                      <RequestRow
                        key={a.id}
                        r={a.summary}
                        sub={subsById.get(a.subscription_id)}
                        session={undefined}
                        next={subName(order[i + 1].subscription_id)}
                        attempt={{ index: a.summary.retry_count + 1, total: Math.max(total, a.summary.retry_count + 1) }}
                        mode="link"
                        onActivate={() => setActiveRequest(a)}
                        t={t}
                      />
                    ))}
                  </Fragment>
                );
              })}
            </div>
          )}

          <Pagination
            total={total}
            page={query.page}
            totalPages={totalPages}
            onChange={(page) => update({ page }, true)}
            trailing={live && total > 0 ? <span className="field-hint rq-autorefresh">{t("requestLogs.autoRefresh")}</span> : undefined}
          />
        </div>
      </section>

      <RequestDetailDialog request={activeRequest} onClose={() => setActiveRequest(null)} />
    </>
  );
}
