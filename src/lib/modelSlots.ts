import type { ModelSlots, SlotEffort } from "@/types";

/** 四个 model slot 的 key, 顺序即 UI 展示顺序 (最强在前). */
export const MODEL_SLOT_KEYS = ["fable", "opus", "sonnet", "haiku"] as const;

/** 槽位 effort 可选档位, 顺序即下拉展示顺序 (弱→强).
 *  "auto" 不在此列 —— 它是下拉的 sentinel 空值 (value=""), 对应后端的 None. */
export const SLOT_EFFORT_LEVELS: readonly SlotEffort[] = [
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
] as const;

/** 把四个 slot 全设成同一个真实模型. 用于占位/自动填充/初始化等"全槽同值"场景.
 *  显式字面量而非遍历 MODEL_SLOT_KEYS: 将来给 ModelSlots 加字段时 tsc 会在此处报错提醒. */
export function uniformSlots(model: string): ModelSlots {
  return { fable: model, opus: model, sonnet: model, haiku: model };
}

/** 四个 slot 是否都已填 (非空). */
export function allSlotsFilled(slots: ModelSlots): boolean {
  return MODEL_SLOT_KEYS.every((k) => !!slots[k]);
}
