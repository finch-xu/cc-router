/**
 * 实时事件: 每行 = 一次上游 attempt。左边讲「它在干什么」(用途 / 会话 / 轮次 / 上一步工具 → 这一步决定),
 * 右边讲「它怎么被路由」(虚拟模型 → 订阅 / 模型 / 切换) 与消耗。点开一行看判定依据。
 *
 * 只显示工具名: 后端从来不下发命令、文件内容或提示词。
 */
import { Fragment, useMemo, useRef, useState, type ReactNode } from "react";
import { ArrowRight } from "lucide-react";
import { Link } from "react-router";
import { ProviderLogo } from "@/components/ProviderLogo";
import { useLiveSessions, type SessionInfo } from "@/components/live/useLiveSessions";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useT, type TFunction } from "@/i18n";
import {
  cacheHitRatio,
  clientKey,
  clientMeta,
  countNames,
  fmtClock,
  fmtMs,
  fmtTok,
  isOk,
  matchFilter,
  PURPOSE_META,
  sessionColor,
  sumTokens,
  totalTokens,
  type FeedFilter,
} from "@/lib/liveStats";
import type { LiveRequest, SubscriptionDto } from "@/types";

const MAX_ROWS = 12;
const FILTERS: FeedFilter[] = ["all", "main", "agent", "bg", "fail"];

interface Props {
  rows: readonly LiveRequest[];
  now: number;
  paused: boolean;
}

export function LiveEventFeed({ rows }: Props) {
  const { t } = useT();
  const subs = useSubscriptions();
  const sessions = useLiveSessions();
  const [filter, setFilter] = useState<FeedFilter>("all");
  const [session, setSession] = useState<string>("all");
  const [open, setOpen] = useState<Set<string>>(() => new Set());
  // 首次渲染时已在的行不算「新」, 之后出现的行播一次入场动画
  const seen = useRef<Set<string> | null>(null);

  const subsById = useMemo(() => new Map((subs.data ?? []).map((s) => [s.id, s])), [subs.data]);

  const counts = useMemo(() => {
    const c: Record<FeedFilter, number> = { all: 0, main: 0, agent: 0, bg: 0, fail: 0 };
    for (const r of rows) for (const f of FILTERS) if (matchFilter(r, f)) c[f] += 1;
    return c;
  }, [rows]);

  const sessionIds = useMemo(() => {
    const m = new Map<string, number>();
    for (const r of rows) if (r.session) m.set(r.session.id, r.timestamp);
    return [...m.entries()].sort((a, b) => b[1] - a[1]).slice(0, 6).map(([id]) => id);
  }, [rows]);

  const list = useMemo(() => {
    const out: LiveRequest[] = [];
    for (let i = rows.length - 1; i >= 0 && out.length < MAX_ROWS; i -= 1) {
      const r = rows[i];
      if (!matchFilter(r, filter)) continue;
      if (session !== "all" && r.session?.id !== session) continue;
      out.push(r);
    }
    return out;
  }, [rows, filter, session]);

  /** 失败的 attempt 之后, 同一虚拟模型上重试次数 +1 的那条就是「改走」的下家 */
  const nextSub = (r: LiveRequest): string | undefined => {
    if (isOk(r)) return undefined;
    const i = rows.indexOf(r);
    for (let j = i + 1; j < rows.length && rows[j].timestamp - r.timestamp < 120_000; j += 1) {
      const n = rows[j];
      if (n.virtual_model === r.virtual_model && n.retry_count === r.retry_count + 1 && n.client_tool === r.client_tool) {
        return subsById.get(n.subscription_id)?.display_name ?? n.provider_id;
      }
    }
    return undefined;
  };

  // 第一批出现的行 (含首屏回填) 不算新; 之后才播入场动画。只记最近的 id, 不无限增长
  const fresh = new Set<string>();
  if (seen.current) {
    for (const r of list) if (!seen.current.has(r.id)) fresh.add(r.id);
  }
  if (list.length > 0 || seen.current) {
    seen.current = new Set([...[...(seen.current ?? [])].slice(-200), ...list.map((r) => r.id)]);
  }

  const toggle = (id: string) =>
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  return (
    <section className="card lv-section">
      <div className="card-head">
        <div className="lv-head-lead">
          <span className="card-title">{t("live.feed.title")}</span>
          <span className="card-sub">{t("live.feed.sub")}</span>
        </div>
        <Link className="lv-link" to="/request-logs">
          {t("live.feed.toLogs")} <ArrowRight size={13} />
        </Link>
      </div>
      <div className="card-body">
        <div className="lv-feed-bar">
          {FILTERS.map((f) => (
            <button
              key={f}
              type="button"
              className={"lv-chip" + (filter === f ? " on" : "") + (f === "fail" && counts.fail ? " err" : "")}
              onClick={() => setFilter(f)}
            >
              {t(`live.filter.${f}`)}
              <b>{counts[f]}</b>
            </button>
          ))}
          {sessionIds.length > 0 && <span className="lv-gap" />}
          {sessionIds.length > 0 && (
            <button type="button" className={"lv-chip" + (session === "all" ? " on" : "")} onClick={() => setSession("all")}>
              {t("live.filter.allSessions")}
            </button>
          )}
          {sessionIds.map((id) => (
            <button key={id} type="button" className={"lv-chip" + (session === id ? " on" : "")} onClick={() => setSession(id)}>
              <i className="sd" style={{ background: sessionColor(id) }} />
              {sessions.get(id)?.project ?? id.slice(0, 8)}
            </button>
          ))}
        </div>
        {list.length === 0 ? (
          <div className="lv-empty">{rows.length ? t("live.feed.emptyFilter") : t("live.feed.empty")}</div>
        ) : (
          <div className="lv-feed">
            {list.map((r) => (
              <FeedRow
                key={r.id}
                r={r}
                sub={subsById.get(r.subscription_id)}
                session={r.session ? sessions.get(r.session.id) : undefined}
                next={nextSub(r)}
                fresh={fresh.has(r.id)}
                open={open.has(r.id)}
                onToggle={() => toggle(r.id)}
                t={t}
              />
            ))}
          </div>
        )}
      </div>
    </section>
  );
}

