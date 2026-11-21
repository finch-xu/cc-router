/**
 * 订阅管理 (设计稿: design/subscriptions-stats-v1.html)
 * 一行回答三个问题: 现在能不能用 / 被谁用 / 用了多少; 点行就地展开槽位、限额与连接信息。
 * 顶部是总览与「需要处理」(只列要人动手的, 冷却会自愈不进来)。
 */
import { useEffect, useMemo, useState } from "react";
import { Link, useNavigate } from "react-router";
import { AlertTriangle, ArrowRight, ChevronRight, Copy, Key, Plus, Search } from "lucide-react";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { EmptyState } from "@/components/EmptyState";
import { ProviderLogo } from "@/components/ProviderLogo";
import { stateLabel } from "@/components/StatusBadge";
import { BalanceBadge } from "@/components/SubscriptionBalanceCard";
import "@/components/subscriptions/subscriptions.css";
import {
  attentionOf,
  buildUses,
  cellTone,
  hostOf,
  isOAuth,
  quotaUsed,
  RANK,
  statusOf,
  tightestQuota,
  type AttentionKind,
  type StatusKind,
  type VmUse,
} from "@/components/subscriptions/subscriptionView";
import { useOverallStats, useSubscriptionActivity } from "@/hooks/useStatistics";
import { useCloneSubscription, useSetSubscriptionEnabled, useSubscriptions } from "@/hooks/useSubscriptions";
import { useProviders } from "@/hooks/useProviders";
import { useCustomVirtualModels, useVirtualModels } from "@/hooks/useVirtualModels";
import { useT, type TFunction } from "@/i18n";
import { fmtCompact, fmtNum, fmtRelativeTime } from "@/lib/format";
import { providerName } from "@/lib/providerText";
import type { OverallStatsDto, ProviderInfo, SubscriptionActivityDto, SubscriptionDto } from "@/types";

const REGION_LABEL_KEYS: Record<string, string> = {
  china: "subscriptions.region.china",
  global: "subscriptions.region.global",
  eu: "subscriptions.region.eu",
};

function regionOf(sub: SubscriptionDto, providers: ProviderInfo[] | undefined): string | undefined {
  if (sub.is_user_defined) return undefined;
  return providers?.find((p) => p.id === sub.provider_id)?.endpoints.find((e) => e.id === sub.endpoint_id)?.region;
}

type Filter = "all" | "ok" | "cool" | "attn" | "off" | "systemone" | "custom";
type Sort = "default" | "usage" | "name";

interface Row {
  sub: SubscriptionDto;
  kind: StatusKind;
  attn: AttentionKind | null;
  uses: VmUse[];
  act: SubscriptionActivityDto | undefined;
  provider: string;
  region?: string;
}

const FILTERS: { key: Filter; test: (r: Row) => boolean; tone?: "err" | "warn"; hideEmpty?: boolean }[] = [
  { key: "all", test: () => true },
  { key: "ok", test: (r) => r.kind === "ok" },
  { key: "cool", test: (r) => r.kind === "cool" || r.kind === "warn", tone: "warn" },
  { key: "attn", test: (r) => r.attn != null, tone: "err" },
  { key: "off", test: (r) => !r.sub.enabled },
  { key: "systemone", test: (r) => r.sub.endpoint_protocol === "systemone", hideEmpty: true },
  { key: "custom", test: (r) => r.sub.is_user_defined, hideEmpty: true },
];

/** 有订阅在冷却时 1 秒一跳, 倒计时要走; 否则不起定时器 */
function useTicker(active: boolean) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [active]);
  return now;
}

