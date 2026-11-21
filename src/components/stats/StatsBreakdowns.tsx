/**
 * 统计页主卡下面的几组分布: Token 去了哪里 / 请求用途 / 订阅表现 / 常用工具 / 耗时分布。
 * 每一行都能点进请求日志 (带同口径的时间范围与筛选), 与实时路由 → 请求日志的跳转一致。
 */
import { useMemo, useState, type ReactNode } from "react";
import { Link } from "react-router";
import { ChevronRight, Info } from "lucide-react";
import { ProviderLogo } from "@/components/ProviderLogo";
import { useT } from "@/i18n";
import { CLIENT_TOOLS_BY_ID, UNKNOWN_CLIENT_ICON } from "@/lib/clientTools";
import { fmtCompact, fmtNum } from "@/lib/format";
import { PURPOSE_META, PURPOSE_ORDER } from "@/lib/liveStats";
import type { LogRange, PurposeChip } from "@/lib/requestLogQuery";
import type {
  BreakdownDto,
  ClientToolId,
  OverallStatsDto,
  RequestMixDto,
  StatsRange,
  SubscriptionDto,
  ToolBreakdownDto,
} from "@/types";

/** 统计范围 → 请求日志范围 (日志页没有 90 天, 退到全部) */
const LOG_RANGE: Record<StatsRange, LogRange> = {
  today: "today",
  last7_days: "7d",
  last30_days: "30d",
  last90_days: "all",
  all_time: "all",
};

export function logsHref(range: StatsRange, extra: Record<string, string>): string {
  const sp = new URLSearchParams();
  const r = LOG_RANGE[range];
  if (r !== "all") sp.set("range", r);
  for (const [k, v] of Object.entries(extra)) sp.set(k, v);
  const s = sp.toString();
  return `/request-logs${s ? `?${s}` : ""}`;
}

const tokensOf = (b: BreakdownDto) =>
  b.total_input_tokens + b.total_output_tokens + b.total_cache_creation_tokens + b.total_cache_read_tokens;

function CardShell({
  title,
  sub,
  end,
  loading,
  errorText,
  className,
  children,
}: {
  title: string;
  sub?: string;
  end?: ReactNode;
  loading?: boolean;
  errorText?: string | null;
  className?: string;
  children: ReactNode;
}) {
  return (
    <section className={"card" + (loading ? " is-loading" : "") + (className ? ` ${className}` : "")}>
      <div className="card-head">
        <div className="sx-head-lead">
          <span className="card-title">{title}</span>
          {sub && <span className="card-sub">{sub}</span>}
        </div>
        {end && <div className="sx-head-end">{end}</div>}
      </div>
      <div className="card-body">{errorText ? <div className="sx-error">{errorText}</div> : children}</div>
    </section>
  );
}

