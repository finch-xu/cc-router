import { useMemo, useState } from "react";
import { Cell, Pie, PieChart, ResponsiveContainer, Tooltip } from "recharts";
import { useT } from "@/i18n";
import { fmtCompact, fmtNum } from "@/lib/format";
import { VM_META, VM_ORDER } from "@/lib/virtualModels";
import type { BreakdownDto, VirtualModelName } from "@/types";
import { ChartTooltip } from "./ChartTooltip";
import { StatsCard } from "./StatsCard";

/** 每个虚拟模型的固定分类色 (styles.css `--vm-*`), 颜色跟随实体, 不随过滤/排序重排 */
export const VM_COLOR: Record<VirtualModelName, string> = {
  "model-fable": "var(--vm-fable)",
  "model-opus": "var(--vm-opus)",
  "model-sonnet": "var(--vm-sonnet)",
  "model-haiku": "var(--vm-haiku)",
  "model-fallback": "var(--vm-fallback)",
};

type Metric = "requests" | "tokens";

interface Slice {
  name: VirtualModelName;
  label: string;
  color: string;
  value: number;
  requests: number;
  tokens: number;
  successPct: number;
}

/**
 * 虚拟模型份额环图 (≤5 段) + 右侧图例表 (名称 / 数值 / 占比 / 成功率)。
 * 环图只有一个度量, 右上角切换「请求数 / Token」; Token = input + output (不含缓存, 与 KPI 口径一致)。
 */
export function VmShareDonut({
  items,
  loading,
  errorText,
}: {
  items: BreakdownDto[];
  loading?: boolean;
  errorText?: string | null;
}) {
  const { t } = useT();
  const [metric, setMetric] = useState<Metric>("requests");
  const [activeKey, setActiveKey] = useState<VirtualModelName | null>(null);

  const slices: Slice[] = useMemo(() => {
    const byKey = new Map(items.map((it) => [it.key, it]));
    // 按 VM_ORDER 固定顺序排好再交给 Pie (recharts 不承诺扇区顺序), 0 值过滤
    return VM_ORDER.flatMap((name) => {
      const it = byKey.get(name);
      if (!it) return [];
      const tokens = it.total_input_tokens + it.total_output_tokens;
      const value = metric === "requests" ? it.request_count : tokens;
      if (value <= 0) return [];
      return [
        {
          name,
          label: t(VM_META[name].labelKey),
          color: VM_COLOR[name],
          value,
          requests: it.request_count,
          tokens,
          successPct: it.request_count > 0 ? (it.success_count / it.request_count) * 100 : 0,
        },
      ];
    });
  }, [items, metric, t]);

  const total = slices.reduce((s, x) => s + x.value, 0);
  const fmtVal = (v: number) => (metric === "requests" ? fmtNum(v) : fmtCompact(v));

  return (
    <StatsCard
      title={t("stats.byVm.title")}
      subtitle={t("stats.byVm.subtitle")}
      isEmpty={slices.length === 0}
      emptyText={t("stats.byVm.empty")}
      loading={loading}
      errorText={errorText}
      right={
        <div className="seg-tabs" role="tablist">
          {(["requests", "tokens"] as Metric[]).map((m) => (
            <button
              key={m}
              type="button"
              role="tab"
              aria-selected={metric === m}
              className={"seg-tab" + (metric === m ? " active" : "")}
              onClick={() => setMetric(m)}
            >
              {m === "requests" ? t("stats.byVm.metricRequests") : t("stats.byVm.metricTokens")}
            </button>
          ))}
        </div>
      }
    >
      <div className="donut-wrap">
        <div className="donut-chart">
          <ResponsiveContainer width="100%" height={200}>
            <PieChart>
              <Pie
                data={slices}
                dataKey="value"
                nameKey="label"
                innerRadius="62%"
                outerRadius="92%"
                paddingAngle={slices.length > 1 ? 2 : 0}
                stroke="none"
                isAnimationActive={false}
                onMouseEnter={(_: unknown, index: number) => setActiveKey(slices[index]?.name ?? null)}
                onMouseLeave={() => setActiveKey(null)}
              >
                {slices.map((s) => (
                  <Cell
                    key={s.name}
                    fill={s.color}
                    opacity={activeKey && activeKey !== s.name ? 0.35 : 1}
                    style={{ outline: "none" }}
                  />
                ))}
              </Pie>
              <Tooltip
                content={({ active, payload }) => {
                  const s = active && payload && payload.length > 0 ? (payload[0].payload as Slice) : null;
                  if (!s) return null;
                  return (
                    <ChartTooltip
                      title={`${s.name} · ${s.label}`}
                      rows={[
                        { label: metric === "requests" ? t("stats.byVm.metricRequests") : t("stats.byVm.metricTokens"), value: fmtVal(s.value), color: s.color, strong: true },
                        { label: t("stats.byVm.col.share"), value: total > 0 ? `${((s.value / total) * 100).toFixed(1)}%` : "-" },
                        { label: t("stats.byVm.col.success"), value: `${s.successPct.toFixed(1)}%` },
                      ]}
                    />
                  );
                }}
              />
            </PieChart>
          </ResponsiveContainer>
          <div className="donut-center">
            <div className="donut-center-val tnum">{fmtVal(total)}</div>
            <div className="donut-center-label">
              {metric === "requests" ? t("stats.byVm.centerRequests") : t("stats.byVm.centerTokens")}
            </div>
          </div>
        </div>
        <table className="donut-legend">
          <tbody>
            {slices.map((s) => (
              <tr
                key={s.name}
                className={activeKey && activeKey !== s.name ? "dim" : undefined}
                onMouseEnter={() => setActiveKey(s.name)}
                onMouseLeave={() => setActiveKey(null)}
              >
                <td title={s.label}>
                  <span className="donut-legend-dot" style={{ background: s.color }} />
                  <span className="mono">{s.name}</span>
                </td>
                <td className="mono tnum strong">{fmtVal(s.value)}</td>
                <td className="mono tnum muted">{total > 0 ? `${((s.value / total) * 100).toFixed(1)}%` : "-"}</td>
                <td className="mono tnum muted">{s.successPct.toFixed(0)}% ✓</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </StatsCard>
  );
}
