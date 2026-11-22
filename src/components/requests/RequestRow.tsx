/**
 * 一条请求的行: 实时路由的事件流与请求日志列表共用 (设计稿 design/request-logs-v1.html)。
 * 左边讲「它在干什么」(用途 / 会话 / 轮次 / 上一步工具 → 这一步决定),
 * 右边讲「它怎么被路由」(虚拟模型 → 订阅 / 模型 / 切换) 与消耗。
 *
 * 两种用法 (`mode`):
 * - `expand` (实时路由): 点一下原地展开概要;
 * - `link` (请求日志): 点一下进整页详情。
 *
 * 只显示工具名: 后端从来不下发命令、文件内容或提示词。
 */
import { Fragment, type ReactNode } from "react";
import { ArrowRight } from "lucide-react";
import { ProviderLogo } from "@/components/ProviderLogo";
import { customProviderLabel } from "@/lib/providerLabels";
import type { SessionInfo } from "@/components/live/useLiveSessions";
import type { TFunction } from "@/i18n";
import {
  cacheHitRatio,
  clientKey,
  clientMeta,
  countNames,
  fmtClock,
  fmtMs,
  fmtTok,
  isOk,
  PURPOSE_META,
  purposeOf,
  sessionColor,
  sumTokens,
  totalTokens,
} from "@/lib/liveStats";
import type { RequestSummary, SubscriptionDto } from "@/types";
import "./request-row.css";

export function ToolChips({ names, t }: { names: readonly string[]; t: TFunction }) {
  const counted = countNames(names);
  const shown = counted.slice(0, 3);
  return (
    <>
      {shown.map(([n, c], i) => (
        <Fragment key={n}>
          {i > 0 && " "}
          <span className="rq-tool">{n}</span>
          {c > 1 && <span className="rq-times">×{c}</span>}
        </Fragment>
      ))}
      {counted.length > 3 && <span className="rq-times">{t("live.feed.moreTools", { n: counted.length - 3 })}</span>}
    </>
  );
}

/** 第二行: 「前因 → 后果」 */
export function Story({ r, t }: { r: RequestSummary; t: TFunction }) {
  const p = purposeOf(r);
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
      {cause && effect && <span className="rq-arr">→</span>}
      {effect}
    </>
  );
}