export function SubscriptionsPage() {
  const { t, locale } = useT();
  const subs = useSubscriptions();
  const providers = useProviders();
  const vms = useVirtualModels();
  const customVms = useCustomVirtualModels();
  const activity = useSubscriptionActivity();
  const today = useOverallStats("today");
  const [filter, setFilter] = useState<Filter>("all");
  const [sort, setSort] = useState<Sort>("default");
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState<Set<string>>(() => new Set());
  const [cloneSource, setCloneSource] = useState<SubscriptionDto | null>(null);

  const anyCooling = (subs.data ?? []).some((s) => s.cooldown_until && s.cooldown_until > Date.now());
  const now = useTicker(anyCooling);

  const rows: Row[] = useMemo(() => {
    const list = subs.data ?? [];
    const uses = buildUses(list, vms.data, customVms.data);
    const acts = new Map((activity.data ?? []).map((a) => [a.subscription_id, a]));
    return list.map((sub) => {
      const kind = statusOf(sub, now);
      return {
        sub,
        kind,
        attn: attentionOf(sub, kind),
        uses: uses.get(sub.id) ?? [],
        act: acts.get(sub.id),
        provider: providerName(sub, locale),
        region: REGION_LABEL_KEYS[regionOf(sub, providers.data) ?? ""],
      };
    });
  }, [subs.data, vms.data, customVms.data, activity.data, providers.data, now, locale]);

  const visible = useMemo(() => {
    const test = FILTERS.find((f) => f.key === filter)!.test;
    const q = query.trim().toLowerCase();
    const out = rows.filter(test).filter((r) => {
      if (!q) return true;
      const s = r.sub;
      const models = [...Object.values(s.model_slots), ...Object.values(s.slot_models_now)].filter(Boolean);
      return [s.display_name, r.provider, s.base_url, ...models].join(" ").toLowerCase().includes(q);
    });
    out.sort((a, b) => {
      if (sort === "usage") return (b.act?.requests ?? 0) - (a.act?.requests ?? 0) || a.sub.created_at - b.sub.created_at;
      if (sort === "name") return a.sub.display_name.localeCompare(b.sub.display_name, locale);
      return RANK[a.kind] - RANK[b.kind] || a.sub.created_at - b.sub.created_at;
    });
    return out;
  }, [rows, filter, query, sort, locale]);

  const toggleOpen = (id: string) =>
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  return (
    <div className="sb-page">
      <div className="page-actions">
        <div className="page-header" style={{ marginBottom: 0 }}>
          <h1>{t("subscriptions.title")}</h1>
          <div className="subtitle">{t("subscriptions.subtitle")}</div>
        </div>
        <Link className="btn primary" to="/subscriptions/new">
          <Plus size={13} /> {t("subscriptions.add")}
        </Link>
      </div>

      {subs.isLoading && <div className="field-hint">{t("common.loading")}</div>}

      {subs.data && subs.data.length === 0 && (
        <EmptyState
          icon={Key}
          message={t("subscriptions.empty.message")}
          action={
            <Link className="btn primary sm" to="/subscriptions/new">
              <Plus size={12} /> {t("subscriptions.empty.action")}
            </Link>
          }
        />
      )}

      {rows.length > 0 && (
        <>
          <Summary rows={rows} activity={activity.data} today={today.data} now={now} />
          <Attention rows={rows} />

          <div className="sb-toolbar">
            <div className="sb-chips" role="tablist">
              {FILTERS.map((f, i) => {
                const n = rows.filter(f.test).length;
                if (f.hideEmpty && n === 0) return null;
                return (
                  <span key={f.key} style={{ display: "contents" }}>
                    {f.hideEmpty && FILTERS[i - 1] && !FILTERS[i - 1].hideEmpty && <span className="gap" />}
                    <button
                      type="button"
                      role="tab"
                      aria-selected={filter === f.key}
                      className={"sb-chip" + (filter === f.key ? " active" : "") + (f.tone && n > 0 ? ` ${f.tone}` : "")}
                      onClick={() => setFilter(f.key)}
                    >
                      {t(`subscriptions.filter.${f.key}`)} <b>{n}</b>
                    </button>
                  </span>
                );
              })}
            </div>
            <div className="end">
              <label className="sb-search">
                <Search size={14} />
                <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t("subscriptions.search")} />
              </label>
              <div className="range-tabs" role="tablist" style={{ marginBottom: 0 }}>
                {(["default", "usage", "name"] as const).map((s) => (
                  <button
                    key={s}
                    type="button"
                    role="tab"
                    aria-selected={sort === s}
                    className={"range-tab" + (sort === s ? " active" : "")}
                    onClick={() => setSort(s)}
                  >
                    {t(`subscriptions.sort.${s}`)}
                  </button>
                ))}
              </div>
            </div>
          </div>

          <section className="card sb-list">
            <div className="sb-head">
              <span>{t("subscriptions.col.status")}</span>
              <span>{t("subscriptions.list.subscription")}</span>
              <span>{t("subscriptions.list.uses")}</span>
              <span>{t("subscriptions.list.activity")}</span>
              <span>{t("subscriptions.list.limits")}</span>
              <span />
            </div>
            {visible.length === 0 && <div className="sb-empty">{t("subscriptions.list.noMatch")}</div>}
            {visible.map((r) => (
              <SubscriptionRow
                key={r.sub.id}
                row={r}
                now={now}
                open={open.has(r.sub.id)}
                onToggle={() => toggleOpen(r.sub.id)}
                onClone={() => setCloneSource(r.sub)}
              />
            ))}
            <Link className="sb-add" to="/subscriptions/new">
              <Plus size={14} /> {t("subscriptions.add")}
            </Link>
          </section>
        </>
      )}

      <CloneDialog source={cloneSource} onClose={() => setCloneSource(null)} />
    </div>
  );
}

