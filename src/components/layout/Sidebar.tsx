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
import { useT } from "@/i18n";
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
  const { t } = useT();
  const subs = useSubscriptions();
  const proxy = useProxyStatus();
  const { detected } = useUpdater();

  const subsCount = subs.data?.length ?? 0;
  const running = proxy.data?.running ?? false;
  const port = proxy.data?.port;
  const hasUpdate = detected !== null;

  const items: NavItem[] = [
    { to: "/guide", label: t("sidebar.nav.guide"), icon: BookOpen },
    { to: "/virtual-models", label: t("sidebar.nav.virtualModels"), icon: Layers, badge: "4" },
    { to: "/subscriptions", label: t("sidebar.nav.subscriptions"), icon: Key, badge: subsCount > 0 ? String(subsCount) : undefined },
    { to: "/request-logs", label: t("sidebar.nav.logs"), icon: ScrollText },
    { to: "/settings", label: t("sidebar.nav.settings"), icon: SettingsIcon },
    { to: "/about", label: t("sidebar.nav.about"), icon: Info, dot: hasUpdate },
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
                aria-label={t("sidebar.updateAvailable")}
                title={t("sidebar.updateAvailable")}
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
