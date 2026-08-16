import { useMemo } from "react";
import { Bar, BarChart, CartesianGrid, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";
import { useT } from "@/i18n";
import { fmtNum } from "@/lib/format";
import { ChartLegend, ChartTooltip } from "./ChartTooltip";
import { StatsCard } from "./StatsCard";
import type { SeriesBucket } from "./series";

interface Row {
  label: string;
  title: string;
  success: number;
  error: number;
  timeout: number;
  total: number;
}

// 状态色直接复用 app 的语义 token, 暗色下自动提亮
const SERIES = [
  { key: "success", color: "var(--ok)", labelKey: "stats.daily.tooltipSuccess" },
  { key: "error", color: "var(--err)", labelKey: "stats.daily.tooltipError" },
  { key: "timeout", color: "var(--ink-4)", labelKey: "stats.daily.tooltipTimeout" },
] as const;

export function DailyRequestsChart({
  buckets,
  hourly,
  loading,
  errorText,
}: {
  buckets: SeriesBucket[];
  hourly: boolean;
  loading?: boolean;
  errorText?: string | null;
}) {
  const { t } = useT();
  const data: Row[] = useMemo(
    () =>
      buckets.map((b) => ({
        label: b.label,
        title: b.title,
        success: b.point?.success_count ?? 0,
        error: b.point?.error_count ?? 0,
        timeout: b.point?.timeout_count ?? 0,
        total: b.point?.request_count ?? 0,
      })),
    [buckets],
  );
  const isEmpty = data.every((d) => d.total === 0);

  return (
    <StatsCard
      title={t("stats.daily.title")}
      subtitle={hourly ? t("stats.daily.hourlySubtitle") : t("stats.daily.subtitle")}
      isEmpty={isEmpty}
      emptyText={t("stats.daily.empty")}
      loading={loading}
      errorText={errorText}
    >
      <ResponsiveContainer width="100%" height={220}>
        <BarChart data={data} margin={{ top: 8, right: 12, bottom: 0, left: -16 }}>
          <CartesianGrid stroke="var(--line)" vertical={false} />
          <XAxis dataKey="label" tick={{ fontSize: 10, fill: "var(--ink-3)" }} stroke="var(--line-2)" tickLine={false} />
          <YAxis tick={{ fontSize: 10, fill: "var(--ink-3)" }} stroke="var(--line-2)" tickLine={false} allowDecimals={false} />
          <Tooltip
            cursor={{ fill: "var(--surface-2)" }}
            content={({ active, payload }) => {
              const p = active && payload && payload.length > 0 ? (payload[0].payload as Row) : null;
              if (!p) return null;
              return (
                <ChartTooltip
                  title={p.title}
                  rows={[
                    { label: t("stats.daily.tooltipTotal"), value: fmtNum(p.total), strong: true },
                    ...SERIES.map((s) => ({ label: t(s.labelKey), value: fmtNum(p[s.key]), color: s.color })),
                  ]}
                />
              );
            }}
          />
          {SERIES.map((s, i) => (
            <Bar
              key={s.key}
              dataKey={s.key}
              stackId="req"
              fill={s.color}
              stroke="var(--surface)"
              strokeWidth={1}
              maxBarSize={24}
              isAnimationActive={false}
              radius={i === SERIES.length - 1 ? [3, 3, 0, 0] : undefined}
            />
          ))}
        </BarChart>
      </ResponsiveContainer>
      <ChartLegend items={SERIES.map((s) => ({ label: t(s.labelKey), color: s.color }))} />
    </StatsCard>
  );
}
