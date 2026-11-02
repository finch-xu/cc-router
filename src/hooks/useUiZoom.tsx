import { createContext, useContext, useRef, useState, type ReactNode } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { runtime } from "@/runtime";
import { applyUiZoom, DEFAULT_UI_ZOOM, readUiZoom, type UiZoomPercent } from "@/lib/uiZoom";

type ZoomError = "apply" | "save" | null;
interface ZoomState {
  percent: UiZoomPercent | null;
  error: ZoomError;
}

// 在 helper 的 try 内访问 localStorage，兼容连 getter 都会抛错的环境。
const storage = {
  getItem: (key: string) => window.localStorage.getItem(key),
  setItem: (key: string, value: string) => window.localStorage.setItem(key, value),
};

async function applyNativeZoom(factor: number) {
  await getCurrentWebview().setZoom(factor);
  // macOS 原生红绿灯不随页面缩放，CSS 留白要反向补偿。
  document.documentElement.style.setProperty("--ui-zoom", String(factor));
}

/** 在 React 挂载前恢复，避免先渲染 100% 再跳成用户选定比例。 */
export async function initializeUiZoom(): Promise<ZoomState> {
  if (runtime.kind !== "desktop") return { percent: 100, error: null };
  const percent = readUiZoom(storage);
  try {
    const { persisted } = await applyUiZoom(percent, applyNativeZoom, storage);
    return { percent, error: persisted ? null : "save" };
  } catch {
    // 原生调用失败仍要显示应用，设置页允许重新选择或重置来重试。
    return { percent: null, error: "apply" };
  }
}

interface UiZoomContextValue extends ZoomState {
  pending: boolean;
  change: (percent: UiZoomPercent) => Promise<void>;
  reset: () => Promise<void>;
}

const UiZoomContext = createContext<UiZoomContextValue | null>(null);

export function UiZoomProvider({ initial, children }: { initial: ZoomState; children: ReactNode }) {
  const [state, setState] = useState(initial);
  const [pending, setPending] = useState(false);
  const inFlight = useRef(false);

  async function change(percent: UiZoomPercent) {
    // ref 同步上锁，防止同一帧连续操作造成保存顺序与原生缩放不一致。
    if (runtime.kind !== "desktop" || inFlight.current) return;
    inFlight.current = true;
    setPending(true);
    try {
      const { persisted } = await applyUiZoom(percent, applyNativeZoom, storage);
      setState({ percent, error: persisted ? null : "save" });
    } catch {
      setState((previous) => ({ ...previous, error: "apply" }));
    } finally {
      inFlight.current = false;
      setPending(false);
    }
  }

  return (
    <UiZoomContext.Provider value={{ ...state, pending, change, reset: () => change(DEFAULT_UI_ZOOM) }}>
      {children}
    </UiZoomContext.Provider>
  );
}

export function useUiZoom() {
  const context = useContext(UiZoomContext);
  if (!context) throw new Error("useUiZoom requires UiZoomProvider");
  return context;
}