function Seg<T extends string>({ value, options, onChange }: { value: T; options: { key: T; label: string }[]; onChange: (v: T) => void }) {
  return (
    <div className="seg-tabs" role="tablist">
      {options.map((o) => (
        <button
          key={o.key}
          type="button"
          role="tab"
          aria-selected={value === o.key}
          className={"seg-tab" + (value === o.key ? " active" : "")}
          onClick={() => onChange(o.key)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

/* ---------------- Token 去了哪里 ---------------- */

type TokenBy = "vm" | "sub" | "client";

export function TokenFlowCard({
  overall,
  byVm,
  bySub,
  mix,
  subs,
  range,
  rangeLabel,
  loading,
  errorText,
}: {
  overall: OverallStatsDto | undefined;
  byVm: BreakdownDto[];
  bySub: BreakdownDto[];
  mix: RequestMixDto | undefined;
  subs: SubscriptionDto[];
  range: StatsRange;
  rangeLabel: string;
  loading?: boolean;
  errorText?: string | null;
}) {
  const { t } = useT();
  const [by, setBy] = useState<TokenBy>("vm");
  const o = overall;
  const parts = o
    ? [
        { cls: "cr", label: t("stats.daily.tokenTooltipCacheRead"), v: o.total_cache_read_tokens },
        { cls: "cw", label: t("stats.daily.tokenTooltipCacheCreate"), v: o.total_cache_creation_tokens },
        { cls: "in", label: t("stats.daily.tokenTooltipInput"), v: o.total_input_tokens },
        { cls: "out", label: t("stats.daily.tokenTooltipOutput"), v: o.total_output_tokens },
      ]
    : [];
  const total = parts.reduce((s, p) => s + p.v, 0);
  const hitBase = o ? o.total_input_tokens + o.total_cache_creation_tokens + o.total_cache_read_tokens : 0;
  const hit = o && hitBase ? o.total_cache_read_tokens / hitBase : null;
  const C = 2 * Math.PI * 17;
  const icon = new Map(subs.map((s) => [s.id, s.provider_icon]));

  const rows = useMemo(() => {
    if (by === "vm")
      return byVm.map((b) => ({
        key: b.key,
        name: <span className="mono">{b.label}</span>,
        v: tokensOf(b),
        href: logsHref(range, { vm: b.key }),
      }));
    if (by === "sub")
      return bySub.map((b) => ({
        key: b.key,
        name: (
          <>
            <ProviderLogo iconId={icon.get(b.key)} size={16} />
            <span>{b.label}</span>
          </>
        ),
        v: tokensOf(b),
        href: logsHref(range, { subscription_id: b.key }),
      }));
    return (mix?.clients ?? []).map((c) => {
      const meta = c.client_tool ? CLIENT_TOOLS_BY_ID[c.client_tool as ClientToolId] : undefined;
      const Icon = meta?.icon ?? UNKNOWN_CLIENT_ICON;
      return {
        key: c.client_tool ?? "__unknown__",
        name: (
          <>
            <Icon size={13} />
            <span>{meta ? t(meta.i18nKey) : t("requestLogs.client.unknown")}</span>
          </>
        ),
        v: c.tokens,
        href: logsHref(range, { client: c.client_tool ?? "__unknown__" }),
      };
    });
  }, [by, byVm, bySub, mix, range, subs, t]);
  const sum = rows.reduce((s, r) => s + r.v, 0);
  const max = Math.max(1, ...rows.map((r) => r.v));

  return (
    <CardShell title={t("stats.tk.title")} sub={rangeLabel} loading={loading} errorText={errorText}>
      {total === 0 ? (
        <div className="sx-empty">{t("stats.daily.empty")}</div>
      ) : (
        <>
          <div className="sx-tk-top">
            <div className="sx-tk-total">
              <div className="v">{fmtCompact(total)}</div>
              <div className="s">
                {o && o.total_requests ? t("stats.tk.perRequest", { v: fmtCompact(Math.round(total / o.total_requests)) }) : ""}
              </div>
            </div>
            <div className="sx-hit" title={t("stats.kpi.cacheReadHint")}>
              <svg viewBox="0 0 42 42" aria-hidden>
                <circle className="bg" cx="21" cy="21" r="17" />
                <circle className="fg" cx="21" cy="21" r="17" strokeDasharray={`${(C * (hit ?? 0)).toFixed(1)} ${C.toFixed(1)}`} />
              </svg>
              <div>
                <b>{hit == null ? "—" : `${Math.round(hit * 100)}%`}</b>
                <small>{t("stats.kpi.cacheHit")}</small>
              </div>
            </div>
          </div>
          <div className="sx-stack">
            {parts.map((p) => (
              <i key={p.cls} className={p.cls} style={{ width: `${(p.v / total) * 100}%` }} title={`${p.label} ${fmtCompact(p.v)}`} />
            ))}
          </div>
          <div className="sx-parts">
            {parts.map((p) => (
              <div key={p.cls} className="sx-part">
                <div className="k">
                  <i className={`sw ${p.cls}`} />
                  {p.label}
                </div>
                <b>{fmtCompact(p.v)}</b>
                <small>{Math.round((p.v / total) * 100)}%</small>
              </div>
            ))}
          </div>
          <div className="sx-switch">
            {t("stats.tk.by")}
            <Seg
              value={by}
              onChange={setBy}
              options={[
                { key: "vm", label: t("stats.tk.byVm") },
                { key: "sub", label: t("stats.tk.bySub") },
                { key: "client", label: t("stats.tk.byClient") },
              ]}
            />
          </div>
          <div className="sx-rows">
            {rows.length === 0 && <div className="sx-empty">{t("stats.byVm.empty")}</div>}
            {rows.map((r, i) => (
              <Link key={r.key} className="sx-row" to={r.href}>
                <span className="n">{r.name}</span>
                <div className="sx-one">
                  <i className={i > 3 ? "mute" : undefined} style={{ width: `${(r.v / max) * 100}%` }} />
                </div>
                <span className="v">{fmtCompact(r.v)}</span>
                <span className="p">{sum ? `${Math.round((r.v / sum) * 100)}%` : "—"}</span>
              </Link>
            ))}
          </div>
          {by === "client" && (
            <div className="sx-note">
              <Info size={12} />
              <span>{t("stats.tk.clientNote")}</span>
            </div>
          )}
        </>
      )}
    </CardShell>
  );
}

/* ---------------- 请求用途 ---------------- */

const PURPOSE_CHIP: Partial<Record<string, PurposeChip>> = {
  main: "main",
  agent: "agent",
  compact: "bg",
  guard: "bg",
  title: "bg",
  fetch: "bg",
  probe: "bg",
  background: "bg",
};

/** 原始表覆盖了多少聚合表里的请求; 两边差得不多时不提示 */
function uncoveredShare(overall: OverallStatsDto | undefined, mix: RequestMixDto | undefined): number {
  if (!overall || !mix || overall.total_requests === 0) return 0;
  const miss = 1 - mix.covered_requests / overall.total_requests;
  return miss > 0.02 ? miss : 0;
}

export function PurposeCard({
  mix,
  overall,
  range,
  loading,
  errorText,
}: {
  mix: RequestMixDto | undefined;
  overall: OverallStatsDto | undefined;
  range: StatsRange;
  loading?: boolean;
  errorText?: string | null;
}) {
  const { t } = useT();
  const rows = useMemo(() => {
    const list = mix?.purposes ?? [];
    const byKind = new Map(list.map((p) => [p.kind ?? "__none__", p]));
    const ordered = [...PURPOSE_ORDER.map((k) => byKind.get(k)), byKind.get("__none__")].filter(
      (p): p is NonNullable<typeof p> => !!p && p.count > 0,
    );
    return ordered;
  }, [mix]);
  const total = mix?.covered_requests ?? 0;
  const max = Math.max(1, ...rows.map((r) => r.count));
  const miss = uncoveredShare(overall, mix);

  return (
    <CardShell title={t("stats.purpose.title")} sub={t("stats.purpose.sub")} loading={loading} errorText={errorText}>
      {rows.length === 0 ? (
        <div className="sx-empty">{t("stats.daily.empty")}</div>
      ) : (
        <div className="sx-rows flush">
          {rows.map((p) => {
            const kind = p.kind;
            const meta = kind ? PURPOSE_META[kind] : null;
            const Icon = meta?.icon;
            const chip = kind ? PURPOSE_CHIP[kind] : undefined;
            const content = (
              <>
                <span className="n">
                  <span className={"sx-ptag " + (meta?.tone ?? "unknown")}>
                    {Icon && <Icon size={11} />}
                    {kind ? t(`live.purpose.${kind}`) : t("live.purpose.unrecorded")}
                  </span>
                  {p.guessed > 0 && p.guessed >= p.count / 2 && <span className="gs">{t("stats.purpose.guessed")}</span>}
                </span>
                <div className="sx-one">
                  <i className={!kind ? "none" : kind === "main" ? undefined : "mute"} style={{ width: `${(p.count / max) * 100}%` }} />
                </div>
                <span className="v">{fmtNum(p.count)}</span>
                <span className="p">{total ? `${Math.round((p.count / total) * 100)}%` : "—"}</span>
              </>
            );
            return chip ? (
              <Link key={kind ?? "none"} className="sx-row" to={logsHref(range, { purpose: chip })}>
                {content}
              </Link>
            ) : (
              <div key={kind ?? "none"} className="sx-row">
                {content}
              </div>
            );
          })}
        </div>
      )}
      <div className="sx-note">
        <Info size={12} />
        <span>
          {miss > 0 ? t("stats.mix.partial", { pct: Math.round(miss * 100) }) : t("stats.mix.full")}{" "}
          <span className="guess">{t("stats.purpose.guessed")}</span> = {t("stats.purpose.guessedHint")}
        </span>
      </div>
    </CardShell>
  );
}

/* ---------------- 订阅表现 ---------------- */

type PerfSort = "requests" | "tokens" | "failures";

export function SubscriptionPerformance({
  items,
  subs,
  range,
  loading,
  errorText,
}: {
  items: BreakdownDto[];
  subs: SubscriptionDto[];
  range: StatsRange;
  loading?: boolean;
  errorText?: string | null;
}) {
  const { t } = useT();
  const [sort, setSort] = useState<PerfSort>("requests");
  const icon = new Map(subs.map((s) => [s.id, s.provider_icon]));
  const fails = (b: BreakdownDto) => b.error_count + b.timeout_count;
  const rows = [...items].sort((a, b) =>
    sort === "tokens" ? tokensOf(b) - tokensOf(a) : sort === "failures" ? fails(b) - fails(a) : b.request_count - a.request_count,
  );
  const val = (b: BreakdownDto) => (sort === "tokens" ? tokensOf(b) : b.request_count);
  const max = Math.max(1, ...rows.map(val));

  return (
    <CardShell
      title={t("stats.perf.title")}
      sub={t("stats.perf.sub")}
      loading={loading}
      errorText={errorText}
      end={
        <Seg
          value={sort}
          onChange={setSort}
          options={[
            { key: "requests", label: t("stats.perf.byRequests") },
            { key: "tokens", label: t("stats.perf.byTokens") },
            { key: "failures", label: t("stats.perf.byFailures") },
          ]}
        />
      }
    >
      {rows.length === 0 ? (
        <div className="sx-empty">{t("stats.bySub.empty")}</div>
      ) : (
        <>
          <div className="sx-perf-head">
            <span>{t("stats.bySub.col.subscription")}</span>
            <span>{sort === "tokens" ? t("stats.perf.tokenShare") : t("stats.perf.requestShare")}</span>
            <span className="r">{t("stats.bySub.col.successRate")}</span>
            <span className="r">{t("stats.bySub.col.avgDuration")}</span>
            <span className="r">Token</span>
            <span className="r">{t("stats.bySub.col.toolCalls")}</span>
            <span />
          </div>
          {rows.map((b) => {
            const rate = b.request_count ? b.success_count / b.request_count : null;
            return (
              <Link key={b.key} className="sx-perf" to={logsHref(range, { subscription_id: b.key })}>
                <span className="nm">
                  <ProviderLogo iconId={icon.get(b.key)} size={18} />
                  <b>{b.label}</b>
                </span>
                <span className="share">
                  <span className="sx-one">
                    <i className={rate != null && rate < 0.95 ? "mute" : undefined} style={{ width: `${(val(b) / max) * 100}%` }} />
                  </span>
                  <span>{sort === "tokens" ? fmtCompact(tokensOf(b)) : fmtNum(b.request_count)}</span>
                </span>
                <span className={"r" + (rate != null && rate < 0.95 ? " bad" : "")}>
                  {rate == null ? "—" : `${(rate * 100).toFixed(1)}%`}
                </span>
                <span className="r hide-sm">{b.avg_duration_ms != null ? `${(b.avg_duration_ms / 1000).toFixed(1)}s` : "—"}</span>
                <span className="r hide-sm">{fmtCompact(tokensOf(b))}</span>
                <span className="r hide-sm">{b.tool_use_count ? fmtNum(b.tool_use_count) : "—"}</span>
                <span className="go">
                  <ChevronRight size={14} />
                </span>
              </Link>
            );
          })}
        </>
      )}
    </CardShell>
  );
}

/* ---------------- 常用工具 ---------------- */

/** `mcp__<server>__<tool>` → `<server> / <tool>`; 其他原样 */
export function displayToolName(name: string): string {
  const m = /^mcp__([^_]+(?:_[^_]+)*)__(.+)$/.exec(name);
  return m ? `${m[1]} / ${m[2]}` : name || "(unnamed)";
}

export function ToolTopCard({
  items,
  range,
  loading,
  errorText,
}: {
  items: ToolBreakdownDto[];
  range: StatsRange;
  loading?: boolean;
  errorText?: string | null;
}) {
  const { t } = useT();
  const nameCounts = new Map<string, number>();
  for (const it of items) nameCounts.set(it.tool_name, (nameCounts.get(it.tool_name) ?? 0) + 1);
  const max = Math.max(1, ...items.map((i) => i.call_count));
  return (
    <CardShell title={t("stats.tools.title")} sub={t("stats.tools.sub")} loading={loading} errorText={errorText}>
      {items.length === 0 ? (
        <div className="sx-empty">{t("stats.tools.empty")}</div>
      ) : (
        <div className="sx-rows flush">
          {items.map((it, i) => {
            const meta = it.client_tool ? CLIENT_TOOLS_BY_ID[it.client_tool as ClientToolId] : undefined;
            const client = meta ? t(meta.i18nKey) : t("requestLogs.client.unknown");
            const name = displayToolName(it.tool_name);
            // 同名工具跨多个客户端 (CC 的 Bash ≠ Codex 的 shell) 时带上客户端消歧
            const label = (nameCounts.get(it.tool_name) ?? 0) > 1 ? `${name} · ${client}` : name;
            return (
              <Link
                key={`${it.client_tool ?? "?"}::${it.tool_name}`}
                className="sx-row"
                style={{ gridTemplateColumns: "minmax(80px, 150px) minmax(0, 1fr) 64px" }}
                to={logsHref(range, { q: it.tool_name })}
                title={`${it.tool_name} · ${client}`}
              >
                <span className="n mono">
                  <span>{label}</span>
                </span>
                <div className="sx-one">
                  <i className={i > 2 ? "mute" : undefined} style={{ width: `${(it.call_count / max) * 100}%` }} />
                </div>
                <span className="v">{fmtNum(it.call_count)}</span>
              </Link>
            );
          })}
        </div>
      )}
    </CardShell>
  );
}

/* ---------------- 耗时分布 ---------------- */

export function LatencyCard({
  mix,
  overall,
  loading,
  errorText,
}: {
  mix: RequestMixDto | undefined;
  overall: OverallStatsDto | undefined;
  loading?: boolean;
  errorText?: string | null;
}) {
  const { t } = useT();
  const b = mix?.latency_buckets ?? [];
  const sum = b.reduce((s, x) => s + x, 0);
  const max = Math.max(1, ...b);
  const miss = uncoveredShare(overall, mix);
  return (
    <CardShell title={t("stats.lat.title")} sub={t("stats.lat.sub")} loading={loading} errorText={errorText}>
      {sum === 0 ? (
        <div className="sx-empty">{t("stats.latency.empty")}</div>
      ) : (
        <div className="sx-rows flush">
          {b.map((n, i) => (
            <div key={i} className="sx-row" style={{ gridTemplateColumns: "86px minmax(0, 1fr) 64px 40px" }}>
              <span className="n">{t(`stats.lat.b${i}`)}</span>
              <div className="sx-one">
                <i className={i >= 4 ? "warn" : undefined} style={{ width: `${(n / max) * 100}%` }} />
              </div>
              <span className="v">{fmtNum(n)}</span>
              <span className="p">{Math.round((n / sum) * 100)}%</span>
            </div>
          ))}
        </div>
      )}
      {miss > 0 && (
        <div className="sx-note">
          <Info size={12} />
          <span>{t("stats.mix.partial", { pct: Math.round(miss * 100) })}</span>
        </div>
      )}
    </CardShell>
  );
}
