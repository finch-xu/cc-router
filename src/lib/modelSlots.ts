import type { ModelSlots } from "@/types";

/** 四个 model slot 的 key, 顺序即 UI 展示顺序 (最强在前). */
export const MODEL_SLOT_KEYS = ["fable", "opus", "sonnet", "haiku"] as const;

/** 把四个 slot 全设成同一个真实模型. 用于占位/自动填充/初始化等"全槽同值"场景.
 *  显式字面量而非遍历 MODEL_SLOT_KEYS: 将来给 ModelSlots 加字段时 tsc 会在此处报错提醒. */
export function uniformSlots(model: string): ModelSlots {
  return { fable: model, opus: model, sonnet: model, haiku: model };
}

/** 四个 slot 是否都已填 (非空). */
export function allSlotsFilled(slots: ModelSlots): boolean {
  return MODEL_SLOT_KEYS.every((k) => !!slots[k]);
}