function ToolChips({ names, t }: { names: readonly string[]; t: TFunction }) {
  const counted = countNames(names);
  const shown = counted.slice(0, 3);
  return (
    <>
      {shown.map(([n, c], i) => (
        <Fragment key={n}>
          {i > 0 && " "}
          <span className="lv-tool">{n}</span>
          {c > 1 && <span className="lv-times">×{c}</span>}
        </Fragment>
      ))}
      {counted.length > 3 && <span className="lv-times">{t("live.feed.moreTools", { n: counted.length - 3 })}</span>}
    </>
  );
}

/** 第二行: 「前因 → 后果」 */
function Story({ r, t }: { r: LiveRequest; t: TFunction }) {
  const p = r.purpose;
  const kind = p.kind;
  if (kind !== "main" && kind !== "agent" && kind !== "unknown") {
    return <>{t(`live.story.${kind}`)}</>;
  }
  const cause =
    p.prev_tools.length > 0 ? (
      <>
        {t("live.story.returned")} <ToolChips names={p.prev_tools} t={t} />
      </>
    ) : p.step === 1 ? (
      <>{kind === "agent" ? t("live.story.agentStart") : t("live.story.newPrompt")}</>
    ) : null;
  let effect: ReactNode = null;
  if (!isOk(r)) effect = null;
  else if (r.tool_use_names.length > 0)
    effect = (
      <>
        {t("live.story.calls")} <ToolChips names={r.tool_use_names} t={t} />
      </>
    );
  else if (r.stop_reason === "end_turn" || r.stop_reason === "stop") effect = t("live.story.answered");
  else if (r.stop_reason === "max_tokens" || r.stop_reason === "length") effect = t("live.story.truncated");
  if (!cause && !effect) return <>{t("live.story.tools", { n: r.tools_offered ?? 0 })}</>;
  return (
    <>
      {cause}
      {cause && effect && <span className="lv-arr">→</span>}
      {effect}
    </>
  );
}

function title(r: LiveRequest, session: SessionInfo | undefined, t: TFunction): string {
  const p = r.purpose;
  if (p.kind === "agent") {
    const kinds = session?.runningAgents ?? [];
    const name = kinds.length === 1 ? kinds[0] : t("live.purpose.agent");
    return p.step ? t("live.title.agentStep", { name, step: p.step }) : name;
  }
  if ((p.kind === "main" || p.kind === "unknown") && p.turn && p.step) {
    return t("live.title.turnStep", { turn: p.turn, step: p.step });
  }
  return t(`live.title.${p.kind}`);
}

function why(r: LiveRequest, t: TFunction): string {
  const codex = r.client_tool?.startsWith("codex");
  if (r.purpose.kind === "main" && codex) return t("live.why.codex");
  return t(`live.why.${r.purpose.kind}`);
}

