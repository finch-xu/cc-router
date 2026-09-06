import React from "react";
import ReactDOM from "react-dom/client";
import { BrowserRouter, HashRouter } from "react-router";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import App from "./App";
import { I18nProvider } from "@/i18n";
import { ThemeProvider } from "@/hooks/useTheme";
import { applyPlatformAttr } from "@/lib/platform";
import { runtime } from "@/runtime";
import "./styles.css";

// 首帧前打平台标记: 拖窗带高度 / 窗口三键的排版靠 <html data-platform> 分流
applyPlatformAttr();

// web 模式用 HashRouter: pathname 永远是 /ui/, Vite base "./" 的相对资源路径在任何深链下都正确,
// Rust 侧不需要改写 index.html 也不需要 <base> 标签
const Router = runtime.kind === "web" ? HashRouter : BrowserRouter;

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      refetchOnWindowFocus: false,
      staleTime: 5_000,
      retry: 1,
    },
  },
});

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <ThemeProvider>
        <I18nProvider>
          <Router>
            <App />
          </Router>
        </I18nProvider>
      </ThemeProvider>
    </QueryClientProvider>
  </React.StrictMode>,
);
