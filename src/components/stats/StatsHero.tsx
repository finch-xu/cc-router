/**
 * 统计页主卡: 6 个 KPI (带「较上一周期」) + 一张可切换「请求 / Token / 耗时」的趋势图。
 * 涨跌按好坏上色, 不按方向: 耗时变长、失败率升高都是红。
 */
import { useMemo, useState } from "react";
import { Bar, BarChart, CartesianGrid, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";
import { useT } from "@/i18n";
import { fmtCompact, fmtNum } from "@/lib/format";
import type { OverallStatsDto, PeriodTotalsDto, StatsRange } from "@/types";
import { ChartTooltip } from "./ChartTooltip";
import type { SeriesBucket } from "./series";

type Metric = "requests" | "tokens" | "latency";

interface Totals {
  requests: number;
  success: number;
  avg: number | null;
  tokens: number;
  hit: number | null;
  toolShare: number | null;
}

function totalsOf(o: OverallStatsDto | PeriodTotalsDto): Totals {
  const tokens = o.total_input_tokens + o.total_output_tokens + o.total_cache_creation_tokens + o.total_cache_read_tokens;
  const hitBase = o.total_input_tokens + o.total_cache_creation_tokens + o.total_cache_read_tokens;
  return {
    requests: o.total_requests,
    success: o.success_count,
    avg: o.avg_duration_ms ?? null,
    tokens,
    hit: hitBase ? o.total_cache_read_tokens / hitBase : null,
    toolShare: o.success_count ? o.tool_use_request_count / o.success_count : null,
  };
}

/** 相对变化 (%); 上一周期为 0 / 缺失时不比 */
function relDelta(cur: number | null, prev: number | null | undefined): number | null {
  if (cur == null || prev == null || prev === 0) return null;
  return ((cur - prev) / prev) * 100;
}

function Delta({ v, unit, goodUp = true, title }: { v: number | null; unit: "%" | "pp"; goodUp?: boolean; title: string }) {
  if (v == null || !Number.isFinite(v)) return null;
  const flat = Math.abs(v) < (unit === "pp" ? 0.05 : 0.5);
  const good = v > 0 === goodUp;
  const abs = Math.abs(v);
  return (
    <span className={"sx-delta " + (flat ? "flat" : good ? "good" : "bad")} title={title}>
      {flat ? "±" : v > 0 ? "↑" : "↓"}
      {abs.toFixed(unit === "pp" || abs < 10 ? 1 : 0)}
      {unit === "pp" ? "pp" : "%"}
    </span>
  );
}

function Spark({ values }: { values: number[] }) {
  if (values.length < 2) return null;
  const w = 160;
  const h = 26;
  const max = Math.max(...values, 1e-9);
  const pts = values.map((v, i) => [(i / (values.length - 1)) * w, h - 2 - (v / max) * (h - 4)]);
  const line = pts.map((p, i) => `${i ? "L" : "M"}${p[0].toFixed(1)} ${p[1].toFixed(1)}`).join(" ");
  return (
    <svg className="sx-spark" viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" aria-hidden>
      <path className="area" d={`${line} L${w} ${h} L0 ${h} Z`} />
      <path className="line" d={line} />
    </svg>
  );
}

const pct = (x: number | null) => (x == null ? "—" : (x * 100).toFixed(x >= 0.995 || x < 0.1 ? 1 : 0));

export function StatsHero({
  overall,
  buckets,
  range,
  loading,
  errorText,
}: {
  overall: OverallStatsDto | undefined;
  buckets: SeriesBucket[];
  range: StatsRange;
  loading?: boolean;
  errorText?: string | null;
}) {
  const { t } = useT();
  const [metric, setMetric] = useState<Metric>("requests");
  const cur = overall ? totalsOf(overall) : null;
  const prev = overall?.previous ? totalsOf(overall.previous) : null;
  const vs = t(`stats.delta.${range}`);
  const fails = overall ? overall.error_count + overall.timeout_count : 0;
  const pp = (a: number | null | undefined, b: number | null | undefined) => (a == null || b == null ? null : (a - b) * 100);

  const reqSpark = buckets.map((b) => b.point?.request_count ?? 0);
  const tokSpark = buckets.map((b) =>
    b.point
      ? b.point.total_input_tokens + b.point.total_output_tokens + b.point.total_cache_creation_tokens + b.point.total_cache_read_tokens
      : 0,
  );

  return (
    <section className={"card sx-hero" + (loading ? " is-loading" : "")}>
      <div className="sx-kpis">
        <div className="sx-kpi focus">
          <div className="l">
            {t("stats.kpi.totalRequests")}
            <Delta v={relDelta(cur?.requests ?? null, prev?.requests)} unit="%" title={vs} />
          </div>
          <div className="v">{cur ? fmtNum(cur.requests) : "—"}</div>
          <div className="s">{t("stats.kpi.failSub", { n: fmtNum(fails) })}</div>
          <Spark values={reqSpark} />
        </div>
        <div className="sx-kpi">
          <div className="l">
            {t("stats.kpi.successRate")}
            <Delta
              v={pp(cur && cur.requests ? cur.success / cur.requests : null, prev && prev.requests ? prev.success / prev.requests : null)}
              unit="pp"
              title={vs}
            />
          </div>
          <div className="v">
            {cur && cur.requests ? ((cur.success / cur.requests) * 100).toFixed(1) : "—"}
            <small>%</small>
          </div>
          <div className="s">{overall ? t("stats.kpi.failedFormat", { failed: fmtNum(fails), total: fmtNum(overall.total_requests) }) : ""}</div>
        </div>
        <div className="sx-kpi">
          <div className="l">
            {t("stats.kpi.avgDuration")}
            <Delta v={relDelta(cur?.avg ?? null, prev?.avg)} unit="%" goodUp={false} title={vs} />
          </div>
          <div className="v">
            {cur?.avg != null ? (cur.avg / 1000).toFixed(1) : "—"}
            <small>s</small>
          </div>
          <div className="s">
            {overall?.p95_duration_ms != null ? t("stats.kpi.p95", { v: (overall.p95_duration_ms / 1000).toFixed(1) }) : "—"}
          </div>
        </div>
        <div className="sx-kpi">
          <div className="l">
            {t("stats.kpi.totalTokens")}
            <Delta v={relDelta(cur?.tokens ?? null, prev?.tokens)} unit="%" title={vs} />
          </div>
          <div className="v">{cur ? fmtCompact(cur.tokens) : "—"}</div>
          <div className="s">
            {overall
              ? t("stats.kpi.totalTokensSub", {
                  input: fmtCompact(overall.total_input_tokens),
                  output: fmtCompact(overall.total_output_tokens),
                })
              : ""}
          </div>
          <Spark values={tokSpark} />
        </div>
        <div className="sx-kpi" title={t("stats.kpi.cacheReadHint")}>
          <div className="l">
            {t("stats.kpi.cacheHit")}
            <Delta v={pp(cur?.hit, prev?.hit)} unit="pp" title={vs} />
          </div>
          <div className="v">
            {pct(cur?.hit ?? null)}
            <small>%</small>
          </div>
          <div className="s">
            {overall
              ? t("stats.kpi.cacheReadSub", {
                  read: fmtCompact(overall.total_cache_read_tokens),
                  write: fmtCompact(overall.total_cache_creation_tokens),
                })
              : ""}
          </div>
        </div>
        <div className="sx-kpi" title={t("stats.kpi.toolShareHint")}>
          <div className="l">
            {t("stats.kpi.toolShare")}
            <Delta v={pp(cur?.toolShare, prev?.toolShare)} unit="pp" title={vs} />
          </div>
          <div className="v">
            {pct(cur?.toolShare ?? null)}
            <small>%</small>
          </div>
          <div className="s">
            {overall
              ? t("stats.kpi.toolShareSub", {
                  withTools: fmtNum(overall.tool_use_request_count),
                  success: fmtNum(overall.success_count),
                })
              : ""}
          </div>
        </div>
      </div>

      <div className="sx-chart">
        <div className="sx-chart-bar">
          <div className="seg-tabs" role="tablist">
            {(["requests", "tokens", "latency"] as const).map((m) => (
              <button
                key={m}
                type="button"
                role="tab"
                aria-selected={metric === m}
                className={"seg-tab" + (metric === m ? " active" : "")}
                onClick={() => setMetric(m)}
              >
                {t(`stats.metric.${m}`)}
              </button>
            ))}
          </div>
          <span className="sx-chart-note">{t(range === "today" ? "stats.chart.hourly" : "stats.chart.daily")}</span>
          <Legend metric={metric} />
        </div>
        {errorText ? (
          <div className="sx-error">{errorText}</div>
        ) : buckets.every((b) => !b.point || b.point.request_count === 0) ? (
          <div className="sx-empty">{t("stats.daily.empty")}</div>
        ) : (
          <TrendChart buckets={buckets} metric={metric} />
        )}
      </div>
    </section>
  );
}

function Legend({ metric }: { metric: Metric }) {
  const { t } = useT();
  if (metric === "requests")
    return (
      <div className="sx-legend">
        <span><i className="sw" style={{ background: "var(--accent)" }} />{t("stats.daily.tooltipSuccess")}</span>
        <span><i className="sw" style={{ background: "var(--err)" }} />{t("stats.legend.failed")}</span>
      </div>
    );
  if (metric === "tokens")
    return (
      <div className="sx-legend">
        <span><i className="sw cr" />{t("stats.daily.tokenTooltipCacheRead")}</span>
        <span><i className="sw cw" />{t("stats.daily.tokenTooltipCacheCreate")}</span>
        <span><i className="sw in" />{t("stats.daily.tokenTooltipInput")}</span>
        <span><i className="sw out" />{t("stats.daily.tokenTooltipOutput")}</span>
      </div>
    );
  return (
    <div className="sx-legend">
      <span><i className="sw" style={{ background: "var(--sx-lat)" }} />{t("stats.kpi.avgDuration")}</span>
    </div>
  );
}

interface ChartRow {
  label: string;
  title: string;
  ok: number;
  fail: number;
  cr: number;
  cw: number;
  in: number;
  out: number;
  avg: number;
}

const TOKEN_SERIES = [
  { key: "cr", color: "var(--sx-cr)", labelKey: "stats.daily.tokenTooltipCacheRead" },
  { key: "cw", color: "var(--sx-cw)", labelKey: "stats.daily.tokenTooltipCacheCreate" },
  { key: "in", color: "var(--sx-in)", labelKey: "stats.daily.tokenTooltipInput" },
  { key: "out", color: "var(--sx-out)", labelKey: "stats.daily.tokenTooltipOutput" },
] as const;

function TrendChart({ buckets, metric }: { buckets: SeriesBucket[]; metric: Metric }) {
  const { t } = useT();
  const data: ChartRow[] = useMemo(
    () =>
      buckets.map((b) => {
        const p = b.point;
        return {
          label: b.label,
          title: b.title,
          ok: p?.success_count ?? 0,
          fail: p ? p.error_count + p.timeout_count : 0,
          cr: p?.total_cache_read_tokens ?? 0,
          cw: p?.total_cache_creation_tokens ?? 0,
          in: p?.total_input_tokens ?? 0,
          out: p?.total_output_tokens ?? 0,
          avg: p?.avg_duration_ms != null ? p.avg_duration_ms / 1000 : 0,
        };
      }),
    [buckets],
  );
  const tick = { fontSize: 10, fill: "var(--ink-4)" };
  const yFmt = metric === "tokens" ? (v: number) => fmtCompact(v) : metric === "latency" ? (v: number) => `${v}s` : undefined;

  return (
    <ResponsiveContainer width="100%" height={220}>
      <BarChart data={data} margin={{ top: 8, right: 4, bottom: 0, left: -12 }}>
        <CartesianGrid stroke="var(--line)" vertical={false} />
        <XAxis dataKey="label" tick={tick} stroke="var(--line)" tickLine={false} minTickGap={12} />
        <YAxis tick={tick} stroke="transparent" tickLine={false} allowDecimals={metric === "latency"} tickFormatter={yFmt} width={52} />
        <Tooltip
          cursor={{ fill: "var(--surface-2)" }}
          content={({ active, payload }) => {
            const p = active && payload && payload.length > 0 ? (payload[0].payload as ChartRow) : null;
            if (!p) return null;
            const rows =
              metric === "requests"
                ? [
                    { label: t("stats.daily.tooltipTotal"), value: fmtNum(p.ok + p.fail), strong: true },
                    { label: t("stats.daily.tooltipSuccess"), value: fmtNum(p.ok), color: "var(--accent)" },
                    { label: t("stats.legend.failed"), value: fmtNum(p.fail), color: "var(--err)" },
                  ]
                : metric === "tokens"
                  ? [
                      { label: t("stats.daily.tooltipTotal"), value: fmtCompact(p.cr + p.cw + p.in + p.out), strong: true },
                      ...[...TOKEN_SERIES].reverse().map((s) => ({ label: t(s.labelKey), value: fmtCompact(p[s.key]), color: s.color })),
                    ]
                  : [{ label: t("stats.kpi.avgDuration"), value: p.avg ? `${p.avg.toFixed(2)}s` : "—", color: "var(--sx-lat)" }];
            return <ChartTooltip title={p.title} rows={rows} />;
          }}
        />
        {metric === "requests" && (
          <>
            <Bar dataKey="ok" stackId="r" fill="var(--accent)" maxBarSize={32} isAnimationActive={false} />
            <Bar dataKey="fail" stackId="r" fill="var(--err)" maxBarSize={32} isAnimationActive={false} radius={[3, 3, 0, 0]} />
          </>
        )}
        {metric === "tokens" &&
          TOKEN_SERIES.map((s, i) => (
            <Bar
              key={s.key}
              dataKey={s.key}
              stackId="t"
              fill={s.color}
              maxBarSize={32}
              isAnimationActive={false}
              radius={i === TOKEN_SERIES.length - 1 ? [3, 3, 0, 0] : undefined}
            />
          ))}
        {metric === "latency" && (
          <Bar dataKey="avg" fill="var(--sx-lat)" maxBarSize={32} isAnimationActive={false} radius={[3, 3, 0, 0]} />
        )}
      </BarChart>
    </ResponsiveContainer>
  );
}
