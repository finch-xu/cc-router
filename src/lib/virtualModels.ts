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
  /** i18n key for the "purpose" line shown above the slot */
  purposeKey: string;
  /** i18n key for the secondary English-style purpose tag */
  purposeEnKey: string;
  /** i18n key for the long human-readable label (used in dropdowns) */
  labelKey: string;
}

export const VM_META: Record<VirtualModelName, VmMeta> = {
  "model-opus": {
    purposeKey: "vm.opus.purpose",
    purposeEnKey: "vm.opus.purposeEn",
    labelKey: "vm.opus.label",
  },
  "model-sonnet": {
    purposeKey: "vm.sonnet.purpose",
    purposeEnKey: "vm.sonnet.purposeEn",
    labelKey: "vm.sonnet.label",
  },
  "model-haiku": {
    purposeKey: "vm.haiku.purpose",
    purposeEnKey: "vm.haiku.purposeEn",
    labelKey: "vm.haiku.label",
  },
  "model-fallback": {
    purposeKey: "vm.fallback.purpose",
    purposeEnKey: "vm.fallback.purposeEn",
    labelKey: "vm.fallback.label",
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

export const MODE_LABEL_KEY: Record<RoutingMode, string> = {
  sequential: "vm.mode.sequential",
  round_robin: "vm.mode.round_robin",
};
