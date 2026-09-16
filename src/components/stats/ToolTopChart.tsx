import { useMemo } from "react";
import { Bar, BarChart, Cell, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";
import { useT } from "@/i18n";
import { fmtNum } from "@/lib/format";
import { CLIENT_TOOLS_BY_ID } from "@/lib/clientTools";
import type { ClientToolId, ToolBreakdownDto } from "@/types";
import { ChartTooltip } from "./ChartTooltip";
import { StatsCard } from "./StatsCard";

interface Row {
  key: string;
  /** 展示名: mcp__server__tool → "server / tool" */
  label: string;
  fullName: string;
  clientLabel: string;
  call_count: number;
}

/** `mcp__<server>__<tool>` → `<server> / <tool>`; 其他原样 */
export function displayToolName(name: string): string {
  const m = /^mcp__([^_]+(?:_[^_]+)*)__(.+)$/.exec(name);
  return m ? `${m[1]} / ${m[2]}` : name || "(unnamed)";
}

/** 工具调用 Top N 横条。同名工具不同客户端分条 (工具由客户端定义, CC 的 Bash ≠ Codex 的 shell)。 */
export function ToolTopChart({
  items,
  loading,
  errorText,
}: {
  items: ToolBreakdownDto[];
  loading?: boolean;
  errorText?: string | null;
}) {
  const { t } = useT();
  const rows: Row[] = useMemo(
    () =>
      items.map((it) => {
        const meta = it.client_tool ? CLIENT_TOOLS_BY_ID[it.client_tool as ClientToolId] : undefined;
        const clientLabel = meta ? t(meta.i18nKey) : t("requestLogs.client.unknown");
        return {
          key: `${it.client_tool ?? "__unknown__"}::${it.tool_name}`,
          label: displayToolName(it.tool_name),
          fullName: it.tool_name,
          clientLabel,
          call_count: it.call_count,
        };
      }),
    [items, t],
  );
  const height = Math.max(160, rows.length * 28 + 24);

  return (
    <StatsCard
      title={t("stats.tools.title")}
      subtitle={t("stats.tools.subtitle")}
      isEmpty={rows.length === 0}
      emptyText={t("stats.tools.empty")}
      loading={loading}
      errorText={errorText}
    >
      <div style={{ width: "100%", height }}>
        <ResponsiveContainer>
          <BarChart data={rows} layout="vertical" margin={{ top: 4, right: 48, bottom: 4, left: 4 }}>
            <XAxis type="number" hide />
            <YAxis
              type="category"
              dataKey="label"
              width={180}
              tick={{ fontSize: 11, fill: "var(--ink-2)" }}
              tickLine={false}
              axisLine={false}
              interval={0}
            />
            <Tooltip
              cursor={{ fill: "var(--surface-2)" }}
              content={({ active, payload }) => {
                if (!active || !payload?.length) return null;
                const r = payload[0].payload as Row;
                return (
                  <ChartTooltip
                    title={r.fullName}
                    rows={[
                      { label: r.clientLabel, value: "" },
                      { label: t("stats.tools.tooltipCalls"), value: fmtNum(r.call_count), strong: true },
                    ]}
                  />
                );
              }}
            />
            <Bar
              dataKey="call_count"
              radius={[0, 4, 4, 0]}
              label={{ position: "right", fontSize: 11, fill: "var(--ink-3)", formatter: (v) => fmtNum(Number(v)) }}
            >
              {rows.map((r) => (
                <Cell key={r.key} fill="var(--vm-sonnet)" />
              ))}
            </Bar>
          </BarChart>
        </ResponsiveContainer>
      </div>
    </StatsCard>
  );
}
