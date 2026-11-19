import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router";
import { agentNames, liveSessionQueryKey, useAgentMonitor, type AgentClient } from "@/hooks/useAgentMonitor";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useT, type TFunction } from "@/i18n";
import { fmtCompact, fmtNum } from "@/lib/format";
import { isTauriHost, runtime } from "@/runtime";

interface Usage {
  input: number;
  output: number;
  cache_read: number;
  cache_write: number;
  reasoning: number | null;
  requests: number | null;
}

interface Activity {
  kind: string;
  tool: string | null;
  detail: string | null;
  at_ms: number;
  duration_ms: number | null;
  agent_type: string | null;
  context_before: number | null;
  context_after: number | null;
}

interface Session {
  client: AgentClient;
  id: string;
  cwd: string | null;
  permission: string | null;
  model: string | null;
  status: string;
  at_ms: number;
  turn_started_ms: number | null;
  turn_finished_ms: number | null;
  context: {
    used: number;
    window: number;
    percent: number;
    approximate: boolean;
    at_ms: number;
  } | null;
  cache: {
    hit_ratio: number | null;
    expires_at: number | null;
    warm: boolean | null;
    requests: number | null;
    misses: number | null;
  } | null;
  limits: { window: string; minutes: number | null; used_percentage: number | null; resets_at: number | null }[];
  observed_usage: Usage | null;
  cumulative_usage: Usage | null;
  proxy_usage: Usage | null;
  route: { provider_id: string; subscription_id: string; virtual_model: string; model: string } | null;
  agents: { id: string; kind: string | null; running: boolean }[];
  activities: Activity[];
}

