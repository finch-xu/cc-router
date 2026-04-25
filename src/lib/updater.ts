import { check, type Update } from "@tauri-apps/plugin-updater";
import { invoke } from "@tauri-apps/api/core";
import { open as openShell } from "@tauri-apps/plugin-shell";

const RELEASE_PAGE_URL = "https://github.com/finch-xu/cc-router/releases/latest";

export type { Update };

export async function checkForUpdate(): Promise<Update | null> {
  try {
    const update = await check();
    return update ?? null;
  } catch (e) {
    console.warn("[updater] check failed", e);
    return null;
  }
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

export async function openReleasePage(): Promise<void> {
  await openShell(RELEASE_PAGE_URL).catch(() => {});
}

export function isLinuxPlatform(): boolean {
  if (typeof navigator === "undefined") return false;
  return /linux/i.test(navigator.userAgent);
}
