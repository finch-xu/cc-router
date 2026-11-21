/**
 * 实时路由页的数据层: 一份模块级 store, 页面第一次挂载时装上监听, 之后常驻
 * (离开页面再回来, 最近一小时的数据还在, 不用重新回填)。
 *
 * 两路事件:
 * - `route_attempt_started`: 一次上游 attempt 开始 → 进「在途」表, 路由图据此放粒子;
 * - `live_request`: 一条请求日志条目 (= 一次 attempt 结束, 带 token / 用途 / 会话) →
 *   按 (虚拟模型, 订阅) 先进先出配对掉一条在途, 进历史。
 * 事件不带请求 id, 并发时配对可能错位, 但只影响「哪颗在途点先消失」, 统计以 live_request 为准。
 * 客户端中途断开等情况会留下等不到结束的在途项, 超过 STALE_MS 判为过期丢弃 (同 TUI 实时页)。
 */
import { useEffect, useSyncExternalStore } from "react";
import { api } from "@/api/tauri";
import { runtime } from "@/runtime";
import type { LiveRequest, PurposeKind, RouteAttemptStartedEvent } from "@/types";

export interface InflightAttempt {
  key: number;
  vm: string;
  sub: string;
  client: string | null;
  purpose: PurposeKind;
  start: number;
  /** 同一虚拟模型刚失败过 → 推断为失败后切下家的重试 */
  failover: boolean;
}

export type FlowEvent =
  | { type: "start"; attempt: InflightAttempt }
  | { type: "done"; record: LiveRequest; attempt?: InflightAttempt };

const KEEP_MS = 60 * 60 * 1000;
const STALE_MS = 10 * 60 * 1000;
/** 失败后这么久内同一虚拟模型的新 attempt 视为切换重试 */
const FAILOVER_WINDOW_MS = 3000;

const records: LiveRequest[] = [];
const ids = new Set<string>();
const inflight = new Map<number, InflightAttempt>();
const lastFailure = new Map<string, number>();
const subscribers = new Set<() => void>();
const flowListeners = new Set<(e: FlowEvent) => void>();
let version = 0;
let seq = 0;
let installed = false;
let loaded = false;

function notify() {
  version += 1;
  subscribers.forEach((fn) => fn());
}

function prune(now: number) {
  let drop = 0;
  while (drop < records.length && records[drop].timestamp < now - KEEP_MS) {
    ids.delete(records[drop].id);
    drop += 1;
  }
  if (drop) records.splice(0, drop);
  for (const [k, a] of inflight) if (now - a.start > STALE_MS) inflight.delete(k);
}

function insert(r: LiveRequest) {
  if (ids.has(r.id)) return false;
  ids.add(r.id);
  // 绝大多数按时间顺序到达; 回填与实时交错时插到正确位置
  let i = records.length;
  while (i > 0 && records[i - 1].timestamp > r.timestamp) i -= 1;
  records.splice(i, 0, r);
  return true;
}

function onStarted(p: RouteAttemptStartedEvent) {
  const now = Date.now();
  const failedAt = lastFailure.get(p.virtual_model);
  const attempt: InflightAttempt = {
    key: ++seq,
    vm: p.virtual_model,
    sub: p.subscription_id,
    client: p.client_tool ?? null,
    purpose: p.purpose ?? "unknown",
    start: now,
    failover: failedAt !== undefined && now - failedAt < FAILOVER_WINDOW_MS,
  };
  inflight.set(attempt.key, attempt);
  flowListeners.forEach((fn) => fn({ type: "start", attempt }));
  notify();
}

function onDone(r: LiveRequest) {
  let match: InflightAttempt | undefined;
  for (const a of inflight.values()) {
    if (a.vm === r.virtual_model && a.sub === r.subscription_id) {
      match = a;
      break; // Map 按插入顺序迭代 = 最早开始的那条
    }
  }
  if (match) inflight.delete(match.key);
  if (r.status !== "success") lastFailure.set(r.virtual_model, Date.now());
  if (!insert(r)) return;
  prune(Date.now());
  flowListeners.forEach((fn) => fn({ type: "done", record: r, attempt: match }));
  notify();
}

async function install() {
  if (installed) return;
  installed = true;
  try {
    await runtime.listen<RouteAttemptStartedEvent>("route_attempt_started", (e) => onStarted(e.payload));
    await runtime.listen<LiveRequest>("live_request", (e) => onDone(e.payload));
  } catch {
    installed = false;
    return;
  }
  try {
    const rows = await api.getLiveRequests(Date.now() - KEEP_MS);
    rows.forEach(insert);
  } catch {
    // 回填失败只影响首屏图表, 实时事件照常
  }
  loaded = true;
  notify();
  // 过期在途项 / 一小时外的记录要在没有新事件时也能被清掉
  setInterval(() => {
    const before = inflight.size + records.length;
    prune(Date.now());
    if (inflight.size + records.length !== before) notify();
  }, 15_000);
}

export interface LiveTraffic {
  version: number;
  loaded: boolean;
  /** 最近一小时, 按时间升序。只读, 不要改 */
  records: readonly LiveRequest[];
  inflight: ReadonlyMap<number, InflightAttempt>;
}

function snapshot(): number {
  return version;
}

function subscribe(fn: () => void) {
  subscribers.add(fn);
  return () => {
    subscribers.delete(fn);
  };
}

export function useLiveTraffic(): LiveTraffic {
  useEffect(() => {
    void install();
  }, []);
  const v = useSyncExternalStore(subscribe, snapshot, snapshot);
  return { version: v, loaded, records, inflight };
}

/** 路由图的动画通道: 不经过 React 渲染, 直接驱动粒子。返回取消订阅函数。 */
export function onFlowEvent(fn: (e: FlowEvent) => void): () => void {
  flowListeners.add(fn);
  return () => {
    flowListeners.delete(fn);
  };
}
