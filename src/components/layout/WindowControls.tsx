import { runtime } from "@/runtime";
import { useT } from "@/i18n";
import { currentPlatform } from "@/lib/platform";

/**
 * Windows / Linux 自绘窗口三键 (decorations=false 后系统不再画)。
 * macOS 用原生红绿灯, 这里直接不渲染。
 * 关闭走 close() 而非 destroy(): 让 tray::on_window_event 拦截 CloseRequested 收进托盘,
 * 与原生 X 行为一致。
 */
export function WindowControls() {
  const { t } = useT();
  if (runtime.kind === "web") return null;
  if (currentPlatform() === "macos") return null;

  const run = (action: "minimize" | "toggleMaximize" | "close") => {
    try {
      runtime.windowAction(action);
    } catch {
      // 纯浏览器预览没有 Tauri 运行时, 按钮只占位
    }
  };

  return (
    <div className="win-controls">
      <button type="button" onClick={() => run("minimize")} aria-label={t("window.minimize")} title={t("window.minimize")}>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round">
          <path d="M5 12h14" />
        </svg>
      </button>
      <button type="button" onClick={() => run("toggleMaximize")} aria-label={t("window.maximize")} title={t("window.maximize")}>
        <svg width="11" height="11" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1">
          <rect x="0.5" y="0.5" width="9" height="9" />
        </svg>
      </button>
      <button type="button" className="close" onClick={() => run("close")} aria-label={t("window.close")} title={t("window.close")}>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round">
          <path d="M18 6 6 18" />
          <path d="m6 6 12 12" />
        </svg>
      </button>
    </div>
  );
}
