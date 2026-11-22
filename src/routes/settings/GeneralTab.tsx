import { useState } from "react";
import { Toggle } from "@/components/Toggle";
import { api } from "@/api/tauri";
import { errorText } from "@/lib/errorText";
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
import { Link } from "react-router";
import { useInstalledPlugins } from "@/hooks/usePlugins";
import { localized, type LocalizedText } from "@/plugins/manifest";
import type { UpdateSource } from "@/types";
import type { SettingsForm } from "./useSettingsForm";
import { useWizard } from "@/components/onboarding/wizardStore";
import { useOnboardingState } from "@/components/onboarding/useOnboardingState";

const COLOR_MODES: readonly ColorMode[] = ["system", "light", "dark"];

const DESKTOP_NOTIFY_EVENTS = [
  "subscription.auth_failed",
  "subscription.cooldown",
  "subscription.recovered",
  "subscription.quota_reached",
  "subscription.quota_warning",
  "subscription.balance_low",
  "requests.all_failed",
  "proxy.stopped",
  "update.available",
  "usage.daily_summary",
] as const;

/** 通用: 语言 / 开机自启 / 更新源 / 自动检查更新, 外观 (界面主题 / 明暗模式), 桌面通知. */
export function GeneralTab({ form }: { form: SettingsForm }) {
  const { t, locale } = useT();
  const { mode, setMode, themeId, setThemeId } = useTheme();
  const plugins = useInstalledPlugins();
  const pluginThemes = (plugins.data ?? []).filter((p) => p.plugin_type === "theme");
  const zoom = useUiZoom();
  const wizard = useWizard();
  const onboarding = useOnboardingState();
  const [notifyTestState, setNotifyTestState] = useState<{ ok: boolean; msg: string } | null>(null);
  const [notifyTesting, setNotifyTesting] = useState(false);

  async function sendTestDesktop() {
    setNotifyTesting(true);
    setNotifyTestState(null);
    try {
      await api.testDesktopNotification();
      setNotifyTestState({ ok: true, msg: t("settings.notifications.test.sent") });
    } catch (e) {
      setNotifyTestState({ ok: false, msg: errorText(e) });
    } finally {
      setNotifyTesting(false);
    }
  }

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
              style={{ display: "flex", maxWidth: 380 }}
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
            <div className="label-col">
              {t("settings.appearance.pluginThemes")}
              <div className="desc">
                <Link to="/plugins">{t("settings.appearance.managePlugins")}</Link>
              </div>
            </div>
            {pluginThemes.length === 0 ? (
              <div className="field-hint">{t("settings.appearance.noPluginThemes")}</div>
            ) : (
              <div className="radio-group" role="radiogroup" aria-label={t("settings.appearance.pluginThemes")} style={{ display: "flex", flexWrap: "wrap", maxWidth: 380 }}>
                {pluginThemes.map((p) => {
                  const id = `plugin:${p.id}` as const;
                  const name = (p.manifest as { name?: LocalizedText } | null)?.name;
                  return (
                    <button key={p.id} type="button" className={themeId === id ? "on" : ""} onClick={() => setThemeId(id)} role="radio" aria-checked={themeId === id}>
                      {localized(name, locale) || p.id}
                      {p.source.kind === "community" && <span className="src-tag">{t("plugins.source.community")}</span>}
                    </button>
                  );
                })}
              </div>
            )}
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

      {/* 系统桌面通知 (与通知插件共用同一事件源与每日汇总时间) */}
      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("settings.section.notifications")}</div>
        </div>
        <div className="card-body">
          <div className="setting-row">
            <div className="label-col">
              {t("settings.notifications.desktop.label")}
              <div className="desc">{t("settings.notifications.desktop.desc")}</div>
            </div>
            <Toggle
              checked={form.desktopNotificationsEnabled}
              onChange={(v) => void form.changeDesktopNotificationsEnabled(v)}
              aria-label={t("settings.notifications.desktop.label")}
            />
          </div>
          {form.desktopNotificationsEnabled && (
            <div className="setting-row" style={{ alignItems: "flex-start" }}>
              <div className="label-col">
                {t("settings.notifications.events.label")}
                <div className="desc">{t("settings.notifications.events.desc")}</div>
              </div>
              <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(180px, 1fr))", gap: "8px 14px" }}>
                {DESKTOP_NOTIFY_EVENTS.map((ev) => {
                  const checked = form.desktopNotificationEvents.includes(ev);
                  return (
                    <label
                      key={ev}
                      style={{ display: "flex", alignItems: "center", gap: 6, fontSize: 12.5, color: "var(--ink-2)", cursor: "pointer" }}
                    >
                      <input
                        type="checkbox"
                        checked={checked}
                        style={{ accentColor: "var(--accent)", width: 14, height: 14 }}
                        onChange={(e) => void form.toggleDesktopNotificationEvent(ev, e.target.checked)}
                      />
                      <span>{t(`notify.event.${ev}`)}</span>
                    </label>
                  );
                })}
              </div>
            </div>
          )}
          <div className="setting-row">
            <div className="label-col">
              {t("settings.notifications.dailySummaryTime.label")}
              <div className="desc">{t("settings.notifications.dailySummaryTime.desc")}</div>
            </div>
            <input
              type="time"
              className="input"
              style={{ maxWidth: 140 }}
              value={form.dailySummaryTime}
              onChange={(e) => void form.changeDailySummaryTime(e.target.value)}
              aria-label={t("settings.notifications.dailySummaryTime.label")}
            />
          </div>
          <div className="setting-row">
            <div className="label-col">
              {t("settings.notifications.test.label")}
              <div className="desc">
                {t("settings.notifications.pluginsHint")}{" "}
                <Link to="/plugins">{t("settings.notifications.managePlugins")}</Link>
              </div>
              {notifyTestState && (
                <div className="desc" style={{ color: notifyTestState.ok ? "var(--ok)" : "var(--err)" }}>
                  {notifyTestState.msg}
                </div>
              )}
            </div>
            {runtime.kind === "desktop" ? (
              <button
                type="button"
                className="btn sm"
                style={{ justifySelf: "start" }}
                disabled={notifyTesting}
                onClick={() => void sendTestDesktop()}
              >
                {t("settings.notifications.test.btn")}
              </button>
            ) : (
              <div className="field-hint">{t("settings.notifications.test.webOnly")}</div>
            )}
          </div>
        </div>
      </div>
    </>
  );
}