export function rowTitle(r: RequestSummary, session: SessionInfo | undefined, t: TFunction): string {
  if (r.purpose === null) return t("live.title.legacy");
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

export function whyText(r: RequestSummary, t: TFunction): string {
  const kind = purposeOf(r).kind;
  const codex = r.client_tool?.startsWith("codex");
  if (kind === "main" && codex) return t("live.why.codex");
  return t(`live.why.${kind}`);
}

export function errorLabel(r: RequestSummary, t: TFunction): string {
  if (r.status === "timeout") return t("live.err.timeout");
  if (r.http_status === 429) return t("live.err.rateLimited");
  if (r.http_status && r.http_status >= 500) return t("live.err.upstream", { code: r.http_status });
  if (r.http_status) return t("live.err.status", { code: r.http_status });
  return t("live.err.failed");
}

export function clientLabel(r: RequestSummary, t: TFunction): string {
  const cm = clientMeta(clientKey(r.client_tool));
  return cm ? t(cm.i18nKey) : t("live.otherClient");
}

/** 用途标签; 老记录 (purpose === null) 显示「未记录用途」 */
export function PurposeTag({ r, t }: { r: RequestSummary; t: TFunction }) {
  if (r.purpose === null) return <span className="rq-ptag legacy">{t("live.purpose.unrecorded")}</span>;
  const meta = PURPOSE_META[r.purpose.kind];
  const Icon = meta.icon;
  return (
    <span className={`rq-ptag ${meta.tone}`}>
      <Icon size={12} />
      {t(`live.purpose.${r.purpose.kind}`)}
    </span>
  );
}

export function SessionChip({ r, session, t }: { r: RequestSummary; session: SessionInfo | undefined; t: TFunction }) {
  if (!r.session) return <span className="rq-sess">{clientLabel(r, t)}</span>;
  return (
    <span className="rq-sess">
      <i style={{ background: sessionColor(r.session.id) }} />
      {session?.project ?? clientLabel(r, t)}
      <span className="mono">{r.session.id.slice(0, 4)}</span>
    </span>
  );
}

export interface RequestRowProps {
  r: RequestSummary;
  sub: SubscriptionDto | undefined;
  session: SessionInfo | undefined;
  /** 失败后改走的下家 (显示名) */
  next?: string;
  /** 新到的行播一次入场动画 */
  fresh?: boolean;
  mode: "expand" | "link";
  /** mode=expand 时是否展开 */
  open?: boolean;
  /** 请求日志里挂在最终那次下面的失败尝试: 第几次 / 共几次 */
  attempt?: { index: number; total: number };
  /** 是否在本行被选中 (键盘 / 刚从详情页返回) */
  selected?: boolean;
  onActivate: () => void;
  /** 展开面板末尾的操作 (如「查看完整详情」) */
  moreAction?: ReactNode;
  t: TFunction;
}

export function RequestRow(props: RequestRowProps) {
  const { r, sub, session, next, fresh, mode, open, attempt, selected, onActivate, moreAction, t } = props;
  const ok = isOk(r);
  const parts = sumTokens([r]);
  const hit = cacheHitRatio(parts);
  const expanded = mode === "expand" && open;
  const cls = [
    "rq-row",
    fresh ? "new" : "",
    ok ? "" : "err",
    ok && r.retry_count > 0 ? "fo" : "",
    expanded ? "open" : "",
    attempt ? "attempt" : "",
    selected ? "open" : "",
  ]
    .filter(Boolean)
    .join(" ");
  const p = purposeOf(r);
  return (
    <div
      className={cls}
      onClick={onActivate}
      role={mode === "link" ? "link" : "button"}
      tabIndex={0}
      aria-expanded={mode === "expand" ? !!open : undefined}
      onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), onActivate())}
    >
      <span className="tm">{fmtClock(r.timestamp)}</span>
      <PurposeTag r={r} t={t} />
      <div className="what">
        {attempt ? (
          <div className="l1">{t("live.feed.attempt", { index: attempt.index, total: attempt.total })}</div>
        ) : (
          <>
            <div className="l1">
              <SessionChip r={r} session={session} t={t} />
              <b>{rowTitle(r, session, t)}</b>
            </div>
            <div className="l2">
              <Story r={r} t={t} />
            </div>
          </>
        )}
      </div>
      <div className="rt">
        <div className="l1">
          <span className="m">{r.virtual_model}</span>
          <ArrowRight size={12} />
          <ProviderLogo iconId={sub?.provider_icon} size={16} />
          <span className="s">{sub?.display_name ?? customProviderLabel(r.provider_id, t) ?? r.provider_id}</span>
        </div>
        <div className="l2">
          {ok ? (
            r.retry_count > 0 && !attempt && mode === "expand" ? (
              <span className="w">{t("live.feed.recovered")}</span>
            ) : (
              <>
                {r.retry_count > 0 && mode === "link" && <span className="w">{t("live.feed.recovered")} · </span>}
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
        <div className="l2" title={timingTitle(r, t)}>
          {slowNetwork(r) ? (
            <>
              <span className="net">{t("live.feed.network", { s: fmtMs(r.network_ms) })}</span>
              {` · ${fmtMs(r.total_ms)}`}
            </>
          ) : ok ? (
            t("live.feed.timing", { ttft: fmtMs(r.ttft_ms), total: fmtMs(r.total_ms) })
          ) : (
            fmtMs(r.total_ms)
          )}
        </div>
      </div>
      {expanded && (
        <div className="more" onClick={(e) => e.stopPropagation()}>
          <div><small>{t("live.more.tokens")}</small><span>{fmtTok(parts.input)} / {fmtTok(parts.cacheRead)} / {fmtTok(parts.cacheWrite)} / {fmtTok(parts.output)}</span></div>
          <div><small>{t("live.more.cache")}</small><span>{hit == null ? "—" : `${Math.round(hit * 100)}%`}</span></div>
          <div><small>{t("live.more.effort")}</small><span>{r.effort ?? "—"}</span></div>
          <div><small>{t("live.more.retry")}</small><span>{r.retry_count}</span></div>
          {r.error && <div className="err-line"><small>{t("live.more.error")}</small><span>{r.error}</span></div>}
          <div className="why">
            <b>{t("live.more.why")}</b>
            <span>{whyText(r, t)}</span>
            {p.guessed && <span className="guess">{t("live.more.guessed")}</span>}
            {moreAction}
          </div>
        </div>
      )}
    </div>
  );
}

/** 用途 chips + 会话 chips, 两页共用 */
export function PurposeChips<F extends string>(props: {
  filters: readonly F[];
  value: F;
  counts: Record<F, number>;
  onChange: (f: F) => void;
  sessions: { id: string; label: string }[];
  session: string | null;
  onSession: (id: string | null) => void;
  t: TFunction;
}) {
  const { filters, value, counts, onChange, sessions, session, onSession, t } = props;
  return (
    <div className="rq-bar">
      {filters.map((f) => (
        <button
          key={f}
          type="button"
          className={"rq-chip" + (value === f ? " on" : "") + (f === "fail" && counts[f] ? " err" : "")}
          onClick={() => onChange(f)}
        >
          {t(`live.filter.${f}`)}
          <b>{counts[f]}</b>
        </button>
      ))}
      {sessions.length > 0 && <span className="rq-gap" />}
      {sessions.length > 0 && (
        <button type="button" className={"rq-chip" + (session === null ? " on" : "")} onClick={() => onSession(null)}>
          {t("live.filter.allSessions")}
        </button>
      )}
      {sessions.map((s) => (
        <button key={s.id} type="button" className={"rq-chip" + (session === s.id ? " on" : "")} onClick={() => onSession(s.id)}>
          <i className="sd" style={{ background: sessionColor(s.id) }} />
          {s.label}
        </button>
      ))}
    </div>
  );
}

/** 网络耗时超过这个值时, 行尾的「首字」换成高亮的「网络」(列宽放不下三项) */
const SLOW_NETWORK_MS = 500;

function slowNetwork(r: RequestSummary): boolean {
  return r.network_ms != null && r.network_ms >= SLOW_NETWORK_MS;
}

function timingTitle(r: RequestSummary, t: TFunction): string {
  const parts = [t("live.feed.timing", { ttft: fmtMs(r.ttft_ms), total: fmtMs(r.total_ms) })];
  if (r.network_ms != null) parts.push(t("live.feed.network", { s: fmtMs(r.network_ms) }));
  return parts.join(" · ");
}