/* ---------------- 顶部总览 ---------------- */

function Spark({ values }: { values: number[] }) {
  const w = 160;
  const h = 26;
  const max = Math.max(...values, 1e-9);
  const pts = values.map((v, i) => [(i / Math.max(1, values.length - 1)) * w, h - 2 - (v / max) * (h - 4)]);
  const line = pts.map((p, i) => `${i ? "L" : "M"}${p[0].toFixed(1)} ${p[1].toFixed(1)}`).join(" ");
  return (
    <svg className="sb-spark" viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" aria-hidden>
      <path className="area" d={`${line} L${w} ${h} L0 ${h} Z`} />
      <path className="line" d={line} />
    </svg>
  );
}

function Summary({
  rows,
  activity,
  today,
  now,
}: {
  rows: Row[];
  activity: SubscriptionActivityDto[] | undefined;
  today: OverallStatsDto | undefined;
  now: number;
}) {
  const { t } = useT();
  const dispatchable = rows.filter((r) => r.kind === "ok").length;
  const cells = [...rows].sort((a, b) => RANK[a.kind] - RANK[b.kind]);

  const hourly = Array.from({ length: 24 }, (_, i) => (activity ?? []).reduce((s, a) => s + (a.buckets[i]?.requests ?? 0), 0));
  const req = (activity ?? []).reduce((s, a) => s + a.requests, 0);
  const fail = (activity ?? []).reduce((s, a) => s + a.failures, 0);

  const tok = today
    ? today.total_input_tokens + today.total_output_tokens + today.total_cache_creation_tokens + today.total_cache_read_tokens
    : 0;
  const hitBase = today ? today.total_input_tokens + today.total_cache_read_tokens + today.total_cache_creation_tokens : 0;
  const hit = today && hitBase ? today.total_cache_read_tokens / hitBase : null;

  let tight: { r: Row; ratio: number; q: SubscriptionDto["quota_usage"][number] } | null = null;
  for (const r of rows) {
    if (!r.sub.enabled) continue;
    const q = tightestQuota(r.sub);
    if (q && (!tight || q.ratio > tight.ratio)) tight = { r, ...q };
  }
  const tightTone = tight ? (tight.ratio >= 1 ? "full" : tight.ratio >= 0.8 ? "hi" : "") : "";

  return (
    <section className="card sb-sum">
      <div className="sb-kpi">
        <div className="l">{t("subscriptions.sum.dispatchable")}</div>
        <div className="v">
          {dispatchable}
          <small>/ {rows.length}</small>
        </div>
        <div className="sb-cells">
          {cells.map((r) => (
            <i key={r.sub.id} className={cellTone(r.kind)} title={r.sub.display_name} />
          ))}
        </div>
        <div className="sb-cells-key">
          <span><i style={{ background: "var(--ok)" }} />{t("subscriptions.sum.keyOk")}</span>
          <span><i style={{ background: "var(--warn)" }} />{t("subscriptions.sum.keyWarn")}</span>
          <span><i style={{ background: "var(--err)" }} />{t("subscriptions.sum.keyErr")}</span>
          <span><i style={{ background: "var(--line-2)" }} />{t("subscriptions.sum.keyOff")}</span>
        </div>
      </div>
      <div className="sb-kpi">
        <div className="l">{t("subscriptions.sum.requests24h")}</div>
        <div className="v">{fmtNum(req)}</div>
        <div className="s">
          {req
            ? t("subscriptions.sum.requestsSub", { rate: (((req - fail) / req) * 100).toFixed(1), fails: fmtNum(fail) })
            : t("subscriptions.sum.noRequests")}
        </div>
        <Spark values={hourly} />
      </div>
      <div className="sb-kpi">
        <div className="l">{t("subscriptions.sum.todayTokens")}</div>
        <div className="v">{fmtCompact(tok)}</div>
        <div className="s">
          {hit == null ? "—" : t("subscriptions.sum.cacheHit", { pct: Math.round(hit * 100) })}
        </div>
      </div>
      <div className="sb-kpi">
        <div className="l">{t("subscriptions.sum.tightest")}</div>
        {tight ? (
          <>
            <div className={"v" + (tightTone === "full" ? " err" : tightTone === "hi" ? " warn" : "")}>
              {Math.round(tight.ratio * 100)}
              <small>%</small>
            </div>
            <div className="sb-tight">
              <ProviderLogo iconId={tight.r.sub.provider_icon} size={18} />
              <span>
                {tight.r.sub.display_name} · {t(`quota.period.${tight.q.period}`)}
              </span>
              <div className="sb-qbar">
                <i className={tightTone} style={{ width: `${Math.min(100, tight.ratio * 100)}%` }} />
              </div>
            </div>
            {tight.q.period_end_ms && (
              <div className="s" style={{ marginTop: 4 }}>
                {t("subscriptions.sum.resetAt", { time: fmtReset(tight.q.period_end_ms, now) })}
              </div>
            )}
          </>
        ) : (
          <>
            <div className="v" style={{ color: "var(--ink-4)" }}>—</div>
            <div className="s">{t("subscriptions.sum.noQuota")}</div>
          </>
        )}
      </div>
    </section>
  );
}

