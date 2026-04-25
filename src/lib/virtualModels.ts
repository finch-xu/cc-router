import type {
  RoutingMode,
  SubscriptionSlot,
  VirtualModelName,
} from "@/types";

export const VM_ORDER: VirtualModelName[] = [
  "model-opus",
  "model-sonnet",
  "model-haiku",
  "model-fallback",
];

export interface VmMeta {
  /** 用于详情页/卡片标题的"用途" */
  purpose: string;
  purposeEn: string;
  /** 用于下拉菜单/页面的人类可读 label */
  label: string;
}

export const VM_META: Record<VirtualModelName, VmMeta> = {
  "model-opus": {
    purpose: "高级任务",
    purposeEn: "Plan Mode",
    label: "高级任务 / Plan Mode",
  },
  "model-sonnet": {
    purpose: "主对话",
    purposeEn: "Default chat",
    label: "主对话",
  },
  "model-haiku": {
    purpose: "小任务",
    purposeEn: "Tool calls",
    label: "小任务 / 工具调用",
  },
  "model-fallback": {
    purpose: "兜底",
    purposeEn: "Unknown models",
    label: "兜底 · 未知模型透传",
  },
};

/** fallback 走原样透传,不绑 slot */
const SLOT_BY_VM: Record<VirtualModelName, SubscriptionSlot | null> = {
  "model-opus": "opus",
  "model-sonnet": "sonnet",
  "model-haiku": "haiku",
  "model-fallback": null,
};

export function vmNameToSlot(name: VirtualModelName): SubscriptionSlot | null {
  return SLOT_BY_VM[name];
}

export const MODE_LABEL: Record<RoutingMode, string> = {
  sequential: "顺序",
  round_robin: "轮询",
};
