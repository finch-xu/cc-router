import { platform } from "@tauri-apps/plugin-os";

/**
 * 窗口外观按三档平台分流 (见 CLAUDE.md「标题栏融入内容区」):
 * - macos: 原生红绿灯叠在 webview 上 (titleBarStyle=Overlay), 侧栏顶条给它留 80px
 * - windows / linux: decorations=false, 由 PageBar 自绘最小化 / 最大化 / 关闭
 * 其余平台 (ios/android 不会跑到这里) 一律按 linux 处理。
 */
export type AppPlatform = "macos" | "windows" | "linux";

const DATASET_KEY = "platform";

export function detectPlatform(): AppPlatform {
  // 仅开发时允许 ?platform=windows 强制切换, 用于浏览器里 QA 另一平台的顶栏排版
  if (import.meta.env.DEV) {
    const forced = new URLSearchParams(window.location.search).get("platform");
    if (forced === "macos" || forced === "windows" || forced === "linux") return forced;
  }
  try {
    const p = platform();
    if (p === "macos") return "macos";
    if (p === "windows") return "windows";
    return "linux";
  } catch {
    // 纯浏览器 (pnpm dev 不带 Tauri) 没有 plugin-os 内部对象, 退化到 UA 猜测
    const ua = navigator.userAgent;
    if (/Mac/i.test(ua)) return "macos";
    if (/Win/i.test(ua)) return "windows";
    return "linux";
  }
}

/** 首帧前写到 <html data-platform>, CSS 靠它决定顶条留白; 只在 main.tsx 调一次 */
export function applyPlatformAttr(): AppPlatform {
  const p = detectPlatform();
  document.documentElement.dataset[DATASET_KEY] = p;
  return p;
}

export function currentPlatform(): AppPlatform {
  const p = document.documentElement.dataset[DATASET_KEY];
  return p === "macos" || p === "windows" || p === "linux" ? p : detectPlatform();
}
