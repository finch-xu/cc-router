import { NavLink } from "react-router-dom";
import {
  Layers,
  Key,
  ScrollText,
  BarChart3,
  Receipt,
  Settings as SettingsIcon,
  Info,
  BookOpen,
  Activity,
  RefreshCw,
  type LucideIcon,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useVirtualModels } from "@/hooks/useVirtualModels";
import { useProxyStatus } from "@/hooks/useSettings";
import { useUpdater } from "@/hooks/useUpdater";
import { useT } from "@/i18n";
import logoUrl from "@/assets/logo.png";

interface NavItem {
  to: string;
  label: string;
  icon: LucideIcon;
  badge?: string | (() => string | null);
  dot?: boolean;
  /** 点的语义: 默认 err(红, 有更新) / ok(绿, 代理在跑) */
  dotTone?: "err" | "ok";
  /** 无障碍与 hover 提示文案的 i18n key */
  dotLabelKey?: string;
}

export function Sidebar() {
  const { t } = useT();
  const subs = useSubscriptions();
  const proxy = useProxyStatus();
  const { detected } = useUpdater();
  const vms = useVirtualModels();

  const subsCount = subs.data?.length ?? 0;
  const running = proxy.data?.running ?? false;
  const hasUpdate = detected !== null;

  const items: NavItem[] = [
    { to: "/guide", label: t("sidebar.nav.guide"), icon: BookOpen },
    {
      to: "/live-routing",
      label: t("sidebar.nav.liveRouting"),
      icon: Activity,
      dot: running,
      dotTone: "ok",
      dotLabelKey: "sidebar.proxyRunning",
    },
    {
      to: "/virtual-models",
      label: t("sidebar.nav.virtualModels"),
      icon: Layers,
      badge: String(vms.data?.length ?? 5),
    },
    { to: "/subscriptions", label: t("sidebar.nav.subscriptions"), icon: Key, badge: subsCount > 0 ? String(subsCount) : undefined },
    { to: "/request-logs", label: t("sidebar.nav.requestLogs"), icon: ScrollText },
    { to: "/statistics", label: t("sidebar.nav.statistics"), icon: BarChart3 },
    { to: "/receipts", label: t("sidebar.nav.receipts"), icon: Receipt },
    {
      to: "/updates",
      label: t("sidebar.nav.updates"),
      icon: RefreshCw,
      dot: hasUpdate,
      dotLabelKey: "sidebar.updateAvailable",
    },
    { to: "/settings", label: t("sidebar.nav.settings"), icon: SettingsIcon },
    { to: "/about", label: t("sidebar.nav.about"), icon: Info },
  ];

  return (
    <aside className="sidebar">
      <div className="brand">
        <div className="brand-mark">
          <img src={logoUrl} alt="cc-router" />
        </div>
        <div className="brand-text">
          <div className="brand-name">cc-router</div>
          <div className="brand-tag">{t("sidebar.brand.tag")}</div>
        </div>
      </div>
      {/* 代理地址/端口与版本号不在这里展示: 地址在「实时路由」页可复制,
       * 版本号在「关于」/「检查更新」页 —— 侧边栏只留导航。 */}
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
            <span className="nav-label">{it.label}</span>
            {badge && <span className="badge mono">{badge}</span>}
            {!badge && it.dot && (
              <span
                className={it.dotTone === "ok" ? "nav-dot ok" : "nav-dot"}
                aria-label={t(it.dotLabelKey ?? "sidebar.updateAvailable")}
                title={t(it.dotLabelKey ?? "sidebar.updateAvailable")}
              />
            )}
          </NavLink>
        );
      })}
    </aside>
  );
}
