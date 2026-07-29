import {
  createContext,
  useContext,
  useEffect,
  useState,
  useCallback,
  type ReactNode,
} from "react";

export type Theme = "light" | "dark" | "system";
type ResolvedTheme = "light" | "dark";

const THEME_KEY = "cc-router-theme";

function getStoredTheme(): Theme {
  try {
    if (typeof window === "undefined") return "system";
    const v = window.localStorage.getItem(THEME_KEY);
    if (v === "light" || v === "dark") return v;
    return "system";
  } catch {
    return "system";
  }
}

function storeTheme(theme: Theme) {
  try {
    window.localStorage.setItem(THEME_KEY, theme);
  } catch {
    // localStorage 不可用时静默降级
  }
}

function getSystemDark(): boolean {
  try {
    return window.matchMedia("(prefers-color-scheme: dark)").matches;
  } catch {
    return false;
  }
}

interface ThemeContextValue {
  theme: Theme;
  resolved: ResolvedTheme;
  setTheme: (t: Theme) => void;
}

const ThemeContext = createContext<ThemeContextValue>({
  theme: "system",
  resolved: "light",
  setTheme: () => {},
});

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setThemeState] = useState<Theme>(getStoredTheme);
  const [systemDark, setSystemDark] = useState<boolean>(getSystemDark);

  const setTheme = useCallback((t: Theme) => {
    storeTheme(t);
    setThemeState(t);
  }, []);

  const resolved: ResolvedTheme =
    theme === "system" ? (systemDark ? "dark" : "light") : theme;

  // 同步 .dark class 到 <html>
  useEffect(() => {
    const root = document.documentElement;
    if (resolved === "dark") {
      root.classList.add("dark");
    } else {
      root.classList.remove("dark");
    }
  }, [resolved]);

  // 监听系统主题变化 (theme === "system" 时 resolved 随之重算)
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
    <ThemeContext.Provider value={{ theme, resolved, setTheme }}>
      {children}
    </ThemeContext.Provider>
  );
}

export function useTheme() {
  return useContext(ThemeContext);
}
