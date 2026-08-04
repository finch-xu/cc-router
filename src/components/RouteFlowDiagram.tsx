import { useMemo, type CSSProperties } from "react";
import { stateLabel } from "@/components/StatusBadge";
import { useProxyStatus } from "@/hooks/useSettings";
import { useVirtualModels } from "@/hooks/useVirtualModels";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useProviders } from "@/hooks/useProviders";
import { useAnyRouteFlashState } from "@/hooks/useRouteFlash";
import { fmtCooldownLeft } from "@/lib/format";
import { VM_ORDER } from "@/lib/virtualModels";
import { useT, type TFunction } from "@/i18n";
import logoUrl from "@/assets/logo.png";
import type { SubscriptionDto, VirtualModelDto } from "@/types";

/* ============================================================
 * 画布几何 (与 styles.css 的 .rf-* 绝对定位一体, 改一处必须改另一处)
 *
 *   0                     452..548 (hub)              714..846  854..986
 *   ├─ 客户端 150×60 ──── 弧 ──── ■ ──── 弧 ──── 云朵内列 ─ 云朵外列 ─┤
 *
 * 画布高度不是常数: 云朵超过 11 家时按需增高, 由 layoutUpstreams 算出后
 * 通过 --rf-h 传给 CSS。hub / 客户端 / 弧线汇聚点全部相对高度居中。
 * ============================================================ */
const CANVAS_W = 1000;
/** 设计稿高度, 同时是下限 */
const CANVAS_H_MIN = 340;

const CLIENT_H = 60;
const CLIENT_STEP = 76;
const CLIENT_COUNT = 4;
/** 4 个客户端整体垂直居中于画布: 首个 top 距中心 -144 (复刻设计稿的 26 / 102 / 178 / 254) */
const clientTop = (i: number, h: number) => h / 2 - 144 + i * CLIENT_STEP;
/** 弧线在客户端一侧的落点 = 图形垂直中心 */
const clientCy = (top: number) => top + CLIENT_H / 2;

const UP_W = 132;
const UP_H = 53;
/** 同列相邻云朵的垂直步长: 必须 ≥ 云朵高, 否则同一列自己就叠上了 */
const UP_STEP = 57;
/** 单列能容纳的上限 (设计稿的 6 个位置: top 1 → 286) */
const UP_SINGLE_MAX = 6;
const UP_TOP_FIRST = 1;
/** 外列贴右边缘; 内列整体左移一个云朵宽 + 8px 间隙, 保证两列水平完全分离 */
const UP_LEFT_OUTER = 854;
const UP_LEFT_INNER = UP_LEFT_OUTER - UP_W - 8;
const upCy = (top: number) => top + UP_H / 2;

/** 云朵位置 + 该云朵的弧线终点 x (两列的终点不同) */
interface UpstreamSlot {
  left: number;
  top: number;
  /** true = 内列, 弧线更短且控制点更早收敛 */
  inner: boolean;
}

/**
 * 云朵的纵向排布。
 *
 * - ≤ 6 家: 单列, 完全复刻设计稿坐标 (1 / 58 / 115 / 172 / 229 / 286);
 *   不足 6 个时保持 57 的步长整组垂直居中, 避免顶部堆一撮、下方空一片。
 * - > 6 家: 左右两列交错。交错的意义是让**水平**方向分离 —— 两列相隔一个
 *   完整云朵宽, 于是相邻两朵即便垂直只差半步(28.5px) 也绝不重叠, 单位高度
 *   能放下的家数翻倍。同列内仍按 57 的步长, 保证同列不叠。
 *
 * 画布高度随之增长: 外列 n 个需要 (n-1)*57 + 53, 内列相位偏移半步再加 28.5。
 * 10 家以内算下来仍在 340 以内, 11 家起画布才开始变高(367 / 424 / …)。
 */
function layoutUpstreams(count: number): { slots: UpstreamSlot[]; height: number } {
  if (count <= 0) return { slots: [], height: CANVAS_H_MIN };

  if (count <= UP_SINGLE_MAX) {
    const span = UP_STEP * (count - 1);
    const first =
      count === 1
        ? (CANVAS_H_MIN - UP_H) / 2
        : UP_TOP_FIRST + (UP_STEP * (UP_SINGLE_MAX - 1) - span) / 2;
    return {
      slots: Array.from({ length: count }, (_, i) => ({
        left: UP_LEFT_OUTER,
        top: first + i * UP_STEP,
        inner: false,
      })),
      height: CANVAS_H_MIN,
    };
  }

  // 偶数索引走外列, 奇数索引走内列 —— 自上而下读仍是 0,1,2,… 的顺序
  const outerCount = Math.ceil(count / 2);
  const halfStep = UP_STEP / 2;
  const needed = (outerCount - 1) * UP_STEP + UP_H + halfStep;
  const height = Math.max(CANVAS_H_MIN, needed);
  const top0 = (height - needed) / 2;

  const slots = Array.from({ length: count }, (_, i) => {
    const inner = i % 2 === 1;
    const row = Math.floor(i / 2);
    return {
      left: inner ? UP_LEFT_INNER : UP_LEFT_OUTER,
      top: top0 + row * UP_STEP + (inner ? halfStep : 0),
      inner,
    };
  });
  return { slots, height };
}

