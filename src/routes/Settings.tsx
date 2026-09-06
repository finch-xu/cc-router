import { useSearchParams } from "react-router";
import { Settings2, Network, ShieldCheck, Wrench } from "lucide-react";
import { useT } from "@/i18n";
import { cn } from "@/lib/utils";
import { useSettingsForm } from "./settings/useSettingsForm";
import { GeneralTab } from "./settings/GeneralTab";
import { ProxyTab } from "./settings/ProxyTab";
import { AccessTab } from "./settings/AccessTab";
import { AdvancedTab } from "./settings/AdvancedTab";

const TAB_IDS = ["general", "proxy", "access", "advanced"] as const;
type SettingsTab = (typeof TAB_IDS)[number];

function isSettingsTab(v: string | null): v is SettingsTab {
  return TAB_IDS.includes(v as SettingsTab);
}

const ICON_SIZE = 14;

/**
 * 设置页: 顶部四个分组 tab (通用 / 代理 / 安全与访问 / 高级), 内容各自沿用 card 体系.
 * 当前 tab 落在 `?tab=` query 上, 刷新 / 从别处深链 (如引导页的「代理设置」) 都能落到对应分组.
 * 表单状态由 useSettingsForm 在页面层持有, 切 tab 不丢「需要重启」判定基准.
 */
export function SettingsPage() {
  const { t } = useT();
  const [searchParams, setSearchParams] = useSearchParams();
  const form = useSettingsForm();

  const rawTab = searchParams.get("tab");
  const tab: SettingsTab = isSettingsTab(rawTab) ? rawTab : "general";

  function selectTab(next: SettingsTab) {
    // replace: 切 tab 不堆历史记录, 返回键仍回到进设置页之前的页面.
    setSearchParams(next === "general" ? {} : { tab: next }, { replace: true });
  }

  const TABS: { id: SettingsTab; label: string; icon: React.ReactNode }[] = [
    { id: "general", label: t("settings.tab.general"), icon: <Settings2 size={ICON_SIZE} /> },
    { id: "proxy", label: t("settings.tab.proxy"), icon: <Network size={ICON_SIZE} /> },
    { id: "access", label: t("settings.tab.access"), icon: <ShieldCheck size={ICON_SIZE} /> },
    { id: "advanced", label: t("settings.tab.advanced"), icon: <Wrench size={ICON_SIZE} /> },
  ];

  return (
    <>
      <div className="page-header">
        <h1>{t("settings.title")}</h1>
        <div className="subtitle">{t("settings.subtitle")}</div>
      </div>

      <div className="tabs" role="tablist">
        {TABS.map(({ id, label, icon }) => (
          <button
            key={id}
            className={cn("tab", tab === id && "active")}
            onClick={() => selectTab(id)}
            role="tab"
            aria-selected={tab === id}
            type="button"
          >
            {icon}
            {label}
          </button>
        ))}
      </div>

      {tab === "general" && <GeneralTab form={form} />}
      {tab === "proxy" && <ProxyTab form={form} />}
      {tab === "access" && <AccessTab form={form} />}
      {tab === "advanced" && <AdvancedTab form={form} />}
    </>
  );
}
