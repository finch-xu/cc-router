import { Outlet } from "react-router";
import { Sidebar } from "./Sidebar";

/**
 * 两栏壳: 侧栏 + main。main 恒为「顶栏 + 内部滚动区」的 flex column (见 styles.css .main),
 * 每个页面自己渲染 <PageBar> 与 .page-flow —— 顶栏就是窗口标题栏, 所以没有页面能例外。
 * 走卡片体系的页面在 .page-flow 里套 .page-flow-pad 拿留白。
 */
export function AppShell() {
  return (
    <div className="app">
      <Sidebar />
      <main className="main">
        <Outlet />
      </main>
    </div>
  );
}
