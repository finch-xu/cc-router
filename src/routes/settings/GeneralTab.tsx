import { Toggle } from "@/components/Toggle";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useT, type LanguagePref } from "@/i18n";
import { useTheme, type Theme } from "@/hooks/useTheme";
import type { UpdateSource } from "@/types";
import type { SettingsForm } from "./useSettingsForm";

/** 通用: 语言 / 开机自启 / 主题 / 更新源. */
export function GeneralTab({ form }: { form: SettingsForm }) {
  const { t } = useT();
  const { theme, setTheme } = useTheme();

  return (
    <div className="card section">
      <div className="card-head">
        <div className="card-title">{t("settings.section.language")}</div>
      </div>
      <div className="card-body">
        <div className="setting-row">
          <div className="label-col">
            {t("settings.language.label")}
            <div className="desc">{t("settings.language.desc")}</div>
          </div>
          <Select
            value={form.preferredLanguage}
            onValueChange={(v) => void form.changeLanguage(v as LanguagePref)}
          >
            <SelectTrigger style={{ maxWidth: 200 }}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="system">{t("settings.language.system")}</SelectItem>
              <SelectItem value="zh">中文</SelectItem>
              <SelectItem value="en">English</SelectItem>
              <SelectItem value="ja">日本語</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <div className="setting-row">
          <div className="label-col">{t("settings.proxy.autostart.label")}</div>
          <Toggle
            checked={form.autostart}
            onChange={(v) => void form.changeAutostart(v)}
            aria-label={t("settings.proxy.autostart.label")}
          />
        </div>
        <div className="setting-row">
          <div className="label-col">{t("settings.theme.label")}</div>
          <Select value={theme} onValueChange={(v) => setTheme(v as Theme)}>
            <SelectTrigger style={{ maxWidth: 200 }}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="system">{t("settings.theme.system")}</SelectItem>
              <SelectItem value="light">{t("settings.theme.light")}</SelectItem>
              <SelectItem value="dark">{t("settings.theme.dark")}</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <div className="setting-row">
          <div className="label-col">{t("settings.update.source.label")}</div>
          <Select
            value={form.settings.data?.update_source ?? "china"}
            onValueChange={(v) => void form.changeUpdateSource(v as UpdateSource)}
          >
            <SelectTrigger style={{ maxWidth: 240 }}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="international">
                {t("settings.update.source.international")}
              </SelectItem>
              <SelectItem value="china">{t("settings.update.source.china")}</SelectItem>
            </SelectContent>
          </Select>
        </div>
      </div>
    </div>
  );
}
