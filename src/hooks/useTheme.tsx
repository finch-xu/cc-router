import {
  createContext,
  useContext,
  useEffect,
  useState,
  useCallback,
  type ReactNode,
} from "react";
import {
  applyThemeAttrs,
  DEFAULT_THEME_ID,
  isThemeId,
  pluginIdOf,
  THEME_ID_KEY,
  themeDef,
  type ThemeArt,
  type ThemeId,
} from "@/themes";
import {
  cachePluginCss,
  clearPluginCss,
  injectPluginCss,
  loadPluginThemeCss,
  PluginThemeError,
} from "@/plugins/theme/apply";
import { runtime } from "@/runtime";

/** 插件页「试穿」: 临时套上一份主题 CSS, 不保存; 置 null 恢复当前主题 */
export interface ThemePreview {
  pluginId: string;
  css: string;
}

/** 明暗模式 (与界面主题正交: 每个主题都有明暗两套) */
export type ColorMode = "light" | "dark" | "system";
type ResolvedMode = "light" | "dark";

// 键名沿用明暗模式最早的叫法, 改名会丢掉老用户的选择
const MODE_KEY = "cc-router-theme";

function readStorage(key: string): string | null {
  try {
    if (typeof window === "undefined") return null;
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeStorage(key: string, value: string) {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    // localStorage 不可用时静默降级
  }
}

function getStoredMode(): ColorMode {
  const v = readStorage(MODE_KEY);
  return v === "light" || v === "dark" ? v : "system";
}

function getStoredThemeId(): ThemeId {
  const v = readStorage(THEME_ID_KEY);
  return isThemeId(v) ? v : DEFAULT_THEME_ID;
}

function getSystemDark(): boolean {
  try {
    return window.matchMedia("(prefers-color-scheme: dark)").matches;
  } catch {
    return false;
  }
}

interface ThemeContextValue {
  mode: ColorMode;
  resolved: ResolvedMode;
  setMode: (m: ColorMode) => void;
  themeId: ThemeId;
  /** 当前主题的画风; 按画风换组件 (侧栏图标 / Logo / 路由图) 时读它 */
  art: ThemeArt;
  setThemeId: (id: ThemeId) => void;
  preview: ThemePreview | null;
  setPreview: (p: ThemePreview | null) => void;
}

const ThemeContext = createContext<ThemeContextValue>({
  mode: "system",
  resolved: "light",
  setMode: () => {},
  themeId: DEFAULT_THEME_ID,
  art: themeDef(DEFAULT_THEME_ID).art,
  setThemeId: () => {},
  preview: null,
  setPreview: () => {},
});

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [mode, setModeState] = useState<ColorMode>(getStoredMode);
  const [themeId, setThemeIdState] = useState<ThemeId>(getStoredThemeId);
  const [systemDark, setSystemDark] = useState<boolean>(getSystemDark);

  const setMode = useCallback((m: ColorMode) => {
    writeStorage(MODE_KEY, m);
    setModeState(m);
  }, []);

  const setThemeId = useCallback((id: ThemeId) => {
    writeStorage(THEME_ID_KEY, id);
    setThemeIdState(id);
  }, []);

  const resolved: ResolvedMode =
    mode === "system" ? (systemDark ? "dark" : "light") : mode;

  // 同步 .dark class 到 <html>
  useEffect(() => {
    document.documentElement.classList.toggle("dark", resolved === "dark");
  }, [resolved]);

  // ---- 主题插件 ----
  const [preview, setPreview] = useState<ThemePreview | null>(null);
  const [pluginCss, setPluginCss] = useState<{ id: string; css: string } | null>(null);
  // 插件装 / 卸 / 更新后重读一次 (正在用的被卸载 → 读不到 → 回退默认主题)
  const [pluginRev, setPluginRev] = useState(0);
  useEffect(() => {
    const promise = runtime.listen("plugins_changed", () => setPluginRev((r) => r + 1));
    return () => {
      promise.then((un) => un()).catch(() => {});
    };
  }, []);

  useEffect(() => {
    const pid = pluginIdOf(themeId);
    if (!pid) {
      setPluginCss(null);
      cachePluginCss(null);
      return;
    }
    let cancelled = false;
    loadPluginThemeCss(pid)
      .then((css) => {
        if (cancelled) return;
        setPluginCss({ id: pid, css: css.full });
        cachePluginCss(pid, css.tokensOnly);
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        console.warn(`[theme] 主题插件 ${pid} 无法应用`, err);
        // 只有「确实装不上 / 内容不合法」才回退; 网络或登录态之类的暂时性错误保留选择, 下次再试
        const permanent =
          err instanceof PluginThemeError ||
          (typeof err === "object" && err !== null && (err as { code?: string }).code === "bad_request");
        if (permanent) {
          cachePluginCss(null);
          setThemeId(DEFAULT_THEME_ID);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [themeId, pluginRev, setThemeId]);

  // 同步 data-theme / data-art 与插件样式到 <html> (首帧由 index.html 内联脚本先打好)
  const effectiveId: ThemeId = preview ? `plugin:${preview.pluginId}` : themeId;
  useEffect(() => {
    applyThemeAttrs(effectiveId);
    const pid = pluginIdOf(themeId);
    if (preview) injectPluginCss(preview.css);
    else if (pid && pluginCss?.id === pid) injectPluginCss(pluginCss.css);
    else if (!pid) clearPluginCss();
    // 插件主题还在加载: 保留首帧注入的缓存样式
  }, [effectiveId, preview, pluginCss, themeId]);

  // 监听系统明暗变化 (mode === "system" 时 resolved 随之重算)
  useEffect(() => {
    try {
      const mq = window.matchMedia("(prefers-color-scheme: dark)");
      const handler = (e: MediaQueryListEvent) => setSystemDark(e.matches);
      mq.addEventListener("change", handler);
      return () => mq.removeEventListener("change", handler);
    } catch {
      return;
    }
  }, []);

  return (
    <ThemeContext.Provider
      value={{ mode, resolved, setMode, themeId, art: themeDef(effectiveId).art, setThemeId, preview, setPreview }}
    >
      {children}
    </ThemeContext.Provider>
  );
}

export function useTheme() {
  return useContext(ThemeContext);
}
