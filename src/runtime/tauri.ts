import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { downloadDir } from "@tauri-apps/api/path";
import { open as openShell } from "@tauri-apps/plugin-shell";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { save } from "@tauri-apps/plugin-dialog";
import { platform } from "@tauri-apps/plugin-os";
import type { Runtime } from "./types";

export const tauriRuntime: Runtime = {
  kind: "desktop",
  invoke: (cmd, args) => invoke(cmd, args),
  listen: async (name, handler) => {
    const unlisten = await listen(name, (e) => handler({ payload: e.payload as never }));
    return unlisten;
  },
  openExternal: (url) => openShell(url),
  copyText: async (text) => {
    try {
      await writeText(text);
    } catch {
      await navigator.clipboard.writeText(text);
    }
  },
  pickSavePath: async ({ defaultName, filters }) => {
    const p = await save({ defaultPath: defaultName, filters });
    return p ?? null;
  },
  downloadText: () => {
    throw new Error("downloadText is web-only");
  },
  openDownloadsDir: async () => {
    const dir = await downloadDir();
    await openShell(dir);
  },
  windowAction: (action) => {
    try {
      void getCurrentWindow()[action]();
    } catch {
      // 纯浏览器预览没有 Tauri 运行时
    }
  },
  platformName: () => {
    try {
      return platform();
    } catch {
      return "unknown";
    }
  },
};
