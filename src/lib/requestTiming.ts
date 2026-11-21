/**
 * 请求详情「耗时」条的分段 (网络 + 模型合在一条里)。详情页耗时条与复制文本共用, 口径只写在这里。
 *
 * 数据: 摘要的首字 / 总耗时 + 后端 NetTiming (proxy/net_timing.rs), 毫秒都相对本次尝试起点。
 * - 老记录 (net.prep_ms 为空): 退回原来的「等待首字 / 生成」两段。
 * - 流式上游: 本地 → DNS → 建连 → 上游响应头 (≈ 一次往返 + 上游网关, 算网络) → 等待首字 → 生成。
 * - 非流式上游: 上游生成完才回响应头, 所以是 本地 → DNS → 建连 → 等待上游 (含生成) → 读取响应体。
 * - 没收到响应头就失败: 网络各段之后剩下的都是「失败」。
 *
 * 时间点先单调化 (后一个 ≥ 前一个、都 ≤ 总耗时) 再求各段长度, 测量误差不会产生负宽度。
 */
import type { NetTiming, RequestSummary } from "@/types";

export type TimingSegKind =
  | "prep"
  | "dns"
  | "connect"
  | "head"
  | "wait"
  | "gen"
  | "upstream"
  | "body"
  | "pending"
  | "fail"
  | "whole";

export type TimingGroup = "net" | "model" | "fail";

export interface TimingSeg {
  kind: TimingSegKind;
  group: TimingGroup;
  ms: number;
}

export interface TimingModel {
  total: number;
  /** 首字 (已截到总耗时内); 没有则为 null */
  firstContent: number | null;
  segs: TimingSeg[];
  /** 有网络分段数据 (新记录); false = 老记录的两段式 */
  detailed: boolean;
  networkMs: number;
  modelMs: number;
}

const GROUP: Record<TimingSegKind, TimingGroup> = {
  prep: "net",
  dns: "net",
  connect: "net",
  head: "net",
  body: "net",
  wait: "model",
  gen: "model",
  upstream: "model",
  pending: "model",
  whole: "model",
  fail: "fail",
};

export function timingModel(summary: RequestSummary, net: NetTiming | null | undefined): TimingModel | null {
  const total = summary.total_ms;
  if (total == null || total <= 0) return null;
  const fc = summary.ttft_ms != null ? Math.min(Math.max(summary.ttft_ms, 0), total) : null;
  const failed = summary.status !== "success";

  const segs: TimingSeg[] = [];
  let cursor = 0;
  const push = (kind: TimingSegKind, end: number) => {
    const e = Math.min(total, Math.max(cursor, end));
    segs.push({ kind, group: GROUP[kind], ms: e - cursor });
    cursor = e;
  };

  const detailed = net?.prep_ms != null;
  if (!net || net.prep_ms == null) {
    if (fc != null) {
      push("wait", fc);
      push("gen", total);
    } else {
      push("whole", total);
    }
  } else {
    push("prep", net.prep_ms);
    if (net.dns_ms != null) push("dns", cursor + net.dns_ms);
    if (net.connect_ms != null) push("connect", cursor + net.connect_ms);
    if (net.headers_ms == null) {
      push("fail", total);
    } else if (net.upstream_stream === false) {
      push("upstream", net.headers_ms);
      push(failed ? "fail" : "body", total);
    } else {
      push("head", net.headers_ms);
      if (fc != null) {
        push("wait", fc);
        push("gen", total);
      } else {
        push(failed ? "fail" : "pending", total);
      }
    }
  }

  // 长度为 0 的段 (例如复用连接时 DNS / 建连根本不存在) 不画
  const visible = segs.filter((s) => s.ms > 0 || s.kind === "dns" || s.kind === "connect");
  const sum = (g: TimingGroup) => visible.filter((s) => s.group === g).reduce((a, s) => a + s.ms, 0);
  return { total, firstContent: fc, segs: visible, detailed, networkMs: sum("net"), modelMs: sum("model") };
}