const sessionKey = (row: Session) => `${row.client}:${row.id}`;
const translate = (t: TFunction, key: string) => t(`liveSessions.${key}`);
const time = (ms: number) => new Date(ms).toLocaleTimeString();
const duration = (ms: number) => ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(1)} s`;

function clientStatus(t: TFunction, session: Session) {
  return translate(t, `event.${session.status}`);
}

export function LiveSessionSection({ variant }: { variant: "classic" | "sketch" }) {
  return isTauriHost ? <DesktopSessions variant={variant} /> : null;
}

function DesktopSessions({ variant }: { variant: "classic" | "sketch" }) {
  const { t } = useT();
  const [selected, setSelected] = useState<string>();
  const connections = useAgentMonitor(3_000);
  const enabled = connections.data?.filter((row) => row.connection === "enabled") ?? [];
  const loaded = useQuery({
    queryKey: liveSessionQueryKey,
    queryFn: () => runtime.invoke<Session[]>("get_live_sessions"),
    enabled: enabled.length > 0,
    refetchInterval: 850,
  });
  const rows = loaded.data?.filter((row) => enabled.some((connection) => connection.client === row.client)) ?? [];
  const active = rows.find((row) => sessionKey(row) === selected) ?? rows[0];
  const sketch = variant === "sketch";
  const failure = connections.error ?? loaded.error;
  const errorMessage = typeof failure === "object" && failure && "message" in failure
    ? String(failure.message)
    : String(failure ?? "");

  return (
    <section className={sketch ? "card alt live-sessions" : "lrc-flush-section live-sessions"}>
      <header className={sketch ? "card-head" : "lrc-flush-title"}>
        <span className={sketch ? "card-title" : undefined}>{translate(t, "title")}</span>
        {rows.length > 0 && <span className="session-muted">{t("liveSessions.count", { count: rows.length })}</span>}
      </header>
      <div className={sketch ? "card-body" : undefined}>
        <Link className="session-management-link" to="/agent-monitor">{t("agentMonitor.manage")}</Link>
        {failure != null && <p role="alert" className="session-error">{errorMessage || translate(t, "error")}</p>}
        {active ? (
          <div className="session-layout">
            <div className="session-picker" role="group" aria-label={translate(t, "title")}>
              {rows.map((row) => (
                <button type="button" key={sessionKey(row)} aria-pressed={row === active} onClick={() => setSelected(sessionKey(row))}>
                  <strong>{agentNames[row.client]}</strong>
                  <span className="session-muted">{row.cwd?.split(/[\\/]/).filter(Boolean).at(-1) ?? "—"} · {row.id.slice(0, 8)}</span>
                  <span data-state={row.status}>{clientStatus(t, row)}</span>
                </button>
              ))}
            </div>
            <SessionDetails key={sessionKey(active)} row={active} />
          </div>
        ) : <p className="session-muted">{translate(t, enabled.length === 0 ? "disabled" : "waiting")}</p>}
      </div>
    </section>
  );
}

function UsageNumbers({ usage, t }: { usage: Usage; t: TFunction }) {
  return (
    <>
      <strong>{t("liveSessions.tokenPair", { input: fmtCompact(usage.input), output: fmtCompact(usage.output) })}</strong>
      <small className="session-muted">
        {t("liveSessions.cachePair", { read: fmtCompact(usage.cache_read), write: fmtCompact(usage.cache_write) })}
        {usage.requests !== null && ` · ${t("liveSessions.requests", { count: fmtNum(usage.requests) })}`}
        {usage.reasoning !== null && ` · ${t("liveSessions.reasoning", { count: fmtCompact(usage.reasoning) })}`}
      </small>
    </>
  );
}

function SessionDetails({ row }: { row: Session }) {
  const { t } = useT();
  const subscriptions = useSubscriptions();
  const [expanded, setExpanded] = useState(false);
  const context = row.context;
  const elapsed = row.turn_started_ms === null
    ? null
    : (row.turn_finished_ms ?? (row.status === "unknown" ? row.at_ms : Date.now())) - row.turn_started_ms;
  const usage = row.cumulative_usage ?? row.observed_usage ?? row.proxy_usage;
  const usageKey = row.cumulative_usage ? "clientTotal" : row.observed_usage ? "clientObserved" : "proxyObserved";
  const route = row.route;
  const subscription = subscriptions.data?.find((sub) => sub.id === route?.subscription_id);
  const cache = row.cache;
  return (
    <div className="session-details">
      <div className="session-heading">
        <strong>{agentNames[row.client]} <span className="session-muted">· {row.id.slice(0, 8)}</span></strong>
        <span className="session-state" data-state={row.status}>{clientStatus(t, row)}</span>
      </div>
      <p className="session-muted session-cwd">{row.cwd ?? "—"}{row.model && ` · ${row.model}`}{row.permission && ` · ${row.permission}`}{elapsed !== null && ` · ${t("liveSessions.turnTime", { duration: duration(Math.max(0, elapsed)) })}`}</p>
      <p className="session-route">
        {route ? `${route.virtual_model} → ${subscription?.display_name ?? route.provider_id} → ${route.model}` : translate(t, "unlinked")}
      </p>
      <div className="session-metrics">
        <div className="session-metric">
          <small className="session-muted">{translate(t, "context")}</small>
          {context ? (
            <>
              <strong title={`${fmtNum(context.used)} / ${fmtNum(context.window)}`}>
                {context.approximate ? "≈ " : ""}{fmtCompact(context.used)} / {fmtCompact(context.window)}
              </strong>
              <progress max={100} value={context.percent} aria-label={translate(t, "context")} />
              <small className="session-muted">{Math.round(context.percent)}% · {translate(t, context.approximate ? "codexSource" : "claudeSource")} · {time(context.at_ms)}</small>
            </>
          ) : <small>{translate(t, "contextUnavailable")}</small>}
        </div>
        <div className="session-metric">
          <small className="session-muted">{translate(t, usage ? usageKey : "usage")}</small>
          {usage ? <UsageNumbers usage={usage} t={t} /> : <small>{translate(t, "unavailable")}</small>}
        </div>
      </div>
      <div className="session-heading">
        <strong>{translate(t, "activity")}</strong>
        <small className="session-muted">{t("liveSessions.updated", { time: time(row.at_ms) })}</small>
      </div>
      <ul className="session-timeline">
        {row.activities.slice(0, expanded ? undefined : 6).map((item, idx) => {
          const suffix = item.detail === "auto" || item.detail === "manual" || item.detail === "resume" || item.detail === "clear" || item.detail === "fork" || item.detail === "startup"
            ? translate(t, item.detail) : item.detail;
          return (
            <li key={`${item.at_ms}-${idx}`}>
              <time className="session-muted" dateTime={new Date(item.at_ms).toISOString()}>{time(item.at_ms)}</time>
              <span>{item.tool && <><strong>{item.tool}</strong> · </>}{translate(t, `event.${item.kind}`)}
                {suffix && ` · ${suffix}`}
                {item.agent_type && ` · ${item.agent_type}`}
                {item.duration_ms !== null && ` · ${duration(item.duration_ms)}`}
                {item.context_before !== null && item.context_after !== null && ` · ${Math.round(item.context_before)}% → ${Math.round(item.context_after)}%`}
              </span>
            </li>
          );
        })}
      </ul>
      {row.activities.length > 6 && <button type="button" onClick={() => setExpanded(!expanded)}>{translate(t, expanded ? "collapse" : "expand")}</button>}
      <details className="session-diagnostics">
        <summary>{translate(t, "diagnostics")}</summary>
        {row.proxy_usage && usageKey !== "proxyObserved" && (
          <div className="session-metric session-extra"><small>{translate(t, "proxyObserved")}</small><UsageNumbers usage={row.proxy_usage} t={t} /></div>
        )}
        <h4>{translate(t, "agents")}</h4>
        {row.agents.length ? (
          <ul className="session-bullets">
            {row.agents.map((agent) => <li key={agent.id}>{agent.kind ?? "—"} · {agent.id.slice(0, 8)} · {translate(t, agent.running ? "event.running" : "event.agent_finished")}</li>)}
          </ul>
        ) : <p className="session-muted">{translate(t, "unavailable")}</p>}
        <h4>{translate(t, "cache")}</h4>
        {cache ? (
          <ul className="session-bullets">
            {cache.warm !== null && <li>{translate(t, cache.warm ? "warm" : "cold")}</li>}
            {cache.hit_ratio !== null && <li>{t("liveSessions.hitRate", { percent: Math.round(cache.hit_ratio * 100) })}</li>}
            {cache.requests !== null && <li>{t("liveSessions.requests", { count: fmtNum(cache.requests) })}</li>}
            {cache.misses !== null && <li>{t("liveSessions.misses", { count: fmtNum(cache.misses) })}</li>}
            {cache.expires_at !== null && <li>{t("liveSessions.expires", { time: new Date(cache.expires_at * 1000).toLocaleString() })}</li>}
          </ul>
        ) : <p className="session-muted">{translate(t, "unavailable")}</p>}
        <h4>{translate(t, "limits")}</h4>
        {row.limits.length ? (
          <ul className="session-bullets">
            {row.limits.map((limit) => (
              <li key={limit.window}>
                {limit.minutes === null
                  ? translate(t, limit.window)
                  : t("liveSessions.limitMinutes", { minutes: fmtNum(limit.minutes) })}
                {limit.used_percentage !== null && ` · ${limit.used_percentage}%`}
                {limit.resets_at !== null && ` · ${t("liveSessions.resets", { time: new Date(limit.resets_at * 1000).toLocaleString() })}`}
              </li>
            ))}
          </ul>
        ) : <p className="session-muted">{translate(t, "unavailable")}</p>}
      </details>
    </div>
  );
}
