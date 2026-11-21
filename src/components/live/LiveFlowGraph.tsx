/**
 * 实时路由图: 客户端 → cc-router 内的虚拟模型 → 订阅。
 *
 * - 节点是普通 DOM (主题样式、文字折行都交给 CSS), 连线是覆盖在上面的 SVG,
 *   端点由节点实测位置算出 (ResizeObserver + 节点集合变化时重量)。
 * - 线宽 = 最近 60 秒这条路径上的请求量 (含在途); 最近 20 秒内发生过失败切换的线画成琥珀色虚线。
 * - 粒子: 每个 `route_attempt_started` 放一颗, 0.35s 到虚拟模型、0.45s 到订阅;
 *   切换重试从虚拟模型直接出发。动画循环只在有粒子时运行, 空闲时页面完全静止。
 * - 完成 (`live_request`): 订阅节点短闪 + 浮出 token 数 / 状态码。
 * 暂停或系统要求减少动态效果时不放粒子、不闪烁, 只更新数字与线宽。
 */
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { Ban } from "lucide-react";
import { ProviderLogo } from "@/components/ProviderLogo";
import { stateLabel } from "@/components/StatusBadge";
import { onFlowEvent, type FlowEvent, type InflightAttempt } from "@/hooks/useLiveTraffic";
import { useT, type TFunction } from "@/i18n";
import { clientMeta, clientKey, fmtMs, fmtTok, isBackground, isOk, median, totalTokens } from "@/lib/liveStats";
import { nodeKey, type LiveTopology } from "@/components/live/useLiveTopology";
import type { LiveRequest, SubscriptionDto } from "@/types";

const LEG_MS = [350, 450];
const FAILOVER_LEG_MS = 520;
const EDGE_WINDOW_MS = 60_000;
const FAILOVER_EDGE_MS = 20_000;
/** 新客户端第一次发请求时, 节点与连线要等下一次渲染才出现; 粒子最多等这么久 */
const WAIT_FOR_EDGE_MS = 600;

const reduceMotion = () =>
  typeof window !== "undefined" && window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;

interface Geo {
  l: number;
  r: number;
  y: number;
}

interface Particle {
  legs: string[];
  leg: number;
  t0: number;
  born: number;
  dot: SVGCircleElement;
  glow: SVGCircleElement;
  failover: boolean;
}

const SVG_NS = "http://www.w3.org/2000/svg";
const ease = (k: number) => (k < 0.5 ? 2 * k * k : 1 - Math.pow(-2 * k + 2, 2) / 2);

interface Props {
  topo: LiveTopology;
  rows: readonly LiveRequest[];
  inflight: readonly InflightAttempt[];
  now: number;
  paused: boolean;
}

