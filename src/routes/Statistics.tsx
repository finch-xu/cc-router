import { useMemo, useState } from "react";
import { RefreshCw } from "lucide-react";
import { useIsFetching, useQueryClient } from "@tanstack/react-query";
import { useT } from "@/i18n";
import {
  STATS_KEY,
  useBreakdown,
  useDailySeries,
  useOverallStats,
  useTokenHeatmap,
} from "@/hooks/useStatistics";
import type { StatsRange } from "@/types";
import { DailyLatencyChart } from "@/components/stats/DailyLatencyChart";
import { DailyRequestsChart } from "@/components/stats/DailyRequestsChart";
import { DailyTokensChart } from "@/components/stats/DailyTokensChart";
import { KpiRow } from "@/components/stats/KpiRow";
import { SubscriptionTable } from "@/components/stats/SubscriptionTable";
import { VmShareDonut } from "@/components/stats/VmShareDonut";
import { YearHeatmap } from "@/components/stats/YearHeatmap";
import { buildSeriesBuckets, isHourlyRange } from "@/components/stats/series";

const RANGES: { key: StatsRange; labelKey: string }[] = [
  { key: "today", labelKey: "stats.range.today" },
  { key: "last7_days", labelKey: "stats.range.last7" },
  { key: "last30_days", labelKey: "stats.range.last30" },
  { key: "last90_days", labelKey: "stats.range.last90" },
  { key: "all_time", labelKey: "stats.range.all" },
];

const HEATMAP_DAYS = 365;

/**
 * 页面分两块:
 * - 「全年概览」: GitHub 年历, 固定近 365 天, **不受**时间范围影响 (它的价值是形状, 不是数字)
 * - 「时段分析」: 一个共用 range 选择器, 其下 KPI / 趋势 / Token / 环图 / 耗时 / 订阅表全部同口径, 数字互相对得上
 * 所有日期口径都是本地日历日 (后端 migration 019 起)。
 */
export function StatisticsPage() {
  const { t } = useT();
  const queryClient = useQueryClient();
  const [range, setRange] = useState<StatsRange>("all_time");

  const overall = useOverallStats(range);
  const daily = useDailySeries(range);
  const heatmap = useTokenHeatmap(HEATMAP_DAYS);
  const byVm = useBreakdown(range, "virtual_model");
  const bySub = useBreakdown(range, "subscription");

  const isFetching = useIsFetching({ queryKey: [STATS_KEY] }) > 0;
  const refetchAll = () => queryClient.invalidateQueries({ queryKey: [STATS_KEY] });

  const hourly = isHourlyRange(range);
  const buckets = useMemo(() => buildSeriesBuckets(daily.data ?? [], range), [daily.data, range]);
  const err = (e: unknown) => (e ? t("stats.error.load") : null);

  return (
    <>
      <div className="page-actions">
        <div className="page-header" style={{ margin: 0 }}>
          <h1>{t("stats.title")}</h1>
          <div className="subtitle">{t("stats.subtitle")}</div>
        </div>
        <button className="btn" onClick={refetchAll} disabled={isFetching} type="button">
          <RefreshCw size={12} className={isFetching ? "spin" : undefined} />
          {t("stats.refresh")}
        </button>
      </div>

      <div className="stats-block-head">
        <div>
          <div className="stats-block-title">{t("stats.section.year")}</div>
          <div className="stats-block-sub">{t("stats.section.yearSub")}</div>
        </div>
      </div>
      <YearHeatmap days={heatmap.data ?? []} loading={heatmap.isFetching} errorText={err(heatmap.error)} />

      <div className="stats-block-head">
        <div>
          <div className="stats-block-title">{t("stats.section.period")}</div>
          <div className="stats-block-sub">{t("stats.section.periodSub")}</div>
        </div>
        <div className="range-tabs" role="tablist">
          {RANGES.map((r) => (
            <button
              key={r.key}
              type="button"
              role="tab"
              aria-selected={range === r.key}
              className={"range-tab" + (range === r.key ? " active" : "")}
              onClick={() => setRange(r.key)}
            >
              {t(r.labelKey)}
            </button>
          ))}
        </div>
      </div>

      <KpiRow stats={overall.data} loading={overall.isFetching} />
      <DailyRequestsChart buckets={buckets} hourly={hourly} loading={daily.isFetching} errorText={err(daily.error)} />
      <DailyTokensChart buckets={buckets} hourly={hourly} loading={daily.isFetching} errorText={err(daily.error)} />
      <div className="stats-grid-2">
        <VmShareDonut items={byVm.data ?? []} loading={byVm.isFetching} errorText={err(byVm.error)} />
        <DailyLatencyChart buckets={buckets} loading={daily.isFetching} errorText={err(daily.error)} />
      </div>
      <SubscriptionTable items={bySub.data ?? []} loading={bySub.isFetching} errorText={err(bySub.error)} />
    </>
  );
}
