export interface Runtime {
  kind: "desktop" | "web";
  invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(name: string, handler: (event: { payload: T }) => void): Promise<() => void>;
  openExternal(url: string): Promise<void>;
  copyText(text: string): Promise<void>;
  /** 确认对话框: 桌面走 plugin-dialog (异步), web 走 window.confirm; 避免直接调 window.confirm (tauri-plugin-dialog 把它覆写成了 async 函数, 同步判真会直接跳过确认) */
  confirm(message: string): Promise<boolean>;
  /** 桌面: 系统保存对话框, 返回用户选的路径 (取消 → null); web: 抛错 (调用方应先判 kind) */
  pickSavePath(opts: { defaultName: string; filters: { name: string; extensions: string[] }[] }): Promise<string | null>;
  /** 桌面: 系统选择文件夹对话框 (取消 → null); web: 抛错 (调用方应先判 kind) */
  pickFolder(): Promise<string | null>;
  /** web: 触发浏览器下载; 桌面: 抛错 (调用方应先判 kind) */
  downloadText(filename: string, text: string, mime: string): void;
  /** web: 触发浏览器下载二进制内容; 桌面: 抛错 (调用方应先判 kind) */
  downloadBytes(filename: string, bytes: Uint8Array<ArrayBuffer>, mime: string): void;
  /** 桌面: 打开系统下载目录; web: no-op */
  openDownloadsDir(): Promise<void>;
  /** 桌面: 窗口三键; web: no-op */
  windowAction(action: "minimize" | "toggleMaximize" | "close"): void;
  /** 桌面: plugin-os 的平台名; web: "web" */
  platformName(): string;
}

/** 会话失效 (401) 时由 web runtime 派发的全局 DOM 事件名, WebAuthGate 监听 */
export const AUTH_REQUIRED_EVENT = "ccr:auth-required";