/** 重置时间: 当天只写时刻, 否则带日期 */
function fmtReset(ms: number, now: number): string {
  const d = new Date(ms);
  const sameDay = new Date(now).toDateString() === d.toDateString();
  return sameDay
    ? d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", hour12: false })
    : d.toLocaleString(undefined, { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit", hour12: false });
}

/* ---------------- 需要处理 ---------------- */

function Attention({ rows }: { rows: Row[] }) {
  const { t } = useT();
  const navigate = useNavigate();
  const list = rows.filter((r) => r.attn != null).sort((a, b) => RANK[a.kind] - RANK[b.kind]);
  if (list.length === 0) return null;
  return (
    <section className="card sb-attn">
      <div className="sb-attn-head">
        <AlertTriangle size={14} />
        {t("subscriptions.attn.title")} <span className="n">{list.length}</span>
        <span className="note">{t("subscriptions.attn.note")}</span>
      </div>
      {list.map((r) => {
        const err = r.attn === "auth" || r.attn === "key";
        return (
          <div key={r.sub.id} className={"sb-fix " + (err ? "err" : "warn")}>
            <ProviderLogo iconId={r.sub.provider_icon} size={26} />
            <div className="t">
              <b>{r.sub.display_name}</b>
              <div className="why">
                <em>{t(`subscriptions.attn.${r.attn}`)}</em>
                <span>{attnDetail(r, t)}</span>
                {r.attn === "balance" && <BalanceBadge subscription={r.sub} />}
              </div>
            </div>
            <div className="acts">
              <button type="button" className={"btn sm" + (err ? " primary" : "")} onClick={() => navigate(`/subscriptions/${r.sub.id}`)}>
                {t(`subscriptions.attn.fix.${r.attn}`)}
              </button>
            </div>
          </div>
        );
      })}
    </section>
  );
}

function attnDetail(r: Row, t: TFunction): string {
  const s = r.sub;
  if (r.attn === "auth") return s.last_error_message ?? "";
  if (r.attn === "key") return t("subscriptions.attn.keyDetail");
  if (r.attn === "full") {
    const q = s.quota_usage.find((x) => x.exceeded);
    if (!q) return "";
    const base = t("subscriptions.attn.fullDetail", {
      period: t(`quota.period.${q.period}`),
      used: fmtCompact(quotaUsed(q)),
      limit: fmtCompact(q.limit ?? 0),
    });
    return q.period_end_ms ? `${base} · ${t("subscriptions.sum.resetAt", { time: fmtReset(q.period_end_ms, Date.now()) })}` : base;
  }
  return "";
}

/* ---------------- 订阅行 ---------------- */

function StatusCell({ row, now }: { row: Row; now: number }) {
  const { t } = useT();
  const s = row.sub;
  switch (row.kind) {
    case "ok":
      return (
        <div className="sb-st"><span className="sb-dot ok" /><span>{t("subscriptions.st.ok")}</span></div>
      );
    case "cool": {
      // 冷却总时长后端不给, 圆环按「1 分钟满格」画: 最后一分钟里看得见它在走
      const left = Math.max(0, (s.cooldown_until ?? now) - now);
      const total = Math.max(60_000, left);
      const c = 2 * Math.PI * 6;
      return (
        <div className="sb-st warn" title={stateLabel(s.state, t)}>
          <svg className="sb-ring" viewBox="0 0 16 16" aria-hidden>
            <circle className="bg" cx="8" cy="8" r="6" />
            <circle className="fg" cx="8" cy="8" r="6" strokeDasharray={`${(c * Math.min(1, left / total)).toFixed(1)} ${c.toFixed(1)}`} />
          </svg>
          <span>
            {t("subscriptions.st.cooling")} <small>{fmtLeft(left)}</small>
          </span>
        </div>
      );
    }
    case "warn":
      return (
        <div className="sb-st warn"><span className="sb-dot warn" /><span>{stateLabel(s.state, t)}</span></div>
      );
    case "auth":
      return (
        <div className="sb-st err"><span className="sb-dot err" /><span>{stateLabel(s.state, t)}</span></div>
      );
    case "full":
      return (
        <div className="sb-st warn"><span className="sb-dot warn" /><span>{t("subscriptions.st.full")}</span></div>
      );
    case "key":
      return (
        <div className="sb-st err"><span className="sb-dot err" /><span>{t("subscriptions.missingKey")}</span></div>
      );
    case "off":
      return (
        <div className="sb-st off"><span className="sb-dot" /><span>{t("subscriptions.st.off")}</span></div>
      );
  }
}

function fmtLeft(ms: number): string {
  const s = Math.ceil(ms / 1000);
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.ceil(s / 60)}m`;
  return `${Math.ceil(s / 3600)}h`;
}

function SubscriptionRow({
  row,
  now,
  open,
  onToggle,
  onClone,
}: {
  row: Row;
  now: number;
  open: boolean;
  onToggle: () => void;
  onClone: () => void;
}) {
  const { t } = useT();
  const setEnabled = useSetSubscriptionEnabled();
  const s = row.sub;
  const act = row.act;
  const buckets = act?.buckets ?? [];
  const max = Math.max(1, ...buckets.map((b) => b.requests));
  const tight = tightestQuota(s);
  const tone = tight ? (tight.ratio >= 1 ? "full" : tight.ratio >= 0.8 ? "hi" : "") : "";
  const rate = act && act.requests ? (act.requests - act.failures) / act.requests : null;

  return (
    <div className={"sb-item" + (open ? " open" : "") + (s.enabled ? "" : " off")}>
      <div
        className="sb-row"
        role="button"
        tabIndex={0}
        aria-expanded={open}
        onClick={onToggle}
        onKeyDown={(e) => {
          if (e.target !== e.currentTarget) return;
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            onToggle();
          }
        }}
      >
        <StatusCell row={row} now={now} />
        <div className="sb-who">
          <ProviderLogo iconId={s.provider_icon} size={26} />
          <div className="t">
            <div className="l1">
              <b>{s.display_name}</b>
              {row.region && <span className="sb-tag">{t(row.region)}</span>}
              {s.is_user_defined && <span className="sb-tag line">{t("subscriptions.custom")}</span>}
              {isOAuth(s) && <span className="sb-tag line">OAuth</span>}
              {s.endpoint_protocol === "systemone" && <span className="sb-tag acc">System One</span>}
            </div>
            <div className="l2">
              {row.provider} · <span className="mono">{hostOf(s.base_url)}</span>
            </div>
          </div>
        </div>
        <div className="sb-uses">
          {row.uses.length === 0 ? (
            <span className="sb-none">{t("subscriptions.uses.none")}</span>
          ) : (
            row.uses.map((u) => (
              <span
                key={u.full}
                className={"sb-vm" + (u.pos === 1 ? " first" : "")}
                title={u.pos ? t("subscriptions.uses.pos", { vm: u.full, pos: u.pos }) : u.full}
              >
                {u.name}
                {u.pos != null && <em>#{u.pos}</em>}
              </span>
            ))
          )}
        </div>
        <div className="sb-act">
          <div className="sb-bars" aria-hidden>
            {(buckets.length ? buckets : Array.from({ length: 24 }, () => ({ requests: 0, failures: 0 }))).map((b, i) => (
              <i
                key={i}
                className={b.requests === 0 ? "z" : b.failures > 0 ? "x" : undefined}
                style={{ height: b.requests === 0 ? 2 : Math.max(3, (b.requests / max) * 22) }}
              />
            ))}
          </div>
          <div className="n">
            <span>
              <b>{fmtNum(act?.requests ?? 0)}</b> {t("subscriptions.activity.times")}
            </span>
            <span className={rate != null && rate < 0.95 ? "bad" : undefined}>
              {rate == null ? "—" : `${(rate * 100).toFixed(1)}%`}
            </span>
          </div>
        </div>
        <div className="sb-lim">
          {tight && (
            <>
              <div className="r1">
                <span>{t("subscriptions.limit.period", { period: t(`quota.period.${tight.q.period}`) })}</span>
                <b className={tone}>{Math.round(tight.ratio * 100)}%</b>
              </div>
              <div className="sb-qbar">
                <i className={tone} style={{ width: `${Math.min(100, tight.ratio * 100)}%` }} />
              </div>
            </>
          )}
          {s.balance_supported && s.balance_cache && (
            <div className="bal">
              <span>{t("subscriptions.limit.balance")}</span>
              <BalanceBadge subscription={s} />
            </div>
          )}
          {!tight && !(s.balance_supported && s.balance_cache) && <span className="muted">{t("subscriptions.quotaNone")}</span>}
        </div>
        <div className="sb-acts">
          <button
            type="button"
            className={"sb-toggle" + (s.enabled ? " on" : "")}
            role="switch"
            aria-checked={s.enabled}
            title={t(s.enabled ? "subscriptions.toggle.disable" : "subscriptions.toggle.enable")}
            disabled={setEnabled.isPending || (!s.enabled && !s.has_api_key && !isOAuth(s))}
            onClick={(e) => {
              e.stopPropagation();
              setEnabled.mutate({ id: s.id, enabled: !s.enabled });
            }}
          />
          <span className="sb-chev" aria-hidden>
            <ChevronRight size={14} />
          </span>
        </div>
      </div>
      {open && <RowDetail row={row} onClone={onClone} />}
    </div>
  );
}

const SLOTS = ["fable", "opus", "sonnet", "haiku"] as const;

function RowDetail({ row, onClone }: { row: Row; onClone: () => void }) {
  const { t } = useT();
  const s = row.sub;
  const quotas = s.quota_usage.filter((q) => q.limit != null);
  const systemone = s.endpoint_protocol === "systemone";

  return (
    <div className="sb-more">
      <div className="sb-more-grid">
        <div>
          <h4>{t("subscriptions.more.slots")}</h4>
          <div className="sb-slots">
            {systemone ? (
              <SlotLine k="jev" model={s.model_slots.jev || null} empty={t("subscriptions.more.passthrough")} />
            ) : (
              <>
                {SLOTS.map((k) => (
                  <SlotLine
                    key={k}
                    k={k}
                    model={s.slot_models_now[k] ?? (s.model_slots[k] || null)}
                    effort={s.slot_efforts[k]}
                    scheduled={(s.slot_schedules[k]?.length ?? 0) > 0}
                  />
                ))}
                <SlotLine k={t("subscriptions.more.fallback")} model={s.model_slots.fallback || null} empty={t("subscriptions.more.passthrough")} />
              </>
            )}
          </div>
        </div>
        <div>
          <h4>{t("subscriptions.more.quota")}</h4>
          {quotas.length === 0 ? (
            <div className="sb-none">{t("subscriptions.more.noQuota")}</div>
          ) : (
            <div className="sb-qrows">
              {quotas.map((q) => {
                const limit = q.limit!;
                return (
                  <div key={q.period} className="sb-qrow">
                    <div className="r1">
                      <span>{t(`quota.period.${q.period}`)}</span>
                      <span>
                        <b className={q.exceeded ? "full" : undefined}>{fmtCompact(quotaUsed(q))}</b> / {fmtCompact(limit)}
                      </span>
                    </div>
                    <div className="sb-seg4">
                      {(["cache_read", "cache_creation", "input", "output"] as const).map((k, i) => (
                        <i key={k} className={["cr", "cw", "in", "out"][i]} style={{ width: `${(Math.min(q[k], limit) / limit) * 100}%` }} />
                      ))}
                    </div>
                  </div>
                );
              })}
              <div className="sb-qkey">
                <span><i className="cr" />{t("stats.daily.tokenTooltipCacheRead")}</span>
                <span><i className="cw" />{t("stats.daily.tokenTooltipCacheCreate")}</span>
                <span><i className="in" />{t("stats.daily.tokenTooltipInput")}</span>
                <span><i className="out" />{t("stats.daily.tokenTooltipOutput")}</span>
              </div>
            </div>
          )}
        </div>
        <div>
          <h4>{t("subscriptions.more.connection")}</h4>
          <dl className="sb-kv">
            <dt>{t("subscriptions.more.endpoint")}</dt>
            <dd className="mono" title={s.base_url}>{s.base_url.replace(/^https?:\/\//, "")}</dd>
            <dt>{t("subscriptions.more.auth")}</dt>
            <dd>
              {isOAuth(s)
                ? t("subscriptions.more.authOauth")
                : s.has_api_key
                  ? t("subscriptions.more.authKey")
                  : t("subscriptions.missingKey")}
            </dd>
            <dt>{t("subscriptions.more.models")}</dt>
            <dd>
              {s.model_cache
                ? t("subscriptions.more.modelsCount", {
                    n: s.model_cache.models.length,
                    when: fmtRelativeTime(s.model_cache.fetched_at, t),
                  })
                : "—"}
            </dd>
            {s.balance_supported && s.balance_cache && (
              <>
                <dt>{t("subscriptions.limit.balance")}</dt>
                <dd>
                  <BalanceBadge subscription={s} />
                </dd>
              </>
            )}
            {s.last_error_message && (
              <>
                <dt>{t("subscriptions.more.lastError")}</dt>
                <dd className="err">{s.last_error_message}</dd>
              </>
            )}
          </dl>
        </div>
      </div>
      <div className="sb-more-foot">
        {!isOAuth(s) && (
          <button type="button" className="btn sm" onClick={onClone}>
            <Copy size={12} /> {t("subscriptions.clone.action")}
          </button>
        )}
        <Link className="sb-link" to={`/request-logs?range=today&subscription_id=${encodeURIComponent(s.id)}`}>
          {t("subscriptions.more.todayLogs")} <ArrowRight size={12} />
        </Link>
        <div className="end">
          <Link className="btn sm primary" to={`/subscriptions/${s.id}`}>
            {t("subscriptions.more.open")}
          </Link>
        </div>
      </div>
    </div>
  );
}

function SlotLine({
  k,
  model,
  effort,
  scheduled,
  empty,
}: {
  k: string;
  model: string | null;
  effort?: string;
  scheduled?: boolean;
  empty?: string;
}) {
  const { t } = useT();
  return (
    <div className="sb-slot">
      <span className="k">{k}</span>
      <span className={"m" + (model ? "" : " empty")} title={model ?? undefined}>
        {model ?? empty ?? "—"}
      </span>
      <span className="x">
        {effort && <span className="sb-tag">effort {effort}</span>}
        {scheduled && <span className="sb-tag warn">{t("subscriptions.more.scheduled")}</span>}
      </span>
    </div>
  );
}

/* ---------------- 复制订阅 ---------------- */

function CloneDialog({ source, onClose }: { source: SubscriptionDto | null; onClose: () => void }) {
  const { t } = useT();
  const clone = useCloneSubscription();
  const [name, setName] = useState("");
  useEffect(() => {
    if (source) {
      clone.reset();
      setName(t("subscriptions.clone.defaultName", { name: source.display_name }));
    }
  }, [source]);

  return (
    <Dialog
      open={source !== null}
      onOpenChange={(o) => {
        if (!o && !clone.isPending) onClose();
      }}
    >
      <DialogContent className="cc-dialog">
        <DialogHeader>
          <DialogTitle>{t("subscriptions.clone.action")}</DialogTitle>
        </DialogHeader>
        <form
          className="grid gap-4"
          onSubmit={(event) => {
            event.preventDefault();
            if (!source || !name.trim() || clone.isPending) return;
            clone.mutate({ id: source.id, name }, { onSuccess: onClose });
          }}
        >
          <label className="grid gap-1">
            {t("subscriptions.clone.name")}
            <input
              className="input"
              required
              maxLength={80}
              value={name}
              onChange={(event) => setName(event.target.value)}
              disabled={clone.isPending}
            />
          </label>
          <p className="field-hint">{t("subscriptions.clone.hint")}</p>
          {clone.isError && <p role="alert">{t("subscriptions.clone.error")}</p>}
          <DialogFooter>
            <button type="button" className="btn" disabled={clone.isPending} onClick={onClose}>
              {t("common.cancel")}
            </button>
            <button type="submit" className="btn primary" disabled={clone.isPending || !name.trim()}>
              {t("subscriptions.clone.create")}
            </button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
