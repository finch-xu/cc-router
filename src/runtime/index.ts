import { tauriRuntime } from "./tauri";
import { webRuntime } from "./web";
import type { Runtime } from "./types";

export type { Runtime } from "./types";
export { AUTH_REQUIRED_EVENT } from "./types";
export { webLogin, webLogout, webSession, resetWebEventSource } from "./web";

/** Tauri WebView 注入 __TAURI_INTERNALS__; 普通浏览器没有 → web 模式 */
export const isTauriHost = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export const runtime: Runtime = isTauriHost ? tauriRuntime : webRuntime;