function FeedRow(props: {
  r: LiveRequest;
  sub: SubscriptionDto | undefined;
  session: SessionInfo | undefined;
  next: string | undefined;
  fresh: boolean;
  open: boolean;
  onToggle: () => void;
  t: TFunction;
}) {
  const { r, sub, session, next, fresh, open, onToggle, t } = props;
  const meta = PURPOSE_META[r.purpose.kind];
  const Icon = meta.icon;
  const ok = isOk(r);
  const cm = clientMeta(clientKey(r.client_tool));
  const clientName = cm ? t(cm.i18nKey) : t("live.otherClient");
  const parts = sumTokens([r]);
  const hit = cacheHitRatio(parts);
  const cls = ["lv-ev", fresh ? "new" : "", ok ? "" : "err", ok && r.retry_count > 0 ? "fo" : "", open ? "open" : ""]
    .filter(Boolean)
    .join(" ");
  return (
    <div className={cls} onClick={onToggle} role="button" tabIndex={0} aria-expanded={open} onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), onToggle())}>
      <span className="tm">{fmtClock(r.timestamp)}</span>
      <span className={`lv-ptag ${meta.tone}`}>
        <Icon size={12} />
        {t(`live.purpose.${r.purpose.kind}`)}
      </span>
      <div className="what">
        <div className="l1">
          {r.session ? (
            <span className="lv-sess">
              <i style={{ background: sessionColor(r.session.id) }} />
              {session?.project ?? clientName}
              <span className="mono">{r.session.id.slice(0, 4)}</span>
            </span>
          ) : (
            <span className="lv-sess">{clientName}</span>
          )}
          <b>{title(r, session, t)}</b>
        </div>
        <div className="l2">
          <Story r={r} t={t} />
        </div>
      </div>
      <div className="rt">
        <div className="l1">
          <span className="m">{r.virtual_model}</span>
          <ArrowRight size={12} />
          <ProviderLogo iconId={sub?.provider_icon} size={16} />
          <span className="s">{sub?.display_name ?? r.provider_id}</span>
        </div>
        <div className="l2">
          {ok ? (
            r.retry_count > 0 ? (
              <span className="w">{t("live.feed.recovered")}</span>
            ) : (
              <>
                {r.model}
                {r.effort ? ` · ${r.effort}` : ""}
              </>
            )
          ) : (
            <>
              <span className="e">{errorLabel(r, t)}</span>
              {next && <span className="w"> → {t("live.feed.switchedTo", { name: next })}</span>}
            </>
          )}
        </div>
      </div>
      <div className="mx">
        <div className="l1">{ok ? `${fmtTok(totalTokens(r))} tok` : (r.http_status ?? r.status)}</div>
        <div className="l2">{ok ? t("live.feed.timing", { ttft: fmtMs(r.ttft_ms), total: fmtMs(r.total_ms) }) : fmtMs(r.total_ms)}</div>
      </div>
      {open && (
        <div className="more" onClick={(e) => e.stopPropagation()}>
          <div><small>{t("live.more.client")}</small><span>{clientName}</span></div>
          <div><small>{t("live.more.tokens")}</small><span>{fmtTok(parts.input)} / {fmtTok(parts.cacheRead)} / {fmtTok(parts.cacheWrite)} / {fmtTok(parts.output)}</span></div>
          <div><small>{t("live.more.cache")}</small><span>{hit == null ? "—" : `${Math.round(hit * 100)}%`}</span></div>
          <div><small>{t("live.more.tools")}</small><span>{r.tools_offered ?? "—"}</span></div>
          <div><small>{t("live.more.model")}</small><span className="mono">{r.model}</span></div>
          <div><small>{t("live.more.effort")}</small><span>{r.effort ?? "—"}</span></div>
          <div><small>{t("live.more.retry")}</small><span>{r.retry_count}</span></div>
          <div><small>{t("live.more.session")}</small><span className="mono">{r.session?.id.slice(0, 12) ?? "—"}</span></div>
          {r.error && <div className="err-line"><small>{t("live.more.error")}</small><span>{r.error}</span></div>}
          <div className="why">
            <b>{t("live.more.why")}</b>
            <span>{why(r, t)}</span>
            {r.purpose.guessed && <span className="guess">{t("live.more.guessed")}</span>}
          </div>
        </div>
      )}
    </div>
  );
}

function errorLabel(r: LiveRequest, t: TFunction): string {
  if (r.status === "timeout") return t("live.err.timeout");
  if (r.http_status === 429) return t("live.err.rateLimited");
  if (r.http_status && r.http_status >= 500) return t("live.err.upstream", { code: r.http_status });
  if (r.http_status) return t("live.err.status", { code: r.http_status });
  return t("live.err.failed");
}
