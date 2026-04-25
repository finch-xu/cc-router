import { NavLink } from "react-router-dom";
import {
  Layers,
  Key,
  ScrollText,
  Settings as SettingsIcon,
  Info,
  BookOpen,
  type LucideIcon,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useProxyStatus } from "@/hooks/useSettings";
import { useUpdater } from "@/hooks/useUpdater";
import { version as VERSION } from "../../../package.json";
import logoUrl from "@/assets/logo.png";

interface NavItem {
  to: string;
  label: string;
  icon: LucideIcon;
  badge?: string | (() => string | null);
  dot?: boolean;
}

export function Sidebar() {
  const subs = useSubscriptions();
  const proxy = useProxyStatus();
  const { detected } = useUpdater();

  const subsCount = subs.data?.length ?? 0;
  const running = proxy.data?.running ?? false;
  const port = proxy.data?.port;
  const hasUpdate = detected !== null;

  const items: NavItem[] = [
    { to: "/guide", label: "接入指南", icon: BookOpen },
    { to: "/virtual-models", label: "虚拟模型", icon: Layers, badge: "4" },
    { to: "/subscriptions", label: "订阅管理", icon: Key, badge: subsCount > 0 ? String(subsCount) : undefined },
    { to: "/request-logs", label: "请求日志", icon: ScrollText },
    { to: "/settings", label: "设置", icon: SettingsIcon },
    { to: "/about", label: "关于", icon: Info, dot: hasUpdate },
  ];

  return (
    <aside className="sidebar">
      <div className="brand">
        <div className="brand-mark">
          <img src={logoUrl} alt="cc-router" />
        </div>
        <div className="brand-text">
          <div className="brand-name">cc-router</div>
          <div className="brand-tag">多订阅聚合代理</div>
        </div>
      </div>
      {items.map((it) => {
        const Ico = it.icon;
        const badge = typeof it.badge === "function" ? it.badge() : it.badge;
        return (
          <NavLink
            key={it.to}
            to={it.to}
            className={({ isActive }) => cn("nav-item", isActive && "active")}
          >
            <span className="nav-icon">
              <Ico size={16} strokeWidth={1.6} />
            </span>
            <span>{it.label}</span>
            {badge && <span className="badge mono">{badge}</span>}
            {!badge && it.dot && (
              <span
                aria-label="有可用更新"
                title="有可用更新"
                style={{
                  marginLeft: "auto",
                  width: 8,
                  height: 8,
                  borderRadius: 9999,
                  background: "var(--err)",
                  boxShadow: "0 0 0 2px var(--err-bg)",
                }}
              />
            )}
          </NavLink>
        );
      })}
      <div className="sidebar-footer">
        <span>
          <span className={cn("live-dot", !running && "off")} />
          <span className="mono">
            127.0.0.1{port ? `:${port}` : ""}
          </span>
        </span>
        <span className="mono">v{VERSION}</span>
      </div>
    </aside>
  );
}
