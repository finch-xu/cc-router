import { NavLink, useLocation } from "react-router";
import { useEffect, useState, type ReactNode } from "react";
import {
  Activity,
  ArrowRight,
  BarChart3,
  BookOpen,
  Bot,
  DatabaseBackup,
  Info,
  Key,
  Layers,
  LogOut,
  Moon,
  Palette,
  PartyPopper,
  Puzzle,
  Receipt,
  RefreshCw,
  ScrollText,
  Settings as SettingsIcon,
  Sun,
  SunMoon,
  type LucideIcon,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useVirtualModels } from "@/hooks/useVirtualModels";
import { useProxyStatus } from "@/hooks/useSettings";
import { useUpdater } from "@/hooks/useUpdater";
import { useT } from "@/i18n";
import { isTauriHost, runtime, webLogout, webSession } from "@/runtime";
import { LogoMark } from "@/components/sketch/LogoMark";
import { SidebarIcon, type SidebarIconName } from "@/components/sketch/SidebarIcon";
import { useReleaseNotesDialog } from "@/components/release-notes/ReleaseNotesController";
import { useOnboardingState } from "@/components/onboarding/useOnboardingState";
import { useWizard } from "@/components/onboarding/wizardStore";
import { ONBOARDING_STEPS, stepIndex } from "@/lib/onboardingFlow";
import { useTheme, type ColorMode } from "@/hooks/useTheme";
import { PixelIcon } from "@/components/win2k/PixelIcon";
import { isPlainBased, nextThemeId, themeDef, type ThemeArt } from "@/themes";

/** 经典画风下的侧栏图标 (手绘改版前的 lucide 线性图标) */
const PLAIN_ICONS: Record<SidebarIconName, LucideIcon> = {
  guide: BookOpen,
  live: Activity,
  agent: Bot,
  vm: Layers,
  subs: Key,
  logs: ScrollText,
  stats: BarChart3,
  receipts: Receipt,
  plugins: Puzzle,
  updates: RefreshCw,
  backup: DatabaseBackup,
  settings: SettingsIcon,
  about: Info,
  logout: LogOut,
  whatsnew: PartyPopper,
  "mode-system": SunMoon,
  "mode-light": Sun,
  "mode-dark": Moon,
  theme: Palette,
};

/** 按画风出图标: 手绘 = 涂鸦小图 (28px), 经典 = lucide 线性图标 (16px), Win2000 = 像素图标 (16px) */
function NavIcon({ name, art }: { name: SidebarIconName; art: ThemeArt }) {
  if (art === "sketch") return <SidebarIcon name={name} size={28} />;
  if (art === "win2k") return <PixelIcon name={name} />;
  const Ico = PLAIN_ICONS[name];
  return <Ico size={16} strokeWidth={1.6} />;
}

/** 明暗模式按钮的轮换顺序: 跟随系统 → 浅色 → 暗色 → 跟随系统 */
const NEXT_MODE: Record<ColorMode, ColorMode> = { system: "light", light: "dark", dark: "system" };

interface NavItem {
  to: string;
  label: string;
  icon: SidebarIconName;
  badge?: string | (() => string | null);
  dot?: boolean;
  /** 点的语义: 默认 err(红, 有更新) / ok(绿, 代理在跑) */
  dotTone?: "err" | "ok";
  /** 无障碍与 hover 提示文案的 i18n key */
  dotLabelKey?: string;
  /** 自定义激活判定; 缺省走 NavLink 的 pathname 匹配 (它不看 query) */
  active?: boolean;
}

