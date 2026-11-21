/**
 * 请求详情整页 /request-logs/:id (设计稿 design/request-logs-v1.html ②), 取代原来的弹窗。
 * 通栏页面: 顶栏常驻 (返回 / 上一条下一条 / 复制), 正文在 .page-flow 里滚动, 右侧目录。
 *
 * 列表的筛选在 URL 里原样带过来: 返回时恢复列表, 「上一条 / 下一条」也在同一份筛选结果里移动。
 * 从实时路由跳来时这条可能还在批量写入队列里 (≤ 5 秒): 先显示「正在写入」, 每秒重试。
 */
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Link, useLocation, useNavigate, useParams } from "react-router";
import { useQuery } from "@tanstack/react-query";
import { ArrowLeft, ArrowRight, Check, ChevronDown, ChevronUp, Copy } from "lucide-react";
import { api } from "@/api/tauri";
import { ClientToolBadge } from "@/components/ClientToolBadge";
import { ProviderLogo } from "@/components/ProviderLogo";
import { Spinner } from "@/components/Spinner";
import { HeaderTable, type HeaderTag } from "@/components/request-detail/HeaderTable";
import { useLiveSessions } from "@/components/live/useLiveSessions";
import {
  errorLabel,
  PurposeTag,
  rowTitle,
  SessionChip,
  Story,
  ToolChips,
  whyText,
} from "@/components/requests/RequestRow";
import { REQUESTS_KEY } from "@/hooks/useRequests";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useT, type TFunction } from "@/i18n";
import { fmtNum, fmtRequestSeconds, fmtTime, fmtTokenSpeed } from "@/lib/format";
import { isReturnedToClient } from "@/lib/headerSources";
import {
  cacheHitRatio,
  fmtClock,
  fmtMs,
  fmtTok,
  isOk,
  partsTotal,
  purposeOf,
  sumTokens,
} from "@/lib/liveStats";
import { customProviderLabel, providerIconId } from "@/lib/providerLabels";
import { providerName as localProviderName } from "@/lib/providerText";
import { parseLogQuery, toFilters, toSearch } from "@/lib/requestLogQuery";
import type {
  HeaderPairs,
  OutboundHeaderRows,
  OutboundSource,
  RequestLogDto,
  RequestSummary,
  SubscriptionDto,
} from "@/types";
import { TOOL_NAME_TRUNCATED_MARKER } from "@/types";
import "@/components/requests/request-row.css";
import "@/components/request-detail/request-detail.css";

/** 刚从实时路由跳来时最多等这么久让日志落库 */
const PENDING_RETRY_MS = 15_000;

const SECTIONS = ["overview", "route", "perf", "purpose", "client", "session", "headers", "body"] as const;
type SectionId = (typeof SECTIONS)[number];

/** 「思考强度」: 客户端请求 → 实际发往上游 (来源) → 上游回显。四项全空返回 null。 */
function EffortChain({ r, t }: { r: RequestLogDto; t: TFunction }) {
  const { client_effort, effective_effort, effort_source, upstream_effort } = r;
  if (!client_effort && !effective_effort && !effort_source && !upstream_effort) return <span className="muted">—</span>;
  const sourceKey = `requestLogs.detail.effortSource.${effort_source ?? ""}`;
  const source = effort_source ? t(sourceKey) : "";
  return (
    <span className="rqd-chainline">
      <span className="pill tag">{t("requestLogs.detail.effortClient")} {client_effort || "—"}</span>
      <span className="muted">→</span>
      <span className={"pill tag" + (effort_source === "slot" ? " warn" : "")}>
        {t("requestLogs.detail.effortEffective")} {effective_effort || "—"}
        {source && source !== sourceKey ? ` · ${source}` : ""}
      </span>
      <span className="muted">→</span>
      <span className="pill tag">
        {upstream_effort ? `${t("requestLogs.detail.effortUpstream")} ${upstream_effort}` : t("requestLogs.detail.effortUpstreamNone")}
      </span>
    </span>
  );
}

function effortText(r: RequestLogDto): string {
  return `${r.client_effort ?? "—"} → ${r.effective_effort ?? "—"} (${r.effort_source ?? "—"}) → ${r.upstream_effort ?? "—"}`;
}

/** tool_use_names 是否被截断 (末尾 "…") */
function namesTruncated(raw?: string | null): boolean {
  if (!raw) return false;
  try {
    const v = JSON.parse(raw);
    return Array.isArray(v) && v.includes(TOOL_NAME_TRUNCATED_MARKER);
  } catch {
    return false;
  }
}

