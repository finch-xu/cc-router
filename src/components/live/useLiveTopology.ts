/**
 * 路由图的拓扑: 三列节点 + 两段连线, 由「配置 (虚拟模型绑定)」与「时间窗内的真实流量」合成。
 * - 客户端: 只列时间窗内发过请求或此刻有请求在途的 (没用过的客户端不占位);
 * - 虚拟模型: 有绑定的内置 / 自定义虚拟模型, 加上流量里出现过但已不在配置里的;
 * - 订阅: 上述虚拟模型绑定的订阅, 加上流量里出现过的。
 * 客户端 → 虚拟模型的线只画真实走过的; 虚拟模型 → 订阅的线画全部绑定 (没流量就是虚线)。
 */
import { useMemo } from "react";
import { useCustomVirtualModels, useVirtualModels } from "@/hooks/useVirtualModels";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import type { InflightAttempt } from "@/hooks/useLiveTraffic";
import { CLIENT_TOOLS } from "@/lib/clientTools";
import { clientKey, OTHER_CLIENT } from "@/lib/liveStats";
import { VM_ORDER } from "@/lib/virtualModels";
import type { LiveRequest, RoutingMode, SubscriptionDto } from "@/types";

export interface VmNode {
  name: string;
  mode: RoutingMode | null;
  subIds: string[];
}

export interface SubNode {
  id: string;
  dto: SubscriptionDto | undefined;
  cooling: boolean;
}

export interface LiveTopology {
  clients: string[];
  vms: VmNode[];
  subs: SubNode[];
  /** "c:<client>>v:<vm>" 与 "v:<vm>>s:<sub>" */
  edges: string[];
  subsById: Map<string, SubscriptionDto>;
}

export const nodeKey = {
  client: (k: string) => `c:${k}`,
  vm: (k: string) => `v:${k}`,
  sub: (k: string) => `s:${k}`,
};

const CLIENT_ORDER = [...CLIENT_TOOLS.map((c) => c.id as string), OTHER_CLIENT];

export function useLiveTopology(rows: readonly LiveRequest[], inflight: readonly InflightAttempt[]): LiveTopology {
  const vmsQ = useVirtualModels();
  const customQ = useCustomVirtualModels();
  const subsQ = useSubscriptions();

  return useMemo(() => {
    const subsById = new Map<string, SubscriptionDto>();
    subsQ.data?.forEach((s) => subsById.set(s.id, s));

    const traffic = [
      ...rows.map((r) => ({ client: clientKey(r.client_tool), vm: r.virtual_model, sub: r.subscription_id })),
      ...inflight.map((a) => ({ client: clientKey(a.client), vm: a.vm, sub: a.sub })),
    ];

    const clientSet = new Set(traffic.map((x) => x.client));
    const clients = CLIENT_ORDER.filter((c) => clientSet.has(c));

    const vms: VmNode[] = [];
    const seenVm = new Set<string>();
    const pushVm = (v: VmNode) => {
      if (seenVm.has(v.name)) return;
      seenVm.add(v.name);
      vms.push(v);
    };
    const usedVms = new Set(traffic.map((x) => x.vm));
    for (const name of VM_ORDER) {
      const v = vmsQ.data?.find((x) => x.name === name);
      if (!v) continue;
      if (v.subscription_ids.length === 0 && !usedVms.has(name)) continue;
      // jev 只服务 /v1/systemone, 平时不占位, 有流量才出现
      if (name === "model-jev" && !usedVms.has(name)) continue;
      pushVm({ name, mode: v.mode, subIds: v.subscription_ids });
    }
    for (const c of customQ.data ?? []) {
      if (c.targets.length === 0 && !usedVms.has(c.name)) continue;
      pushVm({ name: c.name, mode: c.mode, subIds: [...new Set(c.targets.map((t) => t.subscription_id))] });
    }
    for (const name of usedVms) pushVm({ name, mode: null, subIds: [] });

    const subIds: string[] = [];
    const seenSub = new Set<string>();
    const pushSub = (id: string) => {
      if (seenSub.has(id)) return;
      seenSub.add(id);
      subIds.push(id);
    };
    vms.forEach((v) => v.subIds.forEach(pushSub));
    traffic.forEach((x) => pushSub(x.sub));

    const now = Date.now();
    const subs: SubNode[] = subIds.map((id) => {
      const dto = subsById.get(id);
      return { id, dto, cooling: !!dto?.cooldown_until && dto.cooldown_until > now };
    });

    const edges = new Set<string>();
    traffic.forEach((x) => edges.add(`${nodeKey.client(x.client)}>${nodeKey.vm(x.vm)}`));
    vms.forEach((v) => v.subIds.forEach((s) => edges.add(`${nodeKey.vm(v.name)}>${nodeKey.sub(s)}`)));
    traffic.forEach((x) => edges.add(`${nodeKey.vm(x.vm)}>${nodeKey.sub(x.sub)}`));

    return { clients, vms, subs, edges: [...edges], subsById };
  }, [rows, inflight, vmsQ.data, customQ.data, subsQ.data]);
}