/** 上游弧线: 外列要跨过内列所在的 x 区间, 控制点提前到 690 收敛, 免得压到内列云朵 */
function upstreamArc(hubCy: number, cy: number, inner: boolean): string {
  return inner
    ? `M552 ${hubCy} C 610 ${hubCy}, 645 ${cy}, ${UP_LEFT_INNER - 4} ${cy}`
    : `M552 ${hubCy} C 650 ${hubCy}, 690 ${cy}, ${UP_LEFT_OUTER - 4} ${cy}`;
}

/** 本地 AI Agent 工具。写死 —— cc-router 无法探知是谁在调, 这里表达的是「谁可以调」。 */
const CLIENT_NAMES = ["Claude Code", "Codex", "OpenCode", "Others"];

interface UpstreamNode {
  providerId: string;
  name: string;
  icon?: string;
  healthy: boolean;
  /** 非 healthy 时的一行状态文案, 如「限流 4m」 */
  statusText: string | null;
  /** 该 provider 名下所有在用订阅 id, 用于实时闪烁聚合 */
  subIds: string[];
}

export function RouteFlowDiagram() {
  const { t } = useT();
  const proxy = useProxyStatus();
  const vms = useVirtualModels();
  const subs = useSubscriptions();
  const providers = useProviders();

  const subsMap = useMemo(() => {
    const m = new Map<string, SubscriptionDto>();
    subs.data?.forEach((s) => m.set(s.id, s));
    return m;
  }, [subs.data]);

  const orderedVms = useMemo<VirtualModelDto[]>(
    () =>
      VM_ORDER.map((name) => vms.data?.find((v) => v.name === name)).filter(
        (v): v is VirtualModelDto => v !== undefined,
      ),
    [vms.data],
  );

  const upstreams = useMemo<UpstreamNode[]>(
    () => collectUpstreams(orderedVms, subsMap, providers.data, t),
    [orderedVms, subsMap, providers.data, t],
  );

  const running = proxy.data?.running ?? false;
  const { slots: upSlots, height: canvasH } = layoutUpstreams(upstreams.length);
  const hubCy = canvasH / 2;
  const slotCount = orderedVms.filter((v) => v.name !== "model-fallback").length;

  return (
    <div className="rf-wrap" style={{ "--rf-h": `${canvasH}px` } as CSSProperties}>
      {/* .rf-stage 是缩放层: 标签行与画布共用同一个 transform, 保证始终对齐 */}
      <div className="rf-stage">
        <div className="rf-labels">
          <span className="rf-label">{t("liveRouting.clients")}</span>
          <span className="rf-count">
            {t("liveRouting.summary", { slots: slotCount, vendors: upstreams.length })}
          </span>
          <span className="rf-label">{t("liveRouting.upstreams")}</span>
        </div>

        <div className="rf-canvas">
          <svg
            className="rf-arcs"
            viewBox={`0 0 ${CANVAS_W} ${canvasH}`}
            fill="none"
            aria-hidden
          >
            {Array.from({ length: CLIENT_COUNT }, (_, i) => {
              const cy = clientCy(clientTop(i, canvasH));
              const d = `M168 ${cy} C 290 ${cy}, 350 ${hubCy}, 448 ${hubCy}`;
              return (
                <g key={`c${i}`}>
                  <path d={d} stroke="var(--rf-client-line)" strokeWidth={1.5} />
                  {running && (
                    <path
                      className="rf-flow"
                      d={d}
                      stroke="var(--rf-client-flow)"
                      strokeWidth={1.5}
                      strokeDasharray="3 14"
                      strokeLinecap="round"
                    />
                  )}
                </g>
              );
            })}

            {upstreams.map((u, i) => {
              const slot = upSlots[i];
              const d = upstreamArc(hubCy, upCy(slot.top), slot.inner);
              // 异常的链路画成红色虚线且不走流动层 —— 没有流量在上面跑
              if (!u.healthy) {
                return (
                  <path
                    key={u.providerId}
                    d={d}
                    stroke="var(--rf-err-line)"
                    strokeWidth={1.5}
                    strokeDasharray="4 4"
                  />
                );
              }
              return (
                <g key={u.providerId}>
                  <path d={d} stroke="var(--rf-up-line)" strokeWidth={1.5} />
                  {running && (
                    <path
                      className="rf-flow"
                      d={d}
                      stroke="var(--rf-up-flow)"
                      strokeWidth={1.5}
                      strokeDasharray="3 14"
                      strokeLinecap="round"
                    />
                  )}
                </g>
              );
            })}
          </svg>

          {Array.from({ length: CLIENT_COUNT }, (_, i) => (
            <div className="rf-client" style={{ top: clientTop(i, canvasH) }} key={i}>
              <div className="rf-client-bar">
                <i />
                <i />
                <i />
              </div>
              <div className="rf-client-body">
                <span className="rf-client-caret">❯</span>
                <span className="rf-client-name">{CLIENT_NAMES[i]}</span>
              </div>
            </div>
          ))}

          <div className={running ? "rf-hub" : "rf-hub off"}>
            <img src={logoUrl} alt="cc-router" />
          </div>
          <div className="rf-hub-label">cc-router</div>

          {upstreams.map((u, i) => (
            <UpstreamCloud key={u.providerId} node={u} slot={upSlots[i]} />
          ))}
        </div>
      </div>
    </div>
  );
}