function Section(props: { id: SectionId; title: string; sub?: ReactNode; action?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <section className={"card rqd-sec " + (props.className ?? "")} id={`rqd-${props.id}`} data-section={props.id}>
      <div className="card-head">
        <span className="card-title">{props.title}</span>
        {props.sub && <span className="card-sub">{props.sub}</span>}
        {props.action && <span className="rqd-sec-action">{props.action}</span>}
      </div>
      <div className="card-body">{props.children}</div>
    </section>
  );
}

function Kv({ rows }: { rows: [string, ReactNode][] }) {
  return (
    <dl className="rqd-kv">
      {rows.map(([k, v]) => (
        <div key={k}>
          <dt>{k}</dt>
          <dd>{v}</dd>
        </div>
      ))}
    </dl>
  );
}

export function RequestDetailPage() {
  const { id = "" } = useParams();
  const { t, locale } = useT();
  const location = useLocation();
  const navigate = useNavigate();
  const listSearch = location.search;
  const logQuery = useMemo(() => parseLogQuery(new URLSearchParams(listSearch)), [listSearch]);
  const subs = useSubscriptions();
  const sessions = useLiveSessions();
  const flowRef = useRef<HTMLDivElement>(null);
  const [active, setActive] = useState<SectionId>("overview");
  const [copied, setCopied] = useState(false);
  const [openedAt] = useState(() => Date.now());

  const detail = useQuery({
    queryKey: [REQUESTS_KEY, "detail", id],
    queryFn: () => api.getRequestDetail(id),
    // 还没落库 (null) 时每秒重试, 最多 15 秒
    refetchInterval: (q) => (q.state.data === null && Date.now() - openedAt < PENDING_RETRY_MS ? 1_000 : false),
  });
  const data = detail.data ?? null;
  const request = data?.request;
  const r = request?.summary;

  const neighbors = useQuery({
    queryKey: [REQUESTS_KEY, "neighbors", id, listSearch],
    queryFn: () => api.getRequestNeighbors(id, toFilters(logQuery, Date.now())),
    enabled: !!request,
  });
  const headersQuery = useQuery({
    queryKey: ["requestHeaders", id],
    queryFn: () => api.getRequestHeaders(id),
    enabled: !!request,
    staleTime: Infinity,
  });
  const sessionRows = useQuery({
    queryKey: [REQUESTS_KEY, "session", r?.session?.id, r?.timestamp],
    queryFn: () => api.listSessionRequests(r!.session!.id, r!.timestamp, 4),
    enabled: !!r?.session,
  });

  const backTo = `/request-logs${listSearch}`;
  const go = (target: string | null | undefined) => target && navigate(`/request-logs/${target}${listSearch}`);

  // 换一条请求时回到顶部
  useEffect(() => {
    flowRef.current?.scrollTo({ top: 0 });
    setActive("overview");
  }, [id]);

  // j / k 切换上一条下一条, Esc 返回列表 (焦点在输入框里时不抢)
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null;
      if (el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable)) return;
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      if (e.key === "k") go(neighbors.data?.newer);
      else if (e.key === "j") go(neighbors.data?.older);
      else if (e.key === "Escape") navigate(backTo);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  // 目录高亮: 滚动容器里最靠上的可见分区
  useEffect(() => {
    const root = flowRef.current;
    if (!root || !request) return;
    const io = new IntersectionObserver(
      (entries) => {
        const top = entries
          .filter((e) => e.isIntersecting)
          .sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top)[0];
        const sec = top?.target.getAttribute("data-section") as SectionId | null;
        if (sec) setActive(sec);
      },
      { root, rootMargin: "0px 0px -65% 0px" },
    );
    root.querySelectorAll("[data-section]").forEach((el) => io.observe(el));
    return () => io.disconnect();
  }, [request]);

  const subsById = useMemo(() => new Map((subs.data ?? []).map((s) => [s.id, s])), [subs.data]);
  const headers = headersQuery.data;

  const bar = (title: ReactNode, tags?: ReactNode) => (
    <div className="page-bar">
      <div className="rqd-col page-bar-lead">
        <Link className="btn ghost sm rqd-back" to={backTo} title={t("requestLogs.page.back")}>
          <ArrowLeft size={14} />
          <span>{t("requestLogs.title")}</span>
        </Link>
        <span className="sep" />
        <h1>{title}</h1>
        {tags}
        {request && (
          <div className="page-bar-right">
            <button
              type="button"
              className="btn ghost sm"
              disabled={!neighbors.data?.newer}
              onClick={() => go(neighbors.data?.newer)}
              title={`${t("requestLogs.page.newer")} (k)`}
            >
              <ChevronUp size={14} />
              {t("requestLogs.page.newer")}
            </button>
            <button
              type="button"
              className="btn ghost sm"
              disabled={!neighbors.data?.older}
              onClick={() => go(neighbors.data?.older)}
              title={`${t("requestLogs.page.older")} (j)`}
            >
              <ChevronDown size={14} />
              {t("requestLogs.page.older")}
            </button>
            <button className="btn sm" type="button" onClick={() => void copyAll()}>
              {copied ? <Check size={12} /> : <Copy size={12} />}
              {copied ? t("common.copied") : t("requestLogs.detail.copy")}
            </button>
          </div>
        )}
      </div>
    </div>
  );

  if (!request || !r) {
    const pending = detail.isLoading || (data === null && Date.now() - openedAt < PENDING_RETRY_MS);
    return (
      <>
        {bar(t("requestLogs.detail.title"))}
        <div className="page-flow">
          <div className="page-flow-pad">
            <div className="rqd-col rqd-state">
              {detail.isError ? (
                <span>{t("requestLogs.page.loadFailed")}</span>
              ) : pending ? (
                <>
                  <Spinner size={16} />
                  <span>{detail.isLoading ? t("common.loading") : t("requestLogs.page.pending")}</span>
                </>
              ) : (
                <span>{t("requestLogs.page.notFound")}</span>
              )}
              <Link className="btn sm" to={backTo}>{t("requestLogs.page.back")}</Link>
            </div>
          </div>
        </div>
      </>
    );
  }

  const sub = subsById.get(request.subscription_id);
  const providerName =
    (sub && localProviderName(sub, locale)) ?? customProviderLabel(request.provider_id, t) ?? request.provider_id;
  const sessionInfo = r.session ? sessions.get(r.session.id) : undefined;
  const attempts = data!.attempts;
  const attemptIndex = Math.max(0, attempts.findIndex((a) => a.id === r.id));
  const parts = sumTokens([r]);
  const tokenTotal = partsTotal(parts);
  const hit = cacheHitRatio(parts);
  const ok = isOk(r);
  const p = purposeOf(r);
  const prettyBody = (() => {
    if (!request.upstream_response_body) return null;
    try {
      return JSON.stringify(JSON.parse(request.upstream_response_body), null, 2);
    } catch {
      return request.upstream_response_body;
    }
  })();

  async function copyAll() {
    if (!request || !r) return;
    const lines = [
      `request_id: ${request.id}`,
      `trace_id: ${r.trace_id ?? "—"}`,
      `time: ${fmtTime(request.timestamp)}`,
      `purpose: ${r.purpose ? `${r.purpose.kind}${r.purpose.guessed ? " (guessed)" : ""} turn=${r.purpose.turn ?? "—"} step=${r.purpose.step ?? "—"} prev_tools=${r.purpose.prev_tools.join(",") || "—"}` : "—"}`,
      `session: ${r.session ? `${r.session.client} ${r.session.id}` : "—"}`,
      `virtual_model: ${request.virtual_model_name}`,
      `provider: ${customProviderLabel(request.provider_id, t) ?? request.provider_id}`,
      `subscription: ${sub?.display_name ?? request.subscription_id}`,
      `real_model: ${request.real_model_name}`,
      `response_model: ${request.response_model_name ?? "—"}`,
      `effort: ${effortText(request)}`,
      `status: ${request.status}`,
      `http_status: ${request.http_status ?? "—"}`,
      `attempt: ${attemptIndex + 1} / ${attempts.length}`,
      `total_latency: ${fmtRequestSeconds(request.total_latency_ms)}`,
      `first_content: ${fmtRequestSeconds(request.first_content_ms)}`,
      `output_speed: ${fmtTokenSpeed(request.output_tokens_per_second)}`,
      `tokens: input=${request.input_tokens ?? "—"} output=${request.output_tokens ?? "—"} cache_read=${request.cache_read_tokens ?? "—"} cache_write=${request.cache_creation_tokens ?? "—"}`,
      `stop_reason: ${request.stop_reason ?? "—"}`,
      `tools_offered: ${request.tools_offered_count ?? "—"} · tool_results: ${request.tool_result_count ?? "—"} · tool_uses: ${request.tool_use_count ?? "—"}`,
      `tool_use_names: ${request.tool_use_names ?? "—"}`,
      `client: ${request.client_tool ?? "—"} ${request.client_version ?? ""} · ${request.entry_kind ? `/v1/${request.entry_kind}` : "—"} · ${request.downstream_http_version ?? "—"} · ${request.client_ip ?? "—"}`,
      `user_agent: ${request.client_user_agent ?? "—"}`,
      `error_message: ${request.error_message ?? ""}`,
      "",
      "upstream_response_body:",
      prettyBody ?? "",
    ];
    const section = (name: string, pairs: HeaderPairs | OutboundHeaderRows | null | undefined) => {
      if (!pairs) return;
      lines.push("", `${name}:`, ...pairs.map(([k, v, src]) => (src ? `${k}: ${v}  [${src}]` : `${k}: ${v}`)));
    };
    section("client_request_headers", headers?.client_request);
    section("upstream_request_headers", headers?.upstream_request);
    section("upstream_response_headers", headers?.upstream_response);
    try {
      await navigator.clipboard.writeText(lines.join("\n"));
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // 剪贴板不可用 (非安全上下文) 时静默
    }
  }

  // 请求头三张表的来源 / 回传标签 (与原弹窗相同)
  const outboundTag = ([, , src]: [string, string, OutboundSource]): HeaderTag => {
    const tone = src === "forwarded" ? "ok" : src === "cc_router" ? "muted" : "accent";
    return { label: t(`requestLogs.detail.headers.source.${src}`), tone };
  };
  const showReturned = sub?.auth_type === "api_key"; // 回传白名单只作用于 Anthropic 透传路径
  const responseTag = ([name]: [string, string]): HeaderTag | null =>
    showReturned
      ? isReturnedToClient(name)
        ? { label: t("requestLogs.detail.headers.returned"), tone: "ok" }
        : { label: t("requestLogs.detail.headers.notReturned"), tone: "muted" }
      : null;
  const recorded = headers?.client_request != null;
  const emptyText = (kind: "client" | "request" | "response") =>
    headersQuery.isError
      ? t("requestLogs.detail.headers.loadFailed")
      : !recorded || kind === "client"
        ? t("requestLogs.detail.headers.notRecorded")
        : kind === "request" || headers?.upstream_request == null
          ? t("requestLogs.detail.headers.notSent")
          : t("requestLogs.detail.headers.noResponse");
  const headerCount = (pairs: HeaderPairs | OutboundHeaderRows | null | undefined) => pairs?.length ?? 0;
  const headerTotal =
    headerCount(headers?.client_request) + headerCount(headers?.upstream_request) + headerCount(headers?.upstream_response);

  const mono = (v?: string | null) => (v ? <span className="mono">{v}</span> : <span className="muted">—</span>);
  const num = (v?: number | null) => (v != null ? <span className="mono tnum">{v}</span> : <span className="muted">—</span>);
  const toc: [SectionId, string, number | null][] = [
    ["overview", t("requestLogs.page.sec.overview"), null],
    ["route", t("requestLogs.page.sec.route"), attempts.length > 1 ? attempts.length : null],
    ["perf", t("requestLogs.page.sec.perf"), null],
    ["purpose", t("requestLogs.page.sec.purpose"), null],
    ["client", t("requestLogs.page.sec.client"), null],
    ["session", t("requestLogs.page.sec.session"), sessionRows.data?.length ?? null],
    ["headers", t("requestLogs.page.sec.headers"), headers ? headerTotal : null],
    ["body", t("requestLogs.page.sec.body"), null],
  ];
  const jump = (sec: SectionId) => {
    document.getElementById(`rqd-${sec}`)?.scrollIntoView({ behavior: "smooth", block: "start" });
    setActive(sec);
  };

  return (
    <>
      {bar(
        <span title={rowTitle(r, sessionInfo, t)}>{rowTitle(r, sessionInfo, t)}</span>,
        <>
          <span className={`pill ${ok ? "ok" : r.status === "timeout" ? "warn" : "err"} shrink-0`}>
            <span className="dot" />
            {t(`requestLogs.status.${request.status}`)}
            {request.http_status != null ? ` ${request.http_status}` : ""}
          </span>
          <span className="pill tag shrink-0">
            {request.is_streaming ? t("requestLogs.detail.streaming") : t("requestLogs.detail.nonStreaming")}
          </span>
          {attempts.length > 1 && (
            <span className="pill warn shrink-0">{t("requestLogs.page.failover")}</span>
          )}
        </>,
      )}

      <div className="page-flow" ref={flowRef}>
        <div className="page-flow-pad">
          <div className="rqd-col rqd-grid">
            <div className="rqd-main">
              {/* ---------- 概览 ---------- */}
              <section className="card rqd-hero" id="rqd-overview" data-section="overview">
                <div className="rqd-hero-lead">
                  <div className="rqd-hero-tags">
                    <PurposeTag r={r} t={t} />
                    <SessionChip r={r} session={sessionInfo} t={t} />
                    {p.guessed && <span className="rqd-guess">{t("live.more.guessed")}</span>}
                  </div>
                  <div className="rqd-story">
                    <Story r={r} t={t} />
                  </div>
                  <div className="rqd-meta">
                    <span className="mono">{fmtTime(request.timestamp)}</span>
                    <ClientToolBadge toolId={request.client_tool} userAgent={request.client_user_agent} />
                    <span className="mono" title={request.id}>req {request.id.slice(0, 8)}</span>
                  </div>
                  {!ok && r.error && <div className="rqd-error">{errorLabel(r, t)} · {r.error}</div>}
                </div>
                <div className="rqd-stats">
                  <div><small>Token</small><b>{fmtTok(tokenTotal)}</b></div>
                  <div title={t("requestLogs.firstContentHint")}><small>{t("requestLogs.col.firstContent")}</small><b>{fmtMs(r.ttft_ms)}</b></div>
                  <div><small>{t("requestLogs.detail.latency")}</small><b>{fmtMs(r.total_ms)}</b></div>
                  <div title={t("requestLogs.speedHint")}><small>{t("requestLogs.col.speed")}</small><b>{fmtTokenSpeed(request.output_tokens_per_second)}</b></div>
                </div>
              </section>

              {/* ---------- 路由 ---------- */}
              <Section
                id="route"
                title={t("requestLogs.page.sec.route")}
                sub={
                  attempts.length > 1
                    ? t("requestLogs.page.route.summary", { total: attempts.length, index: attemptIndex + 1 })
                    : r.trace_id
                      ? t("requestLogs.page.route.single")
                      : t("requestLogs.page.route.legacy")
                }
              >
                <div className="rqd-chain">
                  <div className="rqd-node">
                    <small>{t("requestLogs.detail.clientTool")}</small>
                    <div className="n"><ClientToolBadge toolId={request.client_tool} userAgent={request.client_user_agent} /></div>
                    <div className="d">{request.entry_kind ? `/v1/${request.entry_kind}` : "—"} · {request.downstream_http_version ?? "—"}</div>
                  </div>
                  <span className="rqd-arrow"><ArrowRight size={14} /></span>
                  <div className="rqd-node">
                    <small>{t("requestLogs.detail.virtualModel")}</small>
                    <div className="n mono">{request.virtual_model_name}</div>
                  </div>
                  {attempts.map((a, i) => (
                    <AttemptNode
                      key={a.id}
                      a={a}
                      index={i}
                      total={attempts.length}
                      current={a.id === r.id}
                      sub={subsById.get(a.subscription_id)}
                      prevFailed={i > 0}
                      onOpen={() => go(a.id)}
                      t={t}
                    />
                  ))}
                  <span className="rqd-arrow"><ArrowRight size={14} /></span>
                  <div className="rqd-node">
                    <small>{t("requestLogs.page.route.upstream")}</small>
                    <div className="n mono">{request.real_model_name}</div>
                    <div className="d">
                      {request.response_model_name
                        ? t("requestLogs.page.route.echo", { model: request.response_model_name })
                        : "—"}
                    </div>
                  </div>
                </div>
                <Kv
                  rows={[
                    [t("requestLogs.detail.subscription"), sub?.display_name ?? providerName],
                    [t("requestLogs.detail.provider"), (
                      <span className="rqd-provider">
                        <ProviderLogo iconId={sub?.provider_icon ?? providerIconId(request.provider_id)} size={18} iconSize={12} />
                        {providerName}
                      </span>
                    )],
                    [t("requestLogs.detail.realModel"), mono(request.real_model_name)],
                    [t("requestLogs.detail.responseModel"), mono(request.response_model_name)],
                  ]}
                />
              </Section>

              {/* ---------- 耗时与 token ---------- */}
              <Section id="perf" title={t("requestLogs.page.sec.perf")}>
                <TimingBar r={r} request={request} t={t} />
                <div className="rqd-tok">
                  {tokenTotal > 0 && (
                    <div className="rqd-tok-bar">
                      <i style={{ flexGrow: parts.cacheRead, background: "var(--rqd-tk-cr)" }} />
                      <i style={{ flexGrow: parts.cacheWrite, background: "var(--rqd-tk-cw)" }} />
                      <i style={{ flexGrow: parts.input, background: "var(--rqd-tk-in)" }} />
                      <i style={{ flexGrow: parts.output, background: "var(--rqd-tk-out)" }} />
                    </div>
                  )}
                  <div className="rqd-tok-list">
                    <div><small><i style={{ background: "var(--rqd-tk-cr)" }} />{t("stats.daily.tokenTooltipCacheRead")}</small><b>{fmtNum(parts.cacheRead)}</b></div>
                    <div><small><i style={{ background: "var(--rqd-tk-cw)" }} />{t("stats.daily.tokenTooltipCacheCreate")}</small><b>{fmtNum(parts.cacheWrite)}</b></div>
                    <div><small><i style={{ background: "var(--rqd-tk-in)" }} />{t("requestLogs.page.perf.uncached")}</small><b>{fmtNum(parts.input)}</b></div>
                    <div><small><i style={{ background: "var(--rqd-tk-out)" }} />{t("stats.daily.tokenTooltipOutput")}</small><b>{fmtNum(parts.output)}</b></div>
                  </div>
                  <p className="rqd-note">
                    {hit != null && `${t("requestLogs.page.perf.cacheHit", { pct: Math.round(hit * 100) })} · `}
                    {t("requestLogs.page.perf.note")}
                  </p>
                </div>
              </Section>

              <div className="rqd-two">
                {/* ---------- 在做什么 ---------- */}
                <Section id="purpose" title={t("requestLogs.page.sec.purpose")}>
                  {r.purpose === null ? (
                    <p className="rqd-note">{t("requestLogs.page.purpose.legacy")}</p>
                  ) : (
                    <>
                      <Kv
                        rows={[
                          [t("requestLogs.page.purpose.kind"), <>{t(`live.purpose.${p.kind}`)}{p.guessed && <span className="rqd-guess">{t("live.more.guessed")}</span>}</>],
                          [t("requestLogs.page.purpose.turnStep"), p.turn && p.step ? t("live.title.turnStep", { turn: p.turn, step: p.step }) : "—"],
                          [t("requestLogs.page.purpose.prevTools"), p.prev_tools.length ? <ToolChips names={p.prev_tools} t={t} /> : "—"],
                        ]}
                      />
                      <div className="rqd-why"><b>{t("live.more.why")}</b> {whyText(r, t)}</div>
                    </>
                  )}
                  <Kv
                    rows={[
                      [t("requestLogs.page.purpose.calls"), r.tool_use_names.length ? (
                        <>
                          <ToolChips names={r.tool_use_names} t={t} />
                          {namesTruncated(request.tool_use_names) && <span className="field-hint"> {t("requestLogs.detail.tools.truncated")}</span>}
                        </>
                      ) : request.tool_use_count === 0 ? t("requestLogs.detail.tools.none") : "—"],
                      [t("requestLogs.detail.stopReason"), mono(request.stop_reason)],
                      [t("requestLogs.detail.tools.title"), (
                        <span className="rqd-counts">
                          <span>{t("requestLogs.detail.tools.offered")} {num(request.tools_offered_count)}</span>
                          <span>{t("requestLogs.detail.tools.results")} {num(request.tool_result_count)}</span>
                          <span>{t("requestLogs.detail.tools.used")} {num(request.tool_use_count)}</span>
                        </span>
                      )],
                    ]}
                  />
                </Section>

                {/* ---------- 思考强度与客户端 ---------- */}
                <Section id="client" title={t("requestLogs.page.sec.client")}>
                  <Kv
                    rows={[
                      [t("requestLogs.detail.effort"), <EffortChain r={request} t={t} />],
                      [t("requestLogs.detail.clientTool"), <ClientToolBadge toolId={request.client_tool} userAgent={request.client_user_agent} />],
                      [t("requestLogs.detail.clientVersion"), mono(request.client_version)],
                      [t("requestLogs.detail.entryKind"), mono(request.entry_kind ? `/v1/${request.entry_kind}` : null)],
                      [t("requestLogs.detail.httpVersion"), mono(request.downstream_http_version)],
                      [t("requestLogs.detail.clientIp"), mono(request.client_ip)],
                      [t("requestLogs.detail.userAgent"), mono(request.client_user_agent)],
                      [t("requestLogs.page.client.session"), r.session ? (
                        <span className="mono">{r.session.id}{sessionInfo?.project ? ` · ${sessionInfo.project}` : ""}</span>
                      ) : "—"],
                    ]}
                  />
                </Section>
              </div>

              {/* ---------- 同会话 ---------- */}
              <Section
                id="session"
                title={t("requestLogs.page.sec.session")}
                sub={r.session ? `${sessionInfo?.project ?? r.session.client} · ${r.session.id.slice(0, 4)}` : undefined}
                action={
                  r.session ? (
                    <Link className="lv-link" to={`/request-logs?${toSearch({ ...parseLogQuery(new URLSearchParams()), session: r.session.id })}`}>
                      {t("requestLogs.page.session.all")} <ArrowRight size={12} />
                    </Link>
                  ) : undefined
                }
              >
                {!r.session ? (
                  <p className="rqd-note">{t("requestLogs.page.session.none")}</p>
                ) : (
                  <div className="rqd-slist">
                    {(sessionRows.data ?? []).map((s) => (
                      <button
                        key={s.id}
                        type="button"
                        className={"rqd-srow" + (s.id === r.id ? " cur" : "") + (isOk(s) ? "" : " err")}
                        onClick={() => s.id !== r.id && go(s.id)}
                        aria-current={s.id === r.id ? "true" : undefined}
                      >
                        <span className="tm">{fmtClock(s.timestamp)}</span>
                        <PurposeTag r={s} t={t} />
                        <span className="what">
                          <b>{rowTitle(s, sessionInfo, t)}</b>
                          <span className="muted"> · <Story r={s} t={t} /></span>
                        </span>
                        <span className="muted rt">
                          {subsById.get(s.subscription_id)?.display_name ?? s.provider_id} · {isOk(s) ? fmtMs(s.total_ms) : errorLabel(s, t)}
                        </span>
                      </button>
                    ))}
                  </div>
                )}
              </Section>

              {/* ---------- 请求头 ---------- */}
              <Section id="headers" title={t("requestLogs.page.sec.headers")} sub={t("requestLogs.page.headers.sub")}>
                <HeaderTabs
                  t={t}
                  tabs={[
                    {
                      key: "client",
                      label: t("requestLogs.detail.nav.clientHeaders"),
                      count: headers ? headerCount(headers.client_request) : null,
                      node: (
                        <HeaderTable
                          title={t("requestLogs.detail.nav.clientHeaders")}
                          subtitle={t("requestLogs.detail.headers.clientSub")}
                          pairs={headers?.client_request}
                          loading={headersQuery.isLoading}
                          emptyText={emptyText("client")}
                        />
                      ),
                    },
                    {
                      key: "upstream",
                      label: t("requestLogs.detail.nav.upstreamHeaders"),
                      count: headers ? headerCount(headers.upstream_request) : null,
                      node: (
                        <>
                          <HeaderTable
                            title={t("requestLogs.detail.nav.upstreamHeaders")}
                            subtitle={t("requestLogs.detail.headers.upstreamSub")}
                            pairs={headers?.upstream_request}
                            loading={headersQuery.isLoading}
                            emptyText={emptyText("request")}
                            tagFor={outboundTag}
                          />
                          {headers?.upstream_request && headers.upstream_request.length > 0 && (
                            <p className="rqd-note">{t("requestLogs.detail.headers.sourceHint")}</p>
                          )}
                        </>
                      ),
                    },
                    {
                      key: "response",
                      label: t("requestLogs.detail.nav.responseHeaders"),
                      count: headers ? headerCount(headers.upstream_response) : null,
                      node: (
                        <HeaderTable
                          title={t("requestLogs.detail.nav.responseHeaders")}
                          subtitle={t("requestLogs.detail.headers.responseSub")}
                          pairs={headers?.upstream_response}
                          loading={headersQuery.isLoading}
                          emptyText={emptyText("response")}
                          tagFor={responseTag}
                        />
                      ),
                    },
                  ]}
                />
              </Section>

              {/* ---------- 响应 ---------- */}
              <Section id="body" title={t("requestLogs.page.sec.body")} sub={ok ? t("requestLogs.page.body.okSub") : undefined}>
                {!ok && request.error_message && <div className="alert err rqd-body-error mono">{request.error_message}</div>}
                {prettyBody ? (
                  <pre className="mono rqd-pre">{prettyBody}</pre>
                ) : (
                  <p className="rqd-note">{t("requestLogs.detail.noBody")}</p>
                )}
                {ok && attempts.some((a) => !isOk(a)) && (
                  <p className="rqd-note">{t("requestLogs.page.body.seeAttempts")}</p>
                )}
              </Section>
            </div>

            <nav className="rqd-toc" aria-label={t("requestLogs.page.toc")}>
              <small>{t("requestLogs.page.toc")}</small>
              {toc.map(([sec, label, n]) => (
                <button key={sec} type="button" className={active === sec ? "on" : ""} onClick={() => jump(sec)}>
                  <span>{label}</span>
                  {n != null && n > 0 && <b>{n}</b>}
                </button>
              ))}
            </nav>
          </div>
        </div>
      </div>
    </>
  );
}

