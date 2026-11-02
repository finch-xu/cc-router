import { Toggle } from "@/components/Toggle";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useT, type LanguagePref } from "@/i18n";
import { useTheme, type ColorMode } from "@/hooks/useTheme";
import { useUiZoom } from "@/hooks/useUiZoom";
import { DEFAULT_UI_ZOOM, UI_ZOOM_OPTIONS, type UiZoomPercent } from "@/lib/uiZoom";
import { runtime } from "@/runtime";
import { THEMES } from "@/themes";
import type { UpdateSource } from "@/types";
import type { SettingsForm } from "./useSettingsForm";
import { useWizard } from "@/components/onboarding/wizardStore";
import { useOnboardingState } from "@/components/onboarding/useOnboardingState";

const COLOR_MODES: readonly ColorMode[] = ["system", "light", "dark"];

/** 通用: 语言 / 开机自启 / 更新源 / 自动检查更新, 外观 (界面主题 / 明暗模式). */
export function GeneralTab({ form }: { form: SettingsForm }) {
  const { t } = useT();
  const { mode, setMode, themeId, setThemeId } = useTheme();
  const zoom = useUiZoom();
  const wizard = useWizard();
  const onboarding = useOnboardingState();

  return (
    <>
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
                <SelectItem value="zh">简体中文</SelectItem>
                <SelectItem value="zh-Hant">繁體中文</SelectItem>
                <SelectItem value="en">English</SelectItem>
                <SelectItem value="ja">日本語</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="setting-row">
            <div className="label-col">
              {t("settings.onboarding.label")}
              <div className="desc">{t("settings.onboarding.desc")}</div>
            </div>
            <button
              type="button"
              className="btn sm"
              style={{ justifySelf: "start" }}
              onClick={() => {
                // 还没走完: 接着上次的进度 (清掉会丢下窗口建的订阅); 已完成: 从头再走一遍
                if (onboarding.data?.completed) wizard.reset();
                wizard.reopen();
              }}
            >
              {t("settings.onboarding.reopen")}
            </button>
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
          <div className="setting-row">
            <div className="label-col">
              {t("settings.update.autoCheck.label")}
              <div className="desc">{t("settings.update.autoCheck.desc")}</div>
            </div>
            <Toggle
              checked={form.settings.data?.auto_check_updates ?? true}
              onChange={(v) => void form.changeAutoCheckUpdates(v)}
              aria-label={t("settings.update.autoCheck.label")}
            />
          </div>
        </div>
      </div>

      {/* 外观: 与侧栏底部的两枚图案按钮是同一份状态, 只存本机 (localStorage) */}
      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("settings.section.appearance")}</div>
        </div>
        <div className="card-body">
          <div className="setting-row">
            <div className="label-col">
              {t("settings.appearance.theme.label")}
              <div className="desc">{t("settings.appearance.theme.desc")}</div>
            </div>
            <div
              className="radio-group"
              role="radiogroup"
              aria-label={t("settings.appearance.theme.label")}
              style={{ display: "flex", maxWidth: 300 }}
            >
              {THEMES.map((theme) => (
                <button
                  key={theme.id}
                  type="button"
                  className={themeId === theme.id ? "on" : ""}
                  onClick={() => setThemeId(theme.id)}
                  role="radio"
                  aria-checked={themeId === theme.id}
                  style={{ flex: 1 }}
                >
                  {t(theme.labelKey)}
                </button>
              ))}
            </div>
          </div>
          <div className="setting-row">
            <div className="label-col">{t("settings.appearance.mode.label")}</div>
            <div
              className="radio-group"
              role="radiogroup"
              aria-label={t("settings.appearance.mode.label")}
              style={{ display: "flex", maxWidth: 300 }}
            >
              {COLOR_MODES.map((m) => (
                <button
                  key={m}
                  type="button"
                  className={mode === m ? "on" : ""}
                  onClick={() => setMode(m)}
                  role="radio"
                  aria-checked={mode === m}
                  style={{ flex: 1 }}
                >
                  {t(`settings.theme.${m}`)}
                </button>
              ))}
            </div>
          </div>
          {runtime.kind === "desktop" && (
            <div className="setting-row">
              <div className="label-col">
                {t("settings.appearance.zoom.label")}
                <div className="desc" id="ui-zoom-description">{t("settings.appearance.zoom.desc")}</div>
                {zoom.error && (
                  <div className="desc" role="alert" style={{ color: "var(--err)" }}>
                    {t(zoom.error === "apply" ? "settings.appearance.zoom.applyFailed" : "settings.appearance.zoom.saveFailed")}
                  </div>
                )}
              </div>
              <div className="ui-zoom-controls" aria-busy={zoom.pending}>
                <Select
                  value={zoom.percent === null ? "" : String(zoom.percent)}
                  onValueChange={(value) => void zoom.change(Number(value) as UiZoomPercent)}
                  disabled={zoom.pending}
                >
                  <SelectTrigger aria-label={t("settings.appearance.zoom.label")} aria-describedby="ui-zoom-description">
                    <SelectValue placeholder={t("settings.appearance.zoom.unapplied")} />
                  </SelectTrigger>
                  <SelectContent>
                    {UI_ZOOM_OPTIONS.map((percent) => (
                      <SelectItem key={percent} value={String(percent)}>
                        {percent === DEFAULT_UI_ZOOM ? t("settings.appearance.zoom.default", { percent }) : `${percent}%`}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <button type="button" className="btn sm" disabled={zoom.pending} onClick={() => void zoom.reset()}>
                  {t("settings.appearance.zoom.reset")}
                </button>
              </div>
            </div>
          )}
        </div>
      </div>
    </>
  );
}
