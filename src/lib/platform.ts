import { runtime } from "@/runtime";

/**
 * 窗口外观按四档平台分流 (见 CLAUDE.md「标题栏内嵌」):
 * - macos: 原生红绿灯叠在 webview 上 (titleBarStyle=Overlay), 拖窗带 28px, 侧栏为它让出这段高度
 * - windows / linux: decorations=false, 拖窗带 32px, 由 WindowChrome 里的 WindowControls 自绘三键
 * - web: 普通浏览器打开 /ui/, 没有窗口壳, 拖窗带高度 0
 * 其余平台 (ios/android 不会跑到这里) 一律按 linux 处理。
 */
export type AppPlatform = "macos" | "windows" | "linux" | "web";

const DATASET_KEY = "platform";

export function detectPlatform(): AppPlatform {
  if (runtime.kind === "web") return "web";
  // 仅开发时允许 ?platform=windows 强制切换, 用于浏览器里 QA 另一平台的顶栏排版
  if (import.meta.env.DEV) {
    const forced = new URLSearchParams(window.location.search).get("platform");
    if (forced === "macos" || forced === "windows" || forced === "linux") return forced;
  }
  const p = runtime.platformName();
  if (p === "macos") return "macos";
  if (p === "windows") return "windows";
  if (p === "linux") return "linux";
  const ua = navigator.userAgent;
  if (/Mac/i.test(ua)) return "macos";
  if (/Win/i.test(ua)) return "windows";
  return "linux";
}

/** 首帧前写到 <html data-platform>, CSS 靠它决定顶条留白; 只在 main.tsx 调一次 */
export function applyPlatformAttr(): AppPlatform {
  const p = detectPlatform();
  document.documentElement.dataset[DATASET_KEY] = p;
  return p;
}

export function currentPlatform(): AppPlatform {
  const p = document.documentElement.dataset[DATASET_KEY];
  return p === "macos" || p === "windows" || p === "linux" || p === "web"
    ? p
    : detectPlatform();
}