function AttemptNode(props: {
  a: RequestSummary;
  index: number;
  total: number;
  current: boolean;
  sub: SubscriptionDto | undefined;
  prevFailed: boolean;
  onOpen: () => void;
  t: TFunction;
}) {
  const { a, index, total, current, sub, prevFailed, onOpen, t } = props;
  const ok = isOk(a);
  const label =
    total > 1
      ? `${t("live.feed.attempt", { index: index + 1, total })}${current ? ` · ${t("requestLogs.page.route.current")}` : ""}`
      : t("requestLogs.detail.subscription");
  return (
    <>
      <span className={"rqd-arrow" + (prevFailed ? " fo" : "")}>
        {prevFailed && <small>{t("requestLogs.page.route.switch")}</small>}
        <ArrowRight size={14} />
      </span>
      <button
        type="button"
        className={"rqd-node attempt" + (ok ? "" : " bad") + (current ? " cur" : "")}
        onClick={current ? undefined : onOpen}
        disabled={current}
        title={current ? undefined : t("requestLogs.page.route.open")}
      >
        <small>{label}</small>
        <div className="n">
          <ProviderLogo iconId={sub?.provider_icon} size={16} />
          <span>{sub?.display_name ?? a.provider_id}</span>
        </div>
        <div className="d">
          {ok ? `${a.http_status ?? 200} · ${fmtMs(a.total_ms)}` : `${errorLabel(a, t)} · ${fmtMs(a.total_ms)}`}
        </div>
      </button>
    </>
  );
}

