export const UI_ZOOM_KEY = "cc-router-ui-zoom";
export const UI_ZOOM_OPTIONS = [80, 90, 100, 110, 125] as const;
export type UiZoomPercent = (typeof UI_ZOOM_OPTIONS)[number];
export const DEFAULT_UI_ZOOM: UiZoomPercent = 90;

function isUiZoom(value: unknown): value is UiZoomPercent {
  return UI_ZOOM_OPTIONS.some((percent) => percent === value);
}

export function readUiZoom(storage: Pick<Storage, "getItem">): UiZoomPercent {
  try {
    const percent = Number(storage.getItem(UI_ZOOM_KEY));
    if (isUiZoom(percent)) return percent;
  } catch {
    // 禁用存储时仍可在当前会话调节缩放。
  }
  return DEFAULT_UI_ZOOM;
}

/** 原生缩放成功后才保存；存储失败不撤销已经生效的缩放。 */
export async function applyUiZoom(
  percent: unknown,
  apply: (factor: number) => Promise<void>,
  storage: Pick<Storage, "setItem">,
): Promise<{ persisted: boolean }> {
  if (!isUiZoom(percent)) throw new Error("Invalid UI zoom");
  await apply(percent / 100);
  try {
    storage.setItem(UI_ZOOM_KEY, String(percent));
    return { persisted: true };
  } catch {
    return { persisted: false };
  }
}