export function Sidebar() {
  const { t } = useT();
  const subs = useSubscriptions();
  const proxy = useProxyStatus();
  const { detected } = useUpdater();
  const vms = useVirtualModels();
  const notes = useReleaseNotesDialog();
  const { art, mode, setMode, themeId, setThemeId } = useTheme();
  const location = useLocation();
  const onboarding = useOnboardingState();
  const wizard = useWizard();
  // 引导被「稍后再设置」关掉、又还没走完: 侧栏底部给一个回去的入口
  const showResume = onboarding.data?.completed === false && wizard.state.dismissed;

  const subsCount = subs.data?.length ?? 0;
  const running = proxy.data?.running ?? false;
  const hasUpdate = detected !== null;
  // 「备份与迁移」是设置页的一个 tab (?tab=backup) 的快捷入口; 两项同路径, 按 query 区分谁亮
  const onSettings = location.pathname === "/settings";
  const onBackupTab = onSettings && new URLSearchParams(location.search).get("tab") === "backup";

  const items: NavItem[] = [
    { to: "/guide", label: t("sidebar.nav.guide"), icon: "guide" },
    {
      to: "/live-routing",
      label: t("sidebar.nav.liveRouting"),
      icon: "live",
      dot: running,
      dotTone: "ok",
      dotLabelKey: "sidebar.proxyRunning",
    },
    {
      to: "/virtual-models",
      label: t("sidebar.nav.virtualModels"),
      icon: "vm",
      badge: String(vms.data?.length ?? 5),
    },
    { to: "/subscriptions", label: t("sidebar.nav.subscriptions"), icon: "subs", badge: subsCount > 0 ? String(subsCount) : undefined },
    ...(isTauriHost ? [{ to: "/agent-monitor", label: t("agentMonitor.title"), icon: "agent" as const }] : []),
    { to: "/request-logs", label: t("sidebar.nav.requestLogs"), icon: "logs" },
    { to: "/statistics", label: t("sidebar.nav.statistics"), icon: "stats" },
    { to: "/receipts", label: t("sidebar.nav.receipts"), icon: "receipts" },
    { to: "/plugins", label: t("sidebar.nav.plugins"), icon: "plugins" },
    {
      to: "/updates",
      label: t("sidebar.nav.updates"),
      icon: "updates",
      dot: hasUpdate,
      dotLabelKey: "sidebar.updateAvailable",
    },
    { to: "/settings?tab=backup", label: t("sidebar.nav.backup"), icon: "backup", active: onBackupTab },
    { to: "/settings", label: t("sidebar.nav.settings"), icon: "settings", active: onSettings && !onBackupTab },
    { to: "/about", label: t("sidebar.nav.about"), icon: "about" },
  ];

  const [showLogout, setShowLogout] = useState(false);
  useEffect(() => {
    if (runtime.kind !== "web") return;
    void webSession().then((s) => setShowLogout(s.auth_enabled));
  }, []);

  return (
    <aside className="sidebar">
      <div className="brand">
        <LogoMark size={44} plainSize={32} variant="compact" className="brand-mark" />
        <div className="brand-text">
          <div className="brand-name">cc-router</div>
          <div className="brand-tag">{t("sidebar.brand.tag")}</div>
        </div>
      </div>
      {/* 手画波浪分隔线; 经典画风用品牌区的下边框代替 */}
      {art === "sketch" && <SketchRule className="brand-rule" />}
      {/* 代理地址/端口不在这里展示: 地址在「实时路由」页可复制 —— 侧边栏只留导航。
       * 版本号只以手写小字出现在底部「更新内容」入口上 (已读时)。
       * .nav-list / .nav-extra 默认 display: contents 不参与布局, Win2000 画风把它们画成凹陷的列表框。 */}
      <div className="nav-list">
        {items.map((it) => {
          const badge = typeof it.badge === "function" ? it.badge() : it.badge;
          return (
            <NavLink
              key={it.to}
              to={it.to}
              className={({ isActive }) => cn("nav-item", (it.active ?? isActive) && "active")}
              aria-current={it.active === false ? false : undefined}
            >
              <span className="nav-icon">
                <NavIcon name={it.icon} art={art} />
              </span>
              <span className="nav-label">{it.label}</span>
              {badge && <span className="badge mono">{badge}</span>}
              {!badge && it.dot && (
                <NavDot
                  art={art}
                  tone={it.dotTone ?? "err"}
                  label={t(it.dotLabelKey ?? "sidebar.updateAvailable")}
                />
              )}
            </NavLink>
          );
        })}
      </div>
      <div className="nav-footer">
        {/* 与品牌区下方对称的手画波浪线 */}
        {art === "sketch" && <SketchRule className="foot-rule" />}
        {showResume && (
          <button type="button" className="ob-resume" onClick={wizard.reopen}>
            <span>
              {t("onboarding.wizard.resume", {
                n: stepIndex(wizard.state.step) + 1,
                total: ONBOARDING_STEPS.length,
              })}
            </span>
            <ArrowRight size={14} aria-hidden="true" />
          </button>
        )}
        {(notes.hasNotes || showLogout) && (
          <div className="nav-extra">
            {notes.hasNotes && (
              <button
                type="button"
                className={cn("nav-item", notes.openMode === "manual" && "active")}
                onClick={notes.openManual}
              >
                <span className="nav-icon">
                  <NavIcon name="whatsnew" art={art} />
                </span>
                <span className="nav-label">{t("releaseNotes.entry")}</span>
                {notes.hasUnread ? (
                  <NavDot art={art} tone="err" label={t("releaseNotes.unread")} />
                ) : (
                  notes.current && <span className="nav-ver">v{notes.current}</span>
                )}
              </button>
            )}
            {showLogout && (
              <button
                type="button"
                className="nav-item nav-logout"
                onClick={() => {
                  void webLogout().then(() => window.location.reload());
                }}
              >
                <span className="nav-icon">
                  <NavIcon name="logout" art={art} />
                </span>
                <span className="nav-label">{t("sidebar.logout")}</span>
              </button>
            )}
          </div>
        )}
        {/* 明暗 / 主题: 只有图案的两枚按钮, 图案随当前状态变; 文字只在悬停提示与读屏里 */}
        <div className="nav-toggles">
          <ToggleButton
            label={t("sidebar.toggle.mode", { mode: t(`settings.theme.${mode}`) })}
            onClick={() => setMode(NEXT_MODE[mode])}
          >
            <NavIcon name={`mode-${mode}`} art={art} />
          </ToggleButton>
          <ToggleButton
            label={t("sidebar.toggle.theme", { theme: t(themeDef(themeId).labelKey) })}
            onClick={() => setThemeId(nextThemeId(themeId))}
          >
            <NavIcon name="theme" art={art} />
          </ToggleButton>
        </div>
      </div>
    </aside>
  );
}

