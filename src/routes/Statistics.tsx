import { useMemo, useState } from "react";
import { RefreshCw } from "lucide-react";
import { useIsFetching, useQueryClient } from "@tanstack/react-query";
import { useT } from "@/i18n";
import {
  STATS_KEY,
  useBreakdown,
  useCostSummary,
  useDailySeries,
  useOverallStats,
  useRequestMix,
  useTokenHeatmap,
  useToolBreakdown,
} from "@/hooks/useStatistics";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import type { StatsRange } from "@/types";
import { StatsHero } from "@/components/stats/StatsHero";
import {
  LatencyCard,
  PurposeCard,
  SubscriptionPerformance,
  TokenFlowCard,
  ToolTopCard,
} from "@/components/stats/StatsBreakdowns";
import { YearHeatmap } from "@/components/stats/YearHeatmap";
import { CostCard } from "@/components/stats/CostCard";
import { buildSeriesBuckets } from "@/components/stats/series";
import "@/components/stats/stats-page.css";

const RANGES: { key: StatsRange; labelKey: string }[] = [
  { key: "today", labelKey: "stats.range.today" },
  { key: "last7_days", labelKey: "stats.range.last7" },
  { key: "last30_days", labelKey: "stats.range.last30" },
  { key: "last90_days", labelKey: "stats.range.last90" },
  { key: "all_time", labelKey: "stats.range.all" },
];

const HEATMAP_DAYS = 365;

/**
 * 数据统计 (设计稿: design/subscriptions-stats-v1.html)
 * 时间范围在页头, 页面内一切 (除全年热力图) 同口径, 数字互相对得上。
 * 主卡 = KPI (带较上一周期) + 可切换趋势图; 下面按「Token 去了哪 → 用来干什么 → 谁在用」排;
 * 全年热力图固定近 365 天, 放在最后。日期口径都是本地日历日 (后端 migration 019 起)。
 */
export function StatisticsPage() {
  const { t } = useT();
  const queryClient = useQueryClient();
  const [range, setRange] = useState<StatsRange>("last7_days");

  const overall = useOverallStats(range);
  const daily = useDailySeries(range);
  const heatmap = useTokenHeatmap(HEATMAP_DAYS);
  const byVm = useBreakdown(range, "virtual_model");
  const bySub = useBreakdown(range, "subscription");
  const tools = useToolBreakdown(range, 10);
  const mix = useRequestMix(range);
  const cost = useCostSummary(range);
  const subs = useSubscriptions();

  const isFetching = useIsFetching({ queryKey: [STATS_KEY] }) > 0;
  const refetchAll = () => queryClient.invalidateQueries({ queryKey: [STATS_KEY] });

  const buckets = useMemo(() => buildSeriesBuckets(daily.data ?? [], range), [daily.data, range]);
  const err = (e: unknown) => (e ? t("stats.error.load") : null);
  const rangeLabel = t(RANGES.find((r) => r.key === range)!.labelKey);
  const subList = subs.data ?? [];

  return (
    <div className="sx-page">
      <div className="page-actions">
        <div className="page-header" style={{ marginBottom: 0 }}>
          <h1>{t("stats.title")}</h1>
          <div className="subtitle">{t("stats.subtitle")}</div>
        </div>
        <div className="sx-actions">
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
          <button className="btn sm" onClick={refetchAll} disabled={isFetching} type="button" title={t("stats.refresh")}>
            <RefreshCw size={13} className={isFetching ? "spin" : undefined} />
            {t("stats.refresh")}
          </button>
        </div>
      </div>

      <StatsHero
        overall={overall.data}
        buckets={buckets}
        range={range}
        loading={overall.isFetching || daily.isFetching}
        errorText={err(overall.error ?? daily.error)}
      />

      <div className="sx-grid wide">
        <TokenFlowCard
          overall={overall.data}
          byVm={byVm.data ?? []}
          bySub={bySub.data ?? []}
          mix={mix.data}
          subs={subList}
          range={range}
          rangeLabel={rangeLabel}
          loading={byVm.isFetching || bySub.isFetching}
          errorText={err(overall.error ?? byVm.error ?? bySub.error)}
        />
        <PurposeCard mix={mix.data} overall={overall.data} range={range} loading={mix.isFetching} errorText={err(mix.error)} />
      </div>

      <CostCard data={cost.data} loading={cost.isFetching} errorText={err(cost.error)} />

      <SubscriptionPerformance
        items={bySub.data ?? []}
        subs={subList}
        range={range}
        loading={bySub.isFetching}
        errorText={err(bySub.error)}
      />

      <div className="sx-grid">
        <ToolTopCard items={tools.data ?? []} range={range} loading={tools.isFetching} errorText={err(tools.error)} />
        <LatencyCard mix={mix.data} overall={overall.data} loading={mix.isFetching} errorText={err(mix.error)} />
      </div>

      <YearHeatmap days={heatmap.data ?? []} loading={heatmap.isFetching} errorText={err(heatmap.error)} />
    </div>
  );
}
