import { ReactNode, useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { RefreshCw } from "lucide-react";
import {
  Bar,
  BarChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { useIsFetching, useQueryClient } from "@tanstack/react-query";
import { ProviderLogo } from "@/components/ProviderLogo";
import { useT } from "@/i18n";
import { fmtNum, fmtKilo } from "@/lib/format";
import { VM_META, VM_ORDER } from "@/lib/virtualModels";
import {
  STATS_KEY,
  useBreakdown,
  useDailySeries,
  useOverallStats,
  useTokenHeatmap,
} from "@/hooks/useStatistics";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useProviders } from "@/hooks/useProviders";
import type {
  BreakdownDto,
  DailySeriesPointDto,
  HeatmapDayDto,
  StatsRange,
  SubscriptionDto,
  ProviderInfo,
  VirtualModelName,
} from "@/types";

const RANGES: { key: StatsRange; labelKey: string }[] = [
  { key: "today", labelKey: "stats.range.today" },
  { key: "last7_days", labelKey: "stats.range.last7" },
  { key: "last30_days", labelKey: "stats.range.last30" },
  { key: "last90_days", labelKey: "stats.range.last90" },
  { key: "all_time", labelKey: "stats.range.all" },
];

const HEATMAP_DAYS = 365;
const DAY_MS = 86_400_000;

function StatsSection({
  title,
  subtitle,
  isEmpty,
  emptyText,
  children,
}: {
  title: string;
  subtitle: string;
  isEmpty: boolean;
  emptyText: string;
  children: ReactNode;
}) {
  return (
    <div className="stats-section">
      <div className="stats-section-header">
        <div className="stats-section-title">{title}</div>
        <div className="stats-section-subtitle">{subtitle}</div>
      </div>
      {isEmpty ? <div className="field-hint">{emptyText}</div> : children}
    </div>
  );
}

export function StatisticsPage() {
  const { t } = useT();
  const queryClient = useQueryClient();
  const [range, setRange] = useState<StatsRange>("last30_days");

  const overall = useOverallStats(range);
  const daily = useDailySeries(range);
  const heatmap = useTokenHeatmap(HEATMAP_DAYS);
  const byVm = useBreakdown(range, "virtual_model");
  const bySub = useBreakdown(range, "subscription");

  const isFetching =
    useIsFetching({ queryKey: [STATS_KEY] }) > 0;
  const refetchAll = () =>
    queryClient.invalidateQueries({ queryKey: [STATS_KEY] });

  const o = overall.data;
  const failCount = (o?.error_count ?? 0) + (o?.timeout_count ?? 0);

  return (
    <>
      <div className="page-actions">
        <div className="page-header" style={{ margin: 0 }}>
          <h1>{t("stats.title")}</h1>
          <div className="subtitle">{t("stats.subtitle")}</div>
        </div>
        <button
          className="btn"
          onClick={refetchAll}
          disabled={isFetching}
          type="button"
        >
          <RefreshCw size={12} className={isFetching ? "spin" : undefined} />
          {t("stats.refresh")}
        </button>
      </div>

      <div className="range-tabs">
        {RANGES.map((r) => (
          <button
            key={r.key}
            type="button"
            className={"range-tab" + (range === r.key ? " active" : "")}
            onClick={() => setRange(r.key)}
          >
            {t(r.labelKey)}
          </button>
        ))}
      </div>

      <div className="log-stats">
        <div className="stat">
          <div className="stat-label">{t("stats.kpi.totalRequests")}</div>
          <div className="stat-val tnum">{fmtNum(o?.total_requests ?? 0)}</div>
          <div className="stat-delta">
            {o?.success_count ?? 0} ✓ · {failCount} ✕
          </div>
        </div>
        <div className="stat">
          <div className="stat-label">{t("stats.kpi.successRate")}</div>
          <div className="stat-val tnum">
            {(o?.success_rate_pct ?? 0).toFixed(1)}
            <span className="stat-unit">%</span>
          </div>
          <div className={"stat-delta" + (failCount > 0 ? " down" : "")}>
            {t("stats.kpi.failedFormat", {
              failed: failCount,
              total: o?.total_requests ?? 0,
            })}
          </div>
        </div>
        <div className="stat">
          <div className="stat-label">{t("stats.kpi.avgDuration")}</div>
          <div className="stat-val tnum">
            {o?.avg_duration_ms != null ? (o.avg_duration_ms / 1000).toFixed(2) : "-"}
            <span className="stat-unit">s</span>
          </div>
          <div className="stat-delta">
            {o?.p95_duration_ms != null
              ? `${t("stats.kpi.p95Prefix")}${(o.p95_duration_ms / 1000).toFixed(2)}${t("stats.kpi.p95Suffix")}`
              : ""}
          </div>
        </div>
        <div className="stat">
          <div className="stat-label">{t("stats.kpi.totalTokensOut")}</div>
          <div className="stat-val tnum">{fmtKilo(o?.total_output_tokens ?? 0)}</div>
          <div className="stat-delta">
            {t("stats.kpi.tokensInPrefix")}
            {fmtKilo(o?.total_input_tokens ?? 0)}
            {t("stats.kpi.tokensInSuffix")}
          </div>
        </div>
      </div>

      <HeatmapSection days={heatmap.data ?? []} />
      <DailyTrendSection points={daily.data ?? []} />
      <VmBreakdownSection items={byVm.data ?? []} />
      <SubscriptionRankingSection items={bySub.data ?? []} />
    </>
  );
}

// ============== Heatmap (GitHub style) ==============

interface HeatmapBucket {
  day: HeatmapDayDto | null;
  date_utc: number;
}

function HeatmapSection({ days }: { days: HeatmapDayDto[] }) {
  const { t } = useT();

  const { buckets, monthMarkers, levels } = useMemo(() => {
    // 用 UTC 0 点而非本地, 跟后端聚合 key 严格对齐, 避免跨日错位
    const todayUtc = Math.floor(Date.now() / DAY_MS) * DAY_MS;
    const since = todayUtc - (HEATMAP_DAYS - 1) * DAY_MS;
    const map = new Map<number, HeatmapDayDto>();
    days.forEach((d) => map.set(d.date_utc, d));

    // 让第一列从「上周日」开始, 列向下从 Sun 排到 Sat (与 GitHub 一致)
    const firstDow = new Date(since).getUTCDay();
    const gridStart = since - firstDow * DAY_MS;
    const totalCells = Math.ceil((todayUtc - gridStart) / DAY_MS) + 1;
    const padded = Math.ceil(totalCells / 7) * 7;

    const buckets: HeatmapBucket[] = [];
    for (let i = 0; i < padded; i++) {
      const date_utc = gridStart + i * DAY_MS;
      buckets.push({
        date_utc,
        day: date_utc <= todayUtc && date_utc >= since ? map.get(date_utc) ?? null : null,
      });
    }

    // quintile 分级基于非零样本, 避免「365 天里 300 天 0」把分位拖到 0
    const positives = days.filter((d) => d.total_tokens > 0).map((d) => d.total_tokens);
    positives.sort((a, b) => a - b);
    const quart = (p: number) => {
      if (positives.length === 0) return 0;
      return positives[Math.floor(positives.length * p)] ?? 0;
    };
    const q1 = quart(0.25);
    const q2 = quart(0.5);
    const q3 = quart(0.75);

    const levels = (tokens: number): 0 | 1 | 2 | 3 | 4 => {
      if (tokens <= 0) return 0;
      if (tokens <= q1) return 1;
      if (tokens <= q2) return 2;
      if (tokens <= q3) return 3;
      return 4;
    };

    const monthMarkers: { col: number; label: string }[] = [];
    let lastMonth = -1;
    for (let col = 0; col < buckets.length / 7; col++) {
      const ts = buckets[col * 7].date_utc;
      const m = new Date(ts).getUTCMonth();
      if (m !== lastMonth) {
        monthMarkers.push({
          col,
          label: new Date(ts).toLocaleDateString(undefined, { month: "short" }),
        });
        lastMonth = m;
      }
    }

    return { buckets, monthMarkers, levels };
  }, [days]);

  const fmtTooltip = (b: HeatmapBucket) =>
    t("stats.heatmap.tooltipFormat", {
      date: new Date(b.date_utc).toLocaleDateString(),
      tokens: b.day ? fmtNum(b.day.total_tokens) : 0,
      requests: b.day?.request_count ?? 0,
    });

  const isEmpty = days.every((d) => d.total_tokens === 0);

  return (
    <StatsSection
      title={t("stats.heatmap.title")}
      subtitle={t("stats.heatmap.subtitle")}
      isEmpty={isEmpty}
      emptyText={t("stats.heatmap.empty")}
    >
      <div className="heatmap-wrap">
        <div className="heatmap-day-labels">
          <span></span>
          <span>Mon</span>
          <span></span>
          <span>Wed</span>
          <span></span>
          <span>Fri</span>
          <span></span>
        </div>
        <div className="heatmap-grid-wrap">
          <div className="heatmap-month-row">
            {monthMarkers.map((m, i) => (
              <span
                key={i}
                className="heatmap-month-label"
                style={{ left: `${m.col * 14}px` }}
              >
                {m.label}
              </span>
            ))}
          </div>
          <div className="heatmap-grid">
            {buckets.map((b, i) => {
              const level = b.day ? levels(b.day.total_tokens) : 0;
              return (
                <span
                  key={i}
                  className={`heatmap-cell${level > 0 ? ` l${level}` : ""}`}
                  title={fmtTooltip(b)}
                />
              );
            })}
          </div>
        </div>
      </div>
      <div className="heatmap-legend">
        <span>{t("stats.heatmap.legendLess")}</span>
        <span className="heatmap-cell" />
        <span className="heatmap-cell l1" />
        <span className="heatmap-cell l2" />
        <span className="heatmap-cell l3" />
        <span className="heatmap-cell l4" />
        <span>{t("stats.heatmap.legendMore")}</span>
      </div>
    </StatsSection>
  );
}

// ============== Daily trend (recharts) ==============

interface DailyChartPoint {
  date: string;
  success: number;
  error: number;
  timeout: number;
  total: number;
}

function DailyTrendSection({ points }: { points: DailySeriesPointDto[] }) {
  const { t } = useT();
  const data: DailyChartPoint[] = points.map((p) => ({
    date: new Date(p.date_utc).toLocaleDateString(undefined, {
      month: "numeric",
      day: "numeric",
    }),
    success: p.success_count,
    error: p.error_count,
    timeout: p.timeout_count,
    total: p.request_count,
  }));

  return (
    <StatsSection
      title={t("stats.daily.title")}
      subtitle={t("stats.daily.subtitle")}
      isEmpty={points.length === 0}
      emptyText={t("stats.daily.empty")}
    >
      <ResponsiveContainer width="100%" height={220}>
        <BarChart data={data} margin={{ top: 8, right: 12, bottom: 0, left: -16 }}>
          <CartesianGrid strokeDasharray="3 3" stroke="var(--line)" vertical={false} />
          <XAxis dataKey="date" tick={{ fontSize: 10, fill: "var(--ink-3)" }} stroke="var(--line-2)" />
          <YAxis tick={{ fontSize: 10, fill: "var(--ink-3)" }} stroke="var(--line-2)" allowDecimals={false} />
          <Tooltip content={<DailyTooltip />} cursor={{ fill: "var(--surface-2)" }} />
          <Bar dataKey="success" stackId="x" fill="oklch(0.62 0.13 150)" />
          <Bar dataKey="error" stackId="x" fill="oklch(0.58 0.18 28)" />
          <Bar dataKey="timeout" stackId="x" fill="var(--ink-4)" radius={[2, 2, 0, 0]} />
        </BarChart>
      </ResponsiveContainer>
    </StatsSection>
  );
}

function DailyTooltip({
  active,
  payload,
}: {
  active?: boolean;
  payload?: { payload: DailyChartPoint }[];
}) {
  const { t } = useT();
  if (!active || !payload || payload.length === 0) return null;
  const p = payload[0].payload;
  return (
    <div className="stats-tooltip">
      <div className="tt-row">
        <span className="tt-label">{p.date}</span>
        <span className="tt-val">{p.total}</span>
      </div>
      <div className="tt-row">
        <span className="tt-label">{t("stats.daily.tooltipSuccess")}</span>
        <span className="tt-val">{p.success}</span>
      </div>
      {p.error > 0 && (
        <div className="tt-row">
          <span className="tt-label">{t("stats.daily.tooltipError")}</span>
          <span className="tt-val">{p.error}</span>
        </div>
      )}
      {p.timeout > 0 && (
        <div className="tt-row">
          <span className="tt-label">{t("stats.daily.tooltipTimeout")}</span>
          <span className="tt-val">{p.timeout}</span>
        </div>
      )}
    </div>
  );
}

// ============== 虚拟模型分布 (横向条形) ==============

function VmBreakdownSection({ items }: { items: BreakdownDto[] }) {
  const { t } = useT();

  // 按 VM_ORDER (opus/sonnet/haiku/fallback) 稳定排序, 后端字面量 label 通过 VM_META 走 i18n
  const byKey = new Map(items.map((it) => [it.key, it]));
  const ordered = VM_ORDER.map((name) => ({
    name,
    item: byKey.get(name),
  })).filter((x) => x.item && x.item.request_count > 0) as {
    name: VirtualModelName;
    item: BreakdownDto;
  }[];

  const maxCount = Math.max(...ordered.map((x) => x.item.request_count), 1);

  return (
    <StatsSection
      title={t("stats.byVm.title")}
      subtitle={t("stats.byVm.subtitle")}
      isEmpty={ordered.length === 0}
      emptyText={t("stats.byVm.empty")}
    >
      {ordered.map(({ name, item }) => {
        const pct = (item.request_count / maxCount) * 100;
        const successPct =
          item.request_count > 0 ? (item.success_count / item.request_count) * 100 : 0;
        return (
          <div key={name} className="bar-row">
            <span className="mono bar-row-label">{t(VM_META[name].labelKey)}</span>
            <div className="bar-track">
              <div className="bar-fill" style={{ width: `${pct}%` }} />
            </div>
            <span className="bar-meta">
              {fmtNum(item.request_count)}
              <span className="bar-meta-sub">{successPct.toFixed(0)}%</span>
            </span>
          </div>
        );
      })}
    </StatsSection>
  );
}

// ============== 订阅排行 ==============

function SubscriptionRankingSection({ items }: { items: BreakdownDto[] }) {
  const { t } = useT();
  const navigate = useNavigate();
  const subs = useSubscriptions();
  const providers = useProviders();

  // 一次构建 id → 详情索引, 避免行渲染时 N×M 的 array.find
  const subById = useMemo(
    () => new Map<string, SubscriptionDto>((subs.data ?? []).map((s) => [s.id, s])),
    [subs.data],
  );
  const providerById = useMemo(
    () => new Map<string, ProviderInfo>((providers.data ?? []).map((p) => [p.id, p])),
    [providers.data],
  );

  return (
    <StatsSection
      title={t("stats.bySub.title")}
      subtitle={t("stats.bySub.subtitle")}
      isEmpty={items.length === 0}
      emptyText={t("stats.bySub.empty")}
    >
      <table className="table">
        <thead>
          <tr>
            <th>{t("stats.bySub.col.subscription")}</th>
            <th style={{ width: 110, textAlign: "right" }}>
              {t("stats.bySub.col.requests")}
            </th>
            <th style={{ width: 110, textAlign: "right" }}>
              {t("stats.bySub.col.successRate")}
            </th>
            <th style={{ width: 110, textAlign: "right" }}>
              {t("stats.bySub.col.avgDuration")}
            </th>
            <th style={{ width: 130, textAlign: "right" }}>
              {t("stats.bySub.col.tokensOut")}
            </th>
          </tr>
        </thead>
        <tbody>
          {items.map((it) => {
            const sub = subById.get(it.key);
            const provider = sub ? providerById.get(sub.provider_id) : undefined;
            const successPct =
              it.request_count > 0 ? (it.success_count / it.request_count) * 100 : 0;
            return (
              <tr
                key={it.key}
                onClick={() => navigate(`/request-logs?subscription_id=${it.key}`)}
                style={{ cursor: "pointer" }}
              >
                <td>
                  <div className="cell-with-icon">
                    <ProviderLogo iconId={provider?.icon} size={18} iconSize={12} />
                    <span className="cell-with-icon-label">{it.label}</span>
                  </div>
                </td>
                <td className="mono tnum strong" style={{ textAlign: "right" }}>
                  {fmtNum(it.request_count)}
                </td>
                <td className="mono tnum" style={{ textAlign: "right" }}>
                  {successPct.toFixed(1)}%
                </td>
                <td className="mono tnum" style={{ textAlign: "right" }}>
                  {it.avg_duration_ms != null
                    ? `${(it.avg_duration_ms / 1000).toFixed(2)}s`
                    : "-"}
                </td>
                <td className="mono tnum" style={{ textAlign: "right" }}>
                  {fmtKilo(it.total_output_tokens)}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </StatsSection>
  );
}
