import { useMemo } from "react";
import { useT } from "@/i18n";
import { fmtNum } from "@/lib/format";
import { localDayKey, localDaysAgo, localToday } from "@/lib/localDay";
import type { HeatmapDayDto } from "@/types";
import { StatsCard } from "./StatsCard";

const HEATMAP_DAYS = 365;
/** 一列的像素宽 = 格子 11px + gap 3px; 与 styles.css `.heatmap-grid` 的 grid-auto-columns / gap 成对, 改一处必改另一处 */
const HM_COL_PX = 14;

interface HeatmapBucket {
  date: Date;
  key: string;
  day: HeatmapDayDto | null;
  inRange: boolean;
}

/**
 * GitHub 风格年历: 近 365 个**本地日历日**, 固定不受时间范围影响。
 * key 用 `localDayKey` (本地 getter) 与后端 `day` 列对齐; 星期/月份都走本地 getter。
 */
export function YearHeatmap({
  days,
  loading,
  errorText,
}: {
  days: HeatmapDayDto[];
  loading?: boolean;
  errorText?: string | null;
}) {
  const { t } = useT();

  const { buckets, monthMarkers, levels } = useMemo(() => {
    const today = localToday();
    const since = localDaysAgo(HEATMAP_DAYS - 1);
    const map = new Map<string, HeatmapDayDto>();
    days.forEach((d) => map.set(d.day, d));

    // 第一列从「上周日」开始, 列向下 Sun→Sat (与 GitHub 一致); 末尾补齐到 7 的倍数
    const gridStart = new Date(since);
    gridStart.setDate(gridStart.getDate() - since.getDay());
    const buckets: HeatmapBucket[] = [];
    const cur = new Date(gridStart);
    while (cur.getTime() <= today.getTime() || buckets.length % 7 !== 0) {
      const key = localDayKey(cur);
      const inRange = cur.getTime() >= since.getTime() && cur.getTime() <= today.getTime();
      buckets.push({ date: new Date(cur), key, day: inRange ? (map.get(key) ?? null) : null, inRange });
      cur.setDate(cur.getDate() + 1);
    }

    // 四分位分级基于非零样本, 避免「365 天里 300 天 0」把分位拖到 0
    const positives = days.filter((d) => d.total_tokens > 0).map((d) => d.total_tokens);
    positives.sort((a, b) => a - b);
    const quart = (p: number) =>
      positives.length === 0 ? 0 : (positives[Math.floor(positives.length * p)] ?? 0);
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
      const d = buckets[col * 7].date;
      const m = d.getMonth();
      if (m !== lastMonth) {
        monthMarkers.push({ col, label: d.toLocaleDateString(undefined, { month: "short" }) });
        lastMonth = m;
      }
    }
    return { buckets, monthMarkers, levels };
  }, [days]);

  const fmtTooltip = (b: HeatmapBucket) =>
    t("stats.heatmap.tooltipFormat", {
      date: b.date.toLocaleDateString(),
      tokens: b.day ? fmtNum(b.day.total_tokens) : 0,
      requests: b.day?.request_count ?? 0,
    });

  const isEmpty = days.every((d) => d.total_tokens === 0);

  return (
    <StatsCard
      title={t("stats.heatmap.title")}
      subtitle={t("stats.heatmap.subtitle")}
      isEmpty={isEmpty}
      emptyText={t("stats.heatmap.empty")}
      loading={loading}
      errorText={errorText}
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
              <span key={i} className="heatmap-month-label" style={{ left: `${m.col * HM_COL_PX}px` }}>
                {m.label}
              </span>
            ))}
          </div>
          <div className="heatmap-grid">
            {buckets.map((b) => {
              const level = b.day ? levels(b.day.total_tokens) : 0;
              return (
                <span
                  key={b.key}
                  className={`heatmap-cell${level > 0 ? ` l${level}` : ""}${b.inRange ? "" : " out"}`}
                  title={b.inRange ? fmtTooltip(b) : undefined}
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
    </StatsCard>
  );
}
