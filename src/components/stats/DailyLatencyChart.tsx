import { useMemo } from "react";
import { CartesianGrid, Line, LineChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";
import { useT } from "@/i18n";
import { ChartTooltip } from "./ChartTooltip";
import { StatsCard } from "./StatsCard";
import type { SeriesBucket } from "./series";

interface Row {
  label: string;
  title: string;
  /** 秒; 该桶没有耗时样本时为 null (断线而非画 0) */
  sec: number | null;
}

/** 单系列折线: 每个时间桶的平均耗时。单系列不配图例, 标题即说明。 */
export function DailyLatencyChart({
  buckets,
  loading,
  errorText,
}: {
  buckets: SeriesBucket[];
  loading?: boolean;
  errorText?: string | null;
}) {
  const { t } = useT();
  const data: Row[] = useMemo(
    () =>
      buckets.map((b) => ({
        label: b.label,
        title: b.title,
        sec: b.point?.avg_duration_ms != null ? b.point.avg_duration_ms / 1000 : null,
      })),
    [buckets],
  );
  const isEmpty = data.every((d) => d.sec == null);

  return (
    <StatsCard
      title={t("stats.latency.title")}
      subtitle={t("stats.latency.subtitle")}
      isEmpty={isEmpty}
      emptyText={t("stats.latency.empty")}
      loading={loading}
      errorText={errorText}
    >
      <ResponsiveContainer width="100%" height={220}>
        <LineChart data={data} margin={{ top: 8, right: 12, bottom: 0, left: -16 }}>
          <CartesianGrid stroke="var(--line)" vertical={false} />
          <XAxis dataKey="label" tick={{ fontSize: 10, fill: "var(--ink-3)" }} stroke="var(--line-2)" tickLine={false} />
          <YAxis
            tick={{ fontSize: 10, fill: "var(--ink-3)" }}
            stroke="var(--line-2)"
            tickLine={false}
            tickFormatter={(v: number) => `${v}s`}
          />
          <Tooltip
            cursor={{ stroke: "var(--line-2)" }}
            content={({ active, payload }) => {
              const p = active && payload && payload.length > 0 ? (payload[0].payload as Row) : null;
              if (!p || p.sec == null) return null;
              return (
                <ChartTooltip
                  title={p.title}
                  rows={[{ label: t("stats.latency.series"), value: `${p.sec.toFixed(2)} s`, color: "var(--accent)", strong: true }]}
                />
              );
            }}
          />
          <Line
            type="monotone"
            dataKey="sec"
            stroke="var(--accent)"
            strokeWidth={2}
            dot={false}
            activeDot={{ r: 4, fill: "var(--accent)", stroke: "var(--surface)", strokeWidth: 2 }}
            connectNulls={false}
            isAnimationActive={false}
          />
        </LineChart>
      </ResponsiveContainer>
    </StatsCard>
  );
}
