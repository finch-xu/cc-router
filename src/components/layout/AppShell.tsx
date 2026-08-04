import { Outlet, useLocation } from "react-router-dom";
import { Sidebar } from "./Sidebar";

/**
 * 走「通栏布局」的页面: main 不留 padding, 自身收成 flex column + overflow hidden,
 * 由页面内部的 .page-bar / .page-flow 接管滚动。其余页面继续用默认 padding。
 * 放在这里而不是让页面自己声明, 是因为 padding 挂在 main 上 —— 子组件够不着。
 */
const FLUSH_ROUTES = ["/live-routing", "/updates"];

export function AppShell() {
  const { pathname } = useLocation();
  const flush = FLUSH_ROUTES.includes(pathname);
  return (
    <div className="app">
      <Sidebar />
      <main className={flush ? "main flush" : "main"}>
        <Outlet />
      </main>
    </div>
  );
}
