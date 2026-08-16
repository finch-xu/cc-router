import { useMemo } from "react";
import { Bar, BarChart, CartesianGrid, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";
import { useT } from "@/i18n";
import { fmtCompact, fmtNum } from "@/lib/format";
import { ChartLegend, ChartTooltip } from "./ChartTooltip";
import { StatsCard } from "./StatsCard";
import type { SeriesBucket } from "./series";

interface Row {
  label: string;
  title: string;
  cache_read: number;
  cache_creation: number;
  input: number;
  output: number;
  total: number;
}

// 铁锈橙顺序色阶: 先写的在下层, 由浅到深 = 缓存读 (量最大、最便宜) → 输出 (最贵)
const SEGMENTS = [
  { key: "cache_read", color: "var(--seq-1)", labelKey: "stats.daily.tokenTooltipCacheRead" },
  { key: "cache_creation", color: "var(--seq-2)", labelKey: "stats.daily.tokenTooltipCacheCreate" },
  { key: "input", color: "var(--seq-3)", labelKey: "stats.daily.tokenTooltipInput" },
  { key: "output", color: "var(--seq-4)", labelKey: "stats.daily.tokenTooltipOutput" },
] as const;

export function DailyTokensChart({
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
      buckets.map((b) => {
        const cache_read = b.point?.total_cache_read_tokens ?? 0;
        const cache_creation = b.point?.total_cache_creation_tokens ?? 0;
        const input = b.point?.total_input_tokens ?? 0;
        const output = b.point?.total_output_tokens ?? 0;
        return {
          label: b.label,
          title: b.title,
          cache_read,
          cache_creation,
          input,
          output,
          total: cache_read + cache_creation + input + output,
        };
      }),
    [buckets],
  );
  const isEmpty = data.every((d) => d.total === 0);

  return (
    <StatsCard
      title={t("stats.daily.tokenTitle")}
      subtitle={hourly ? t("stats.daily.tokenHourlySubtitle") : t("stats.daily.tokenSubtitle")}
      isEmpty={isEmpty}
      emptyText={t("stats.daily.empty")}
      loading={loading}
      errorText={errorText}
    >
      <ResponsiveContainer width="100%" height={220}>
        <BarChart data={data} margin={{ top: 8, right: 12, bottom: 0, left: -8 }}>
          <CartesianGrid stroke="var(--line)" vertical={false} />
          <XAxis dataKey="label" tick={{ fontSize: 10, fill: "var(--ink-3)" }} stroke="var(--line-2)" tickLine={false} />
          <YAxis
            tick={{ fontSize: 10, fill: "var(--ink-3)" }}
            stroke="var(--line-2)"
            tickLine={false}
            allowDecimals={false}
            tickFormatter={(v: number) => fmtCompact(v)}
            width={48}
          />
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
                    ...[...SEGMENTS].reverse().map((s) => ({ label: t(s.labelKey), value: fmtNum(p[s.key]), color: s.color })),
                  ]}
                />
              );
            }}
          />
          {SEGMENTS.map((s, i) => (
            <Bar
              key={s.key}
              dataKey={s.key}
              stackId="tok"
              fill={s.color}
              stroke="var(--surface)"
              strokeWidth={1}
              maxBarSize={24}
              isAnimationActive={false}
              radius={i === SEGMENTS.length - 1 ? [3, 3, 0, 0] : undefined}
            />
          ))}
        </BarChart>
      </ResponsiveContainer>
      <ChartLegend items={[...SEGMENTS].reverse().map((s) => ({ label: t(s.labelKey), color: s.color }))} />
    </StatsCard>
  );
}