export function LiveFlowGraph({ topo, rows, inflight, now, paused }: Props) {
  const { t } = useT();
  const flowRef = useRef<HTMLDivElement>(null);
  const ptsRef = useRef<SVGGElement>(null);
  const paths = useRef(new Map<string, SVGPathElement>());
  const particles = useRef<Particle[]>([]);
  const raf = useRef<number | null>(null);
  const pausedRef = useRef(paused);
  pausedRef.current = paused;
  const [geo, setGeo] = useState<{ w: number; h: number; nodes: Record<string, Geo> }>({ w: 0, h: 0, nodes: {} });
  const [hover, setHover] = useState<string | null>(null);

  /* ---------- 几何 ---------- */
  const layoutSig = [...topo.clients, "|", ...topo.vms.map((v) => v.name), "|", ...topo.subs.map((s) => s.id)].join(",");
  const measure = useCallback(() => {
    const el = flowRef.current;
    if (!el) return;
    const fr = el.getBoundingClientRect();
    const nodes: Record<string, Geo> = {};
    el.querySelectorAll<HTMLElement>("[data-k]").forEach((n) => {
      const b = n.getBoundingClientRect();
      nodes[n.dataset.k!] = { l: b.left - fr.left, r: b.right - fr.left, y: b.top - fr.top + b.height / 2 };
    });
    setGeo({ w: fr.width, h: fr.height, nodes });
  }, []);
  useLayoutEffect(measure, [measure, layoutSig]);
  useEffect(() => {
    const el = flowRef.current;
    if (!el) return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [measure]);

  /* ---------- 线宽 / 状态 ---------- */
  const edgeStats = useMemo(() => {
    const count = new Map<string, number>();
    const fo = new Set<string>();
    const add = (k: string) => count.set(k, (count.get(k) ?? 0) + 1);
    for (const r of rows) {
      const vm = nodeKey.vm(r.virtual_model);
      const sub = `${vm}>${nodeKey.sub(r.subscription_id)}`;
      if (r.timestamp >= now - EDGE_WINDOW_MS) {
        if (r.retry_count === 0) add(`${nodeKey.client(clientKey(r.client_tool))}>${vm}`);
        add(sub);
      }
      if (r.retry_count > 0 && r.timestamp >= now - FAILOVER_EDGE_MS) fo.add(sub);
    }
    for (const a of inflight) {
      const vm = nodeKey.vm(a.vm);
      if (!a.failover) add(`${nodeKey.client(clientKey(a.client))}>${vm}`);
      add(`${vm}>${nodeKey.sub(a.sub)}`);
    }
    return { count, fo };
  }, [rows, inflight, now]);

  const busy = useMemo(() => {
    const m = new Map<string, number>();
    const add = (k: string) => m.set(k, (m.get(k) ?? 0) + 1);
    for (const a of inflight) {
      add(nodeKey.client(clientKey(a.client)));
      add(nodeKey.vm(a.vm));
      add(nodeKey.sub(a.sub));
    }
    return m;
  }, [inflight]);

  /* ---------- 粒子 ---------- */
  const step = useCallback((ts: number) => {
    const list = particles.current;
    for (let i = list.length - 1; i >= 0; i -= 1) {
      const p = list[i];
      const path = paths.current.get(p.legs[p.leg]);
      if (!path) {
        if (ts - p.born > WAIT_FOR_EDGE_MS) {
          p.dot.remove();
          p.glow.remove();
          list.splice(i, 1);
        } else {
          p.t0 = ts;
        }
        continue;
      }
      const dur = p.failover ? FAILOVER_LEG_MS : LEG_MS[p.leg] ?? LEG_MS[1];
      let k = (ts - p.t0) / dur;
      if (k >= 1) {
        if (p.leg < p.legs.length - 1) {
          p.leg += 1;
          p.t0 = ts;
          k = 0;
        } else {
          p.dot.remove();
          p.glow.remove();
          list.splice(i, 1);
          continue;
        }
      }
      const cur = paths.current.get(p.legs[p.leg]);
      if (!cur) continue;
      const pt = cur.getPointAtLength(cur.getTotalLength() * ease(Math.max(0, k)));
      p.dot.setAttribute("cx", String(pt.x));
      p.dot.setAttribute("cy", String(pt.y));
      p.glow.setAttribute("cx", String(pt.x));
      p.glow.setAttribute("cy", String(pt.y));
    }
    raf.current = list.length ? requestAnimationFrame(step) : null;
  }, []);

  const spawn = useCallback(
    (a: InflightAttempt) => {
      const g = ptsRef.current;
      if (!g) return;
      const vm = nodeKey.vm(a.vm);
      const toSub = `${vm}>${nodeKey.sub(a.sub)}`;
      const legs = a.failover ? [toSub] : [`${nodeKey.client(clientKey(a.client))}>${vm}`, toSub];
      const bg = isBackground(a.purpose);
      const cls = a.failover ? " fo" : bg ? " bg" : "";
      const glow = document.createElementNS(SVG_NS, "circle");
      glow.setAttribute("r", bg ? "6" : "9");
      glow.setAttribute("class", "lv-glow" + cls);
      const dot = document.createElementNS(SVG_NS, "circle");
      dot.setAttribute("r", bg ? "2.6" : "3.6");
      dot.setAttribute("class", "lv-pt" + cls);
      g.append(glow, dot);
      const ts = performance.now();
      particles.current.push({ legs, leg: 0, t0: ts, born: ts, dot, glow, failover: a.failover });
      if (raf.current == null) raf.current = requestAnimationFrame(step);
    },
    [step],
  );

  const finish = useCallback((r: LiveRequest) => {
    const el = flowRef.current?.querySelector<HTMLElement>(`[data-k="${CSS.escape(nodeKey.sub(r.subscription_id))}"]`);
    if (!el) return;
    const ok = isOk(r);
    el.classList.remove("flash-ok", "flash-err");
    void el.offsetWidth; // 重新触发动画
    el.classList.add(ok ? "flash-ok" : "flash-err");
    const pop = document.createElement("span");
    pop.className = "lv-pop" + (ok ? "" : " err");
    pop.textContent = ok ? `+${fmtTok(totalTokens(r))}` : String(r.http_status ?? r.status);
    el.appendChild(pop);
    window.setTimeout(() => pop.remove(), 1500);
  }, []);

  useEffect(
    () =>
      onFlowEvent((e: FlowEvent) => {
        if (pausedRef.current || reduceMotion()) return;
        if (e.type === "start") spawn(e.attempt);
        else finish(e.record);
      }),
    [spawn, finish],
  );

  useEffect(
    () => () => {
      if (raf.current != null) cancelAnimationFrame(raf.current);
      particles.current = [];
    },
    [],
  );

  // 暂停时清掉还在飞的粒子
  useEffect(() => {
    if (!paused) return;
    particles.current.forEach((p) => {
      p.dot.remove();
      p.glow.remove();
    });
    particles.current = [];
  }, [paused]);

  /* ---------- 悬停 ---------- */
  const related = useMemo(() => {
    if (!hover) return null;
    const nodes = new Set([hover]);
    const edges = new Set<string>();
    const [kind] = hover.split(":");
    for (const e of topo.edges) {
      const [a, b] = e.split(">");
      let hit = a === hover || b === hover;
      // 客户端 → 它用过的虚拟模型 → 这些虚拟模型的订阅; 订阅反向同理
      if (!hit && kind === "c" && a.startsWith("v:")) hit = topo.edges.includes(`${hover}>${a}`);
      if (!hit && kind === "s" && b.startsWith("v:")) hit = topo.edges.includes(`${b}>${hover}`);
      if (hit) {
        edges.add(e);
        nodes.add(a);
        nodes.add(b);
      }
    }
    return { nodes, edges };
  }, [hover, topo.edges]);

  const nodeCls = (k: string, base: string) =>
    [base, busy.get(k) ? "busy" : "", related && !related.nodes.has(k) ? "dim" : ""].filter(Boolean).join(" ");

  const recentSessions = (client: string) =>
    new Set(rows.filter((r) => r.timestamp > now - 5 * 60_000 && clientKey(r.client_tool) === client && r.session).map((r) => r.session!.id)).size;
  const runningAgents = (client: string) => inflight.filter((a) => clientKey(a.client) === client && a.purpose === "agent").length;
  const lastModel = (subId: string) => {
    for (let i = rows.length - 1; i >= 0; i -= 1) if (rows[i].subscription_id === subId) return rows[i].model;
    return null;
  };

  return (
    <div className="lv-flow-wrap">
      <div className="lv-flow-head">
        <span>{t("live.flow.hint")}</span>
        <div className="lv-legend">
          <span><i className="pt" />{t("live.flow.legendRequest")}</span>
          <span><i className="pt bg" />{t("live.flow.legendBackground")}</span>
          <span><i className="pt fo" />{t("live.flow.legendFailover")}</span>
          <span><i className="ln" />{t("live.flow.legendActive")}</span>
        </div>
      </div>
      <div className={"lv-flow" + (related ? " focusing" : "")} ref={flowRef} onMouseLeave={() => setHover(null)}>
        <svg className="lv-lines" viewBox={`0 0 ${geo.w || 1} ${geo.h || 1}`} aria-hidden>
          <g>
            {topo.edges.map((e) => {
              const [a, b] = e.split(">");
              const A = geo.nodes[a];
              const B = geo.nodes[b];
              if (!A || !B) return null;
              const x1 = A.r + 2;
              const x2 = B.l - 2;
              const dx = (x2 - x1) / 2;
              const n = edgeStats.count.get(e) ?? 0;
              const fo = edgeStats.fo.has(e);
              const width = n ? Math.min(14, 2 + Math.sqrt(n) * 2.2) : 1.2;
              const cls = ["lv-edge", fo ? "fo" : n ? "on" : "idle", related && !related.edges.has(e) ? "dim" : ""].join(" ");
              return (
                <path
                  key={e}
                  ref={(el) => {
                    if (el) paths.current.set(e, el);
                    else paths.current.delete(e);
                  }}
                  className={cls}
                  strokeWidth={width}
                  d={`M${x1} ${A.y} C ${x1 + dx} ${A.y}, ${x2 - dx} ${B.y}, ${x2} ${B.y}`}
                />
              );
            })}
          </g>
          <g ref={ptsRef} />
        </svg>

        <div className="lv-col">
          <div className="lv-col-label">
            {t("live.flow.clients")}
            {topo.clients.length > 0 && <span className="n">{topo.clients.length}</span>}
          </div>
          {topo.clients.length === 0 && <div className="lv-cli-empty">{t("live.flow.noClients")}</div>}
          {topo.clients.map((c) => {
            const k = nodeKey.client(c);
            const meta = clientMeta(c);
            const Icon = meta?.icon;
            const ss = recentSessions(c);
            const ag = runningAgents(c);
            return (
              <div key={k} data-k={k} className={nodeCls(k, "lv-node lv-cli")} onMouseEnter={() => setHover(k)}>
                <span className="ic">{Icon ? <Icon size={16} /> : <Ban size={14} />}</span>
                <div className="t">
                  <b>{meta ? t(meta.i18nKey) : t("live.otherClient")}</b>
                  <small>{ss ? t("live.flow.sessions", { n: ss }) + (ag ? " · " + t("live.flow.agents", { n: ag }) : "") : t("live.flow.noSession")}</small>
                </div>
                <span className="cnt">{busy.get(k) ?? 0}</span>
              </div>
            );
          })}
        </div>
        <div />
        <div className="lv-core">
          <div className="lv-core-title">cc-router<span>{t("live.flow.vms")}</span></div>
          {topo.vms.map((v) => {
            const k = nodeKey.vm(v.name);
            const idle = !busy.get(k) && !rows.some((r) => r.virtual_model === v.name);
            return (
              <div key={k} data-k={k} className={nodeCls(k, "lv-node lv-vm" + (idle ? " idle" : ""))} onMouseEnter={() => setHover(k)}>
                <div className="t">
                  <b>{v.name}</b>
                  <small>
                    {v.mode ? t(`vm.mode.${v.mode}`) : "—"} · {t("live.flow.subCount", { n: v.subIds.length })}
                  </small>
                </div>
                <span className="cnt">{busy.get(k) ?? 0}</span>
              </div>
            );
          })}
        </div>
        <div />
        <div className="lv-col">
          <div className="lv-col-label">{t("live.flow.subs")}</div>
          {topo.subs.map((s) => {
            const k = nodeKey.sub(s.id);
            return (
              <SubNodeView
                key={k}
                k={k}
                sub={s.dto}
                cooling={s.cooling}
                now={now}
                lastModel={lastModel(s.id)}
                busyN={busy.get(k) ?? 0}
                cls={nodeCls(k, "lv-node lv-sub")}
                onHover={() => setHover(k)}
                t={t}
              />
            );
          })}
        </div>
        {hover && geo.nodes[hover] && <NodeTip k={hover} rows={rows} topo={topo} geo={geo} t={t} />}
      </div>
    </div>
  );
}

/** 订阅额度用得最多的那个周期的比例; 没配额度返回 null */
export function quotaFraction(sub: SubscriptionDto | undefined): { frac: number; period: string } | null {
  let best: { frac: number; period: string } | null = null;
  for (const q of sub?.quota_usage ?? []) {
    if (!q.limit) continue;
    const frac = (q.input + q.output + q.cache_creation + q.cache_read) / q.limit;
    if (!best || frac > best.frac) best = { frac, period: q.period };
  }
  return best;
}

function SubNodeView(props: {
  k: string;
  sub: SubscriptionDto | undefined;
  cooling: boolean;
  now: number;
  lastModel: string | null;
  busyN: number;
  cls: string;
  onHover: () => void;
  t: TFunction;
}) {
  const { k, sub, cooling, now, lastModel, busyN, cls, onHover, t } = props;
  const quota = quotaFraction(sub);
  const disabled = sub ? !sub.enabled : false;
  const left = cooling && sub?.cooldown_until ? Math.max(0, sub.cooldown_until - now) : 0;
  const unhealthy = sub && sub.enabled && sub.state !== "healthy";
  let status: ReactNode = <span className="mono">{lastModel ?? "—"}</span>;
  if (disabled) status = t("live.sub.disabled");
  else if (cooling) {
    // 圆环按 1 分钟满格走, 更长的冷却先显示满圈
    const frac = Math.min(1, left / 60_000);
    status = (
      <>
        <svg className="lv-cool-ring" viewBox="0 0 20 20" aria-hidden>
          <circle className="bg" cx="10" cy="10" r="7" />
          <circle className="fg" cx="10" cy="10" r="7" strokeDasharray={`${(frac * 44).toFixed(1)} 44`} />
        </svg>
        {t("live.sub.cooling", { state: stateLabel(sub!.state, t), left: fmtLeft(left) })}
      </>
    );
  } else if (unhealthy) status = stateLabel(sub.state, t);
  const tone = disabled ? " off" : cooling ? " cool" : unhealthy && sub.state === "auth_failed" ? " dead" : "";
  return (
    <div data-k={k} className={cls + tone} onMouseEnter={onHover}>
      <ProviderLogo iconId={sub?.provider_icon} size={26} />
      <div className="t">
        <b>{sub?.display_name ?? t("live.sub.removed")}</b>
        <small>{status}</small>
      </div>
      <span className="cnt">{busyN}</span>
      {quota && (
        <span className="lv-meter" title={t("live.sub.quota", { pct: Math.round(quota.frac * 100) })}>
          <i className={quota.frac > 0.75 ? "hi" : undefined} style={{ width: `${Math.min(100, quota.frac * 100)}%` } as CSSProperties} />
        </span>
      )}
    </div>
  );
}

function fmtLeft(ms: number): string {
  const s = Math.ceil(ms / 1000);
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
  return `${Math.ceil(s / 3600)}h`;
}

function NodeTip(props: {
  k: string;
  rows: readonly LiveRequest[];
  topo: LiveTopology;
  geo: { w: number; h: number; nodes: Record<string, Geo> };
  t: TFunction;
}) {
  const { k, rows, topo, geo, t } = props;
  const [kind, ...rest] = k.split(":");
  const id = rest.join(":");
  const mine = rows.filter((r) =>
    kind === "c" ? clientKey(r.client_tool) === id : kind === "v" ? r.virtual_model === id : r.subscription_id === id,
  );
  const ok = mine.filter(isOk);
  const top = (f: (r: LiveRequest) => string) => {
    const m = new Map<string, number>();
    mine.forEach((r) => m.set(f(r), (m.get(f(r)) ?? 0) + 1));
    return [...m.entries()].sort((a, b) => b[1] - a[1])[0]?.[0];
  };
  const extra =
    kind === "s"
      ? [t("live.tip.from"), top((r) => r.virtual_model)]
      : kind === "v"
        ? [t("live.tip.to"), top((r) => topo.subsById.get(r.subscription_id)?.display_name ?? r.provider_id)]
        : [t("live.tip.mostUsed"), top((r) => r.virtual_model)];
  const title =
    kind === "c" ? (clientMeta(id) ? t(clientMeta(id)!.i18nKey) : t("live.otherClient")) : kind === "v" ? id : topo.subsById.get(id)?.display_name ?? id;
  const g = geo.nodes[k];
  const W = 220;
  let left = kind === "s" ? g.r - W : g.l;
  left = Math.max(4, Math.min(left, geo.w - W - 4));
  const below = g.y + 40 + 150 < geo.h;
  const style: CSSProperties = { left, width: W, ...(below ? { top: g.y + 30 } : { bottom: geo.h - g.y + 30 }) };
  const ttft = median(ok.map((r) => r.ttft_ms).filter((x): x is number => x != null));
  return (
    <div className="lv-tip" style={style}>
      <b>{title}</b>
      <div className="r"><span>{t("live.tip.requests")}</span><span>{mine.length}</span></div>
      <div className="r"><span>{t("live.tip.success")}</span><span>{mine.length ? Math.round((ok.length / mine.length) * 100) + "%" : "—"}</span></div>
      <div className="r"><span>{t("live.tip.tokens")}</span><span>{fmtTok(mine.reduce((s, r) => s + totalTokens(r), 0))}</span></div>
      <div className="r"><span>{t("live.tip.ttft")}</span><span>{fmtMs(ttft)}</span></div>
      {extra[1] && <div className="r"><span>{extra[0]}</span><span>{extra[1]}</span></div>}
    </div>
  );
}