function ToggleButton({
  label,
  onClick,
  children,
}: {
  label: string;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button type="button" className="nav-toggle" onClick={onClick} aria-label={label} title={label}>
      {children}
    </button>
  );
}

function SketchRule({ className }: { className: string }) {
  return (
    <svg className={className} viewBox="0 0 240 10" preserveAspectRatio="none" aria-hidden="true">
      <path d="M0 5 C 15 1, 45 9, 60 5 C 75 1, 105 9, 120 5 C 135 1, 165 9, 180 5 C 195 1, 225 9, 240 5" />
    </svg>
  );
}

/** 侧栏状态点。手绘: ok = 绿色手画圆点带放射短线 (代理在跑), err = 陶土色星号 (有可用更新);
 *  经典 / Win2000: 一粒实心圆点 (绿 / 红) */
function NavDot({ art, tone, label }: { art: ThemeArt; tone: "ok" | "err"; label: string }) {
  if (isPlainBased(art)) {
    return <span className={tone === "ok" ? "nav-dot ok" : "nav-dot"} role="img" aria-label={label} title={label} />;
  }
  if (tone === "ok") {
    return (
      <svg className="nav-dot ok" viewBox="0 0 18 18" width="18" height="18" role="img" aria-label={label}>
        <title>{label}</title>
        <path className="nav-dot-fill" d="M8 5 C 10.3 4.9, 11.7 6.5, 11.6 8.6 C 11.5 10.7, 9.9 12.1, 7.9 12 C 5.8 11.9, 4.4 10.3, 4.5 8.3 C 4.6 6.4, 6.1 5.1, 8 5 Z" />
        <path className="nav-dot-rays" d="M13.4 4.2 L15 2.6 M14.4 8.4 L16.6 8.3 M13.4 12.6 L15 14.2" />
      </svg>
    );
  }
  return (
    <svg className="nav-dot err" viewBox="0 0 16 16" width="15" height="15" role="img" aria-label={label}>
      <title>{label}</title>
      <path d="M8 1.8 L8 14.2 M2.6 4.6 L13.4 11.4 M2.6 11.4 L13.4 4.6" />
    </svg>
  );
}
