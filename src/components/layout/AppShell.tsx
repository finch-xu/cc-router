import { Outlet, matchPath, useLocation } from "react-router-dom";
import { Sidebar } from "./Sidebar";

/**
 * 走「通栏布局」的页面: main 不留 padding, 自身收成 flex column + overflow hidden,
 * 由页面内部的 .page-bar / .page-flow 接管滚动。其余页面继续用默认 padding。
 * 放在这里而不是让页面自己声明, 是因为 padding 挂在 main 上 —— 子组件够不着。
 * 写成 react-router 的路径模式 (matchPath), 动态段用 :id, 静态路径照写。
 */
const FLUSH_ROUTES = ["/live-routing", "/updates", "/subscriptions/:id"];

/**
 * 与某条 FLUSH_ROUTES 模式同形、但刻意保留默认 padding 的路由。优先级高于 FLUSH_ROUTES。
 * `/subscriptions/:id` 会把字面量 `new` 当成 id 匹配上, 而新建向导是线性表单, 不通栏。
 */
const FLUSH_EXCEPTIONS = ["/subscriptions/new"];

export function AppShell() {
  const { pathname } = useLocation();
  const flush =
    !FLUSH_EXCEPTIONS.includes(pathname) &&
    FLUSH_ROUTES.some((pattern) => matchPath(pattern, pathname) !== null);
  return (
    <div className="app">
      <Sidebar />
      <main className={flush ? "main flush" : "main"}>
        <Outlet />
      </main>
    </div>
  );
}
