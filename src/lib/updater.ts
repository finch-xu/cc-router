import { invoke } from "@tauri-apps/api/core";
import { open as openShell } from "@tauri-apps/plugin-shell";
import type { UpdateSource } from "@/types";

// 与 Rust 侧 src-tauri/src/updater_source.rs 的两个常量保持文本一致。
// 改这里时记得同步那边。
export const INTERNATIONAL_MANIFEST_URL =
  "https://github.com/finch-xu/cc-router/releases/latest/download/latest.json";
// 自有域名 d.cc-router.catonthe.top 反代阿里云 OSS bucket=cc-router-prod (oss-cn-shanghai)。
export const CHINA_MANIFEST_URL =
  "https://d.cc-router.catonthe.top/latest.json";

const INTERNATIONAL_RELEASE_PAGE = "https://github.com/finch-xu/cc-router/releases/latest";
// TODO(oss): OSS 部署后给国内用户一个对应的下载列表页(可以是 GitHub Pages 或 OSS 静态站)
const CHINA_RELEASE_PAGE = INTERNATIONAL_RELEASE_PAGE;

/** 把 settings.update_source 映射到 manifest URL。null/未知 → 国际(GitHub) */
export function manifestUrlForSource(source: UpdateSource | null): string {
  return source === "china" ? CHINA_MANIFEST_URL : INTERNATIONAL_MANIFEST_URL;
}

export async function isAppImageRuntime(): Promise<boolean> {
  try {
    return await invoke<boolean>("is_appimage_runtime");
  } catch {
    return false;
  }
}

export async function relaunchApp(): Promise<void> {
  await invoke("relaunch_app");
}

export async function openReleasePage(source: UpdateSource | null): Promise<void> {
  const url = source === "china" ? CHINA_RELEASE_PAGE : INTERNATIONAL_RELEASE_PAGE;
  await openShell(url).catch(() => {});
}

export function isLinuxPlatform(): boolean {
  if (typeof navigator === "undefined") return false;
  return /linux/i.test(navigator.userAgent);
}
