import { useState, useEffect, useRef } from "react";
import { AlertTriangle, RefreshCw, Check } from "lucide-react";
import { Toggle } from "@/components/Toggle";
import { Spinner } from "@/components/Spinner";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { api } from "@/api/tauri";
import {
  useProxyStatus,
  useSettings,
  useUpdateSettings,
  useGenerateNewToken,
} from "@/hooks/useSettings";
import { useT, type LanguagePref } from "@/i18n";
import type { UpdateSource } from "@/types";

export function SettingsPage() {
  const { t } = useT();
  const settings = useSettings();
  const proxy = useProxyStatus();
  const updateMut = useUpdateSettings();

  const [port, setPort] = useState<number>(23456);
  const [listenAll, setListenAll] = useState(false);
  const [autostart, setAutostart] = useState(false);
  const [retentionDays, setRetentionDays] = useState(30);
  const [dbLimitMb, setDbLimitMb] = useState(500);
  const [authEnabled, setAuthEnabled] = useState(true);
  const [corsEnabled, setCorsEnabled] = useState(true);
  const [corsAllowOrigin, setCorsAllowOrigin] = useState("*");
  const [preferredLanguage, setPreferredLanguage] = useState<LanguagePref>("system");
  const [resetDialog, setResetDialog] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [tokenJustRegenerated, setTokenJustRegenerated] = useState(false);
  const regenerateTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // 防止 generateNewToken 等 mutation 触发的 settings 失效把用户未保存的编辑覆盖掉
  const initializedRef = useRef(false);

  const generateTokenMut = useGenerateNewToken();

  useEffect(
    () => () => {
      if (regenerateTimerRef.current) clearTimeout(regenerateTimerRef.current);
    },
    [],
  );

  useEffect(() => {
    if (settings.data && !initializedRef.current) {
      setPort(settings.data.proxy_port);
      setListenAll(settings.data.listen_all);
      setAutostart(settings.data.autostart);
      setRetentionDays(settings.data.log_retention_days);
      setDbLimitMb(settings.data.db_size_limit_mb);
      setAuthEnabled(settings.data.auth_enabled);
      setCorsEnabled(settings.data.cors_enabled);
      setCorsAllowOrigin(settings.data.cors_allow_origin);
      setPreferredLanguage(settings.data.preferred_language ?? "system");
      initializedRef.current = true;
    }
  }, [settings.data]);

  const needsRestart =
    settings.data !== undefined &&
    (settings.data.proxy_port !== port || settings.data.listen_all !== listenAll);

  async function save() {
    await updateMut.mutateAsync({
      proxy_port: port,
      listen_all: listenAll,
      autostart,
      log_retention_days: retentionDays,
      db_size_limit_mb: dbLimitMb,
      auth_enabled: authEnabled,
      cors_enabled: corsEnabled,
      cors_allow_origin: corsAllowOrigin,
      preferred_language: preferredLanguage,
    });
  }

  // 语言下拉单独立即保存,无需用户点"保存设置";其他字段需要点保存按钮。
  async function changeLanguage(next: LanguagePref) {
    setPreferredLanguage(next);
    await updateMut.mutateAsync({ preferred_language: next });
  }

  // 更新源同样即时保存:运行时一次性 builder 下次 check 自动按新源走
  async function changeUpdateSource(next: UpdateSource) {
    await updateMut.mutateAsync({ update_source: next });
  }

  async function regenerateToken() {
    try {
      await generateTokenMut.mutateAsync();
      setTokenJustRegenerated(true);
      if (regenerateTimerRef.current) clearTimeout(regenerateTimerRef.current);
      regenerateTimerRef.current = setTimeout(
        () => setTokenJustRegenerated(false),
        2000,
      );
    } catch (e) {
      alert(`${t("settings.auth.token.alertFailed")}: ${e}`);
    }
  }

  async function confirmReset() {
    setResetting(true);
    try {
      await api.factoryReset();
      // app 会自动重启;这里不会真正 resolve
    } catch (e) {
      setResetting(false);
      setResetDialog(false);
      alert(`${t("settings.danger.alertFailed")}: ${e}`);
    }
  }

  return (
    <>
      <div className="page-header">
        <h1>{t("settings.title")}</h1>
        <div className="subtitle">{t("settings.subtitle")}</div>
      </div>

      {/* 语言 */}
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
            <select
              className="select"
              style={{ maxWidth: 200 }}
              value={preferredLanguage}
              onChange={(e) => changeLanguage(e.target.value as LanguagePref)}
            >
              <option value="system">{t("settings.language.system")}</option>
              <option value="zh">中文</option>
              <option value="en">English</option>
              <option value="ja">日本語</option>
            </select>
          </div>
        </div>
      </div>

      {/* 更新设置 */}
      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("settings.section.update")}</div>
        </div>
        <div className="card-body">
          <div className="setting-row">
            <div className="label-col">
              {t("settings.update.source.label")}
            </div>
            <select
              className="select"
              style={{ maxWidth: 240 }}
              value={settings.data?.update_source ?? "international"}
              onChange={(e) => void changeUpdateSource(e.target.value as UpdateSource)}
            >
              <option value="international">
                {t("settings.update.source.international")}
              </option>
              <option value="china">{t("settings.update.source.china")}</option>
            </select>
          </div>
        </div>
      </div>

      {/* 代理服务 */}
      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("settings.section.proxy")}</div>
          <span className={"pill " + (proxy.data?.running ? "ok" : "")}>
            <span className="dot" />
            {proxy.data?.running
              ? t("settings.proxy.statusRunning")
              : t("settings.proxy.statusStopped")}
          </span>
        </div>
        <div className="card-body">
          <div className="setting-row">
            <div className="label-col">
              {t("settings.proxy.port.label")}
              <div className="desc">{t("settings.proxy.port.desc")}</div>
            </div>
            <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
              <input
                className="input mono"
                type="number"
                value={port}
                onChange={(e) => setPort(Number(e.target.value) || 23456)}
                style={{ width: 120 }}
              />
              <span style={{ fontSize: 12, color: "var(--ink-4)" }}>
                {t("settings.proxy.port.actual")}
                <span className="mono"> {proxy.data?.port ?? "-"}</span>
              </span>
            </div>
          </div>

          <div className="setting-row">
            <div className="label-col">
              {t("settings.proxy.bind.label")}
              <div className="desc">{t("settings.proxy.bind.desc")}</div>
            </div>
            <div>
              <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
                <Toggle
                  checked={listenAll}
                  onChange={setListenAll}
                  aria-label={t("settings.proxy.bind.aria")}
                />
                <span
                  className="mono"
                  style={{
                    fontSize: 12,
                    color: listenAll ? "var(--ink-2)" : "var(--ink-4)",
                  }}
                >
                  {listenAll ? "0.0.0.0:" : "127.0.0.1:"}
                  {port}
                </span>
              </div>
              {listenAll && (
                <div className="field-hint" style={{ color: "var(--err)" }}>
                  {t("settings.proxy.bind.warning")}
                </div>
              )}
            </div>
          </div>

          <div className="setting-row">
            <div className="label-col">{t("settings.proxy.autostart.label")}</div>
            <Toggle
              checked={autostart}
              onChange={setAutostart}
              aria-label={t("settings.proxy.autostart.label")}
            />
          </div>

          {needsRestart && (
            <div className="alert warn">
              <AlertTriangle size={14} />
              {t("settings.proxy.needsRestart")}
            </div>
          )}
        </div>
      </div>

      {/* 鉴权与跨域 */}
      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("settings.section.auth")}</div>
        </div>
        <div className="card-body">
          <div className="setting-row">
            <div className="label-col">
              {t("settings.auth.token.label")}
              <div className="desc">{t("settings.auth.token.desc")}</div>
            </div>
            <div>
              <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 10 }}>
                <Toggle
                  checked={authEnabled}
                  onChange={setAuthEnabled}
                  aria-label={t("settings.auth.token.label")}
                />
                <span style={{ fontSize: 12, color: "var(--ink-2)" }}>
                  {authEnabled
                    ? t("settings.auth.token.enabled")
                    : t("settings.auth.token.disabled")}
                </span>
              </div>
              {authEnabled && settings.data && (
                <div style={{ display: "flex", gap: 8 }}>
                  <input
                    className="input mono"
                    value={settings.data.auth_token}
                    readOnly
                    style={{ fontSize: 11.5, color: "var(--ink-2)" }}
                  />
                  <button
                    className="btn"
                    onClick={regenerateToken}
                    disabled={generateTokenMut.isPending}
                    type="button"
                  >
                    {generateTokenMut.isPending ? (
                      <Spinner />
                    ) : tokenJustRegenerated ? (
                      <Check size={12} style={{ color: "var(--ok)" }} />
                    ) : (
                      <RefreshCw size={12} />
                    )}
                    {tokenJustRegenerated
                      ? t("settings.auth.token.regenerated")
                      : t("settings.auth.token.regenerate")}
                  </button>
                </div>
              )}
            </div>
          </div>

          <div className="setting-row">
            <div className="label-col">
              {t("settings.auth.cors.label")}
              <div className="desc">
                {corsEnabled
                  ? t("settings.auth.cors.descEnabled")
                  : t("settings.auth.cors.descDisabled")}
              </div>
            </div>
            <div>
              <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 10 }}>
                <Toggle
                  checked={corsEnabled}
                  onChange={setCorsEnabled}
                  aria-label={t("settings.auth.cors.label")}
                />
                <span style={{ fontSize: 12, color: "var(--ink-2)" }}>
                  {corsEnabled
                    ? t("settings.auth.cors.statusEnabled")
                    : t("settings.auth.cors.statusDisabled")}
                </span>
              </div>
              {corsEnabled && (
                <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                  <input
                    className="input mono"
                    value={corsAllowOrigin}
                    onChange={(e) => setCorsAllowOrigin(e.target.value)}
                    placeholder="*"
                    style={{ maxWidth: 280 }}
                  />
                  <span
                    className="mono"
                    style={{ fontSize: 11.5, color: "var(--ink-4)" }}
                  >
                    Access-Control-Allow-Origin
                  </span>
                </div>
              )}
            </div>
          </div>
        </div>
      </div>

      {/* 数据存储 */}
      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("settings.section.storage")}</div>
        </div>
        <div className="card-body">
          <div className="setting-row">
            <div className="label-col">{t("settings.storage.retention.label")}</div>
            <select
              className="select"
              style={{ maxWidth: 200 }}
              value={String(retentionDays)}
              onChange={(e) => setRetentionDays(Number(e.target.value))}
            >
              <option value="7">{t("settings.storage.retention.7d")}</option>
              <option value="30">{t("settings.storage.retention.30d")}</option>
              <option value="90">{t("settings.storage.retention.90d")}</option>
              <option value="36500">{t("settings.storage.retention.forever")}</option>
            </select>
          </div>
          <div className="setting-row">
            <div className="label-col">{t("settings.storage.dbLimit.label")}</div>
            <select
              className="select"
              style={{ maxWidth: 200 }}
              value={String(dbLimitMb)}
              onChange={(e) => setDbLimitMb(Number(e.target.value))}
            >
              <option value="100">100 MB</option>
              <option value="500">500 MB</option>
              <option value="1024">1 GB</option>
              <option value="10240">{t("settings.storage.dbLimit.unlimited")}</option>
            </select>
          </div>
          <div style={{ display: "flex", justifyContent: "flex-end", paddingTop: 16 }}>
            <button className="btn primary" onClick={save} disabled={updateMut.isPending} type="button">
              {updateMut.isPending && <Spinner />}
              {t("common.saveSettings")}
            </button>
          </div>
        </div>
      </div>

      {/* 危险区域 */}
      <div className="danger-card section">
        <div>
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 8,
              fontSize: 13,
              fontWeight: 600,
              color: "oklch(0.42 0.16 28)",
              marginBottom: 6,
            }}
          >
            <AlertTriangle size={14} /> {t("settings.section.danger")}
          </div>
          <div style={{ fontSize: 12, color: "var(--ink-3)", lineHeight: 1.6 }}>
            {t("settings.danger.desc")}
          </div>
        </div>
        <button className="btn danger" type="button" onClick={() => setResetDialog(true)}>
          {t("settings.danger.button")}
        </button>
      </div>

      {/* 恢复出厂确认弹窗 */}
      <Dialog open={resetDialog} onOpenChange={(v) => !resetting && setResetDialog(v)}>
        <DialogContent className="cc-dialog">
          <DialogHeader>
            <DialogTitle>
              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <AlertTriangle size={16} style={{ color: "var(--err)" }} />
                {t("settings.danger.dialog.title")}
              </div>
            </DialogTitle>
            <DialogDescription asChild>
              <div>
                {t("settings.danger.dialog.intro")}
                <ul style={{ marginTop: 8, paddingLeft: 20, fontSize: 13, lineHeight: 1.7 }}>
                  <li>{t("settings.danger.dialog.item.subscriptions")}</li>
                  <li>{t("settings.danger.dialog.item.virtualModels")}</li>
                  <li>{t("settings.danger.dialog.item.logs")}</li>
                  <li>{t("settings.danger.dialog.item.settings")}</li>
                </ul>
                <p style={{ marginTop: 12 }}>{t("settings.danger.dialog.outro")}</p>
              </div>
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <button
              className="btn"
              onClick={() => setResetDialog(false)}
              disabled={resetting}
              type="button"
            >
              {t("common.cancel")}
            </button>
            <button
              className="btn danger"
              onClick={confirmReset}
              disabled={resetting}
              type="button"
            >
              {resetting && <Spinner />}
              {t("settings.danger.dialog.confirm")}
            </button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