function TimingBar({ r, request, t }: { r: RequestSummary; request: RequestLogDto; t: TFunction }) {
  const total = r.total_ms;
  if (total == null || total <= 0) return <p className="rqd-note">{t("requestLogs.page.perf.noTiming")}</p>;
  const ttft = r.ttft_ms != null ? Math.min(r.ttft_ms, total) : null;
  const waitPct = ttft != null ? Math.max(4, Math.min(96, (ttft / total) * 100)) : 100;
  return (
    <div className="rqd-timing">
      <div className="rqd-tbar">
        <span className="wait" style={{ width: `${waitPct}%` }}>
          {ttft != null ? t("requestLogs.page.perf.wait", { s: fmtMs(ttft) }) : t("requestLogs.page.perf.whole", { s: fmtMs(total) })}
        </span>
        {ttft != null && (
          <span className="gen" style={{ width: `${100 - waitPct}%` }}>
            {t("requestLogs.page.perf.gen", { s: fmtMs(total - ttft) })}
            {request.output_tokens_per_second != null && ` · ${fmtTokenSpeed(request.output_tokens_per_second)}`}
          </span>
        )}
      </div>
      <div className="rqd-tlegend">
        <span>0</span>
        {ttft != null && <span>{t("requestLogs.page.perf.ttftMark", { s: fmtMs(ttft) })}</span>}
        <span>{t("requestLogs.page.perf.endMark", { s: fmtMs(total) })}</span>
      </div>
    </div>
  );
}

function HeaderTabs({ tabs, t }: { tabs: { key: string; label: string; count: number | null; node: ReactNode }[]; t: TFunction }) {
  const [cur, setCur] = useState(tabs[1]?.key ?? tabs[0].key);
  return (
    <div className="rqd-htabs">
      <div className="seg-tabs" role="tablist" aria-label={t("requestLogs.page.sec.headers")}>
        {tabs.map((tab) => (
          <button
            key={tab.key}
            type="button"
            role="tab"
            aria-selected={cur === tab.key}
            className={"seg-tab" + (cur === tab.key ? " active" : "")}
            onClick={() => setCur(tab.key)}
          >
            {tab.label}
            {tab.count != null && <b className="rqd-tab-n">{tab.count}</b>}
          </button>
        ))}
      </div>
      {tabs.find((tab) => tab.key === cur)?.node}
    </div>
  );
}