function UpstreamCloud({ node, slot }: { node: UpstreamNode; slot: UpstreamSlot }) {
  const flash = useAnyRouteFlashState(node.subIds);
  const stroke = node.healthy ? "var(--rf-up-line)" : "var(--rf-err-line)";
  // 实时高亮: 有请求打到这家时描边加粗提亮, 与弧线的 flow 动画互补
  const active = flash !== undefined;

  return (
    <div
      className={node.healthy ? "rf-up" : "rf-up err"}
      style={{ top: slot.top, left: slot.left }}
      title={node.statusText ? `${node.name} · ${node.statusText}` : node.name}
    >
      <svg viewBox="0 0 160 64" preserveAspectRatio="none" aria-hidden>
        <path
          d="M28 58 A24 24 0 0 1 26 20 A22 22 0 0 1 62 12 A26 26 0 0 1 106 16
             A20 20 0 0 1 132 26 A18 18 0 0 1 132 58 Z"
          fill="var(--surface)"
          stroke={active ? "var(--accent)" : stroke}
          strokeWidth={active ? 3 : 2}
          strokeLinejoin="round"
        />
      </svg>
      <div className="rf-up-text">
        <span className="rf-up-name">{node.name}</span>
        {node.statusText && <span className="rf-up-state">{node.statusText}</span>}
      </div>
    </div>
  );
}

/**
 * 收集「在用」的上游: 只算被虚拟模型引用到的订阅, 按 provider 去重。
 * 一家 provider 下任一在用订阅非 healthy 就整体标异常 —— 图上一朵云代表一家,
 * 没有半健康的表达方式, 报警比报平安安全。
 */
function collectUpstreams(
  vms: VirtualModelDto[],
  subsMap: Map<string, SubscriptionDto>,
  providers: { id: string; display_name: string; icon?: string }[] | undefined,
  t: TFunction,
): UpstreamNode[] {
  const byProvider = new Map<string, SubscriptionDto[]>();
  for (const vm of vms) {
    for (const sid of vm.subscription_ids) {
      const sub = subsMap.get(sid);
      if (!sub) continue;
      const list = byProvider.get(sub.provider_id);
      if (list) {
        if (!list.some((s) => s.id === sub.id)) list.push(sub);
      } else {
        byProvider.set(sub.provider_id, [sub]);
      }
    }
  }

  return Array.from(byProvider.entries()).map(([providerId, list]) => {
    const info = providers?.find((p) => p.id === providerId);
    const bad = list.find((s) => s.state !== "healthy");
    const cooldown = bad ? fmtCooldownLeft(bad.cooldown_until) : null;
    return {
      providerId,
      name: info?.display_name ?? list[0].provider_display_name ?? providerId,
      icon: info?.icon ?? list[0].provider_icon,
      healthy: !bad,
      statusText: bad
        ? cooldown
          ? `${stateLabel(bad.state, t)} ${cooldown}`
          : stateLabel(bad.state, t)
        : null,
      subIds: list.map((s) => s.id),
    };
  });
}
