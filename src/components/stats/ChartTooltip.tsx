/**
 * 统一的图表 tooltip 外观: 首行标题 (日期/时刻/类别), 之后每系列一行「色键 + 标签 + mono 数值」。
 * 数值是强调元素, 标签次之; 色键只是一小段系列色, 文字永远用 --ink-* (不穿系列色)。
 */
export interface TooltipRow {
  label: string;
  value: string;
  color?: string;
  strong?: boolean;
}

export function ChartTooltip({ title, rows }: { title: string; rows: TooltipRow[] }) {
  return (
    <div className="stats-tooltip">
      <div className="tt-title">{title}</div>
      {rows.map((r, i) => (
        <div key={i} className={"tt-row" + (r.strong ? " strong" : "")}>
          <span className="tt-label">
            {r.color && <span className="tt-key" style={{ background: r.color }} />}
            {r.label}
          </span>
          <span className="tt-val">{r.value}</span>
        </div>
      ))}
    </div>
  );
}

/** 图下图例: 色块 + 标签, ≥2 系列必有。 */
export function ChartLegend({
  items,
  kind = "rect",
}: {
  items: { label: string; color: string }[];
  kind?: "rect" | "line";
}) {
  return (
    <div className="chart-legend">
      {items.map((it) => (
        <span key={it.label} className="chart-legend-item">
          <span className={"chart-legend-swatch " + kind} style={{ background: it.color }} />
          {it.label}
        </span>
      ))}
    </div>
  );
}
