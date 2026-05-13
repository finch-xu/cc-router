import type React from "react";
import { useState, useEffect, useRef } from "react";

function arraysEqual<T>(a: readonly T[], b: readonly T[]): boolean {
  return a.length === b.length && a.every((v, i) => v === b[i]);
}
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
import type { ProxyMode, TlsStatus, UpdateSource } from "@/types";
import { save } from "@tauri-apps/plugin-dialog";
import { useQuery, useQueryClient } from "@tanstack/react-query";

export function SettingsPage() {
  const { t } = useT();
  const settings = useSettings();
  const proxy = useProxyStatus();
  const updateMut = useUpdateSettings();

  const [port, setPort] = useState<number>(23456);
  const [proxyMode, setProxyMode] = useState<ProxyMode>("http");
  const [httpsPort, setHttpsPort] = useState<number>(23457);
  const [listenAll, setListenAll] = useState(false);
  const [autostart, setAutostart] = useState(false);
  const [retentionDays, setRetentionDays] = useState(30);
  const [dbLimitMb, setDbLimitMb] = useState(500);
  const [authEnabled, setAuthEnabled] = useState(true);
  const [corsEnabled, setCorsEnabled] = useState(true);
  const [corsAllowOrigin, setCorsAllowOrigin] = useState("*");
  const [preferredLanguage, setPreferredLanguage] = useState<LanguagePref>("system");
  const [debugMode, setDebugMode] = useState(false);
  const [clearDumpsDialog, setClearDumpsDialog] = useState(false);
  const [clearingDumps, setClearingDumps] = useState(false);
  const [resetDialog, setResetDialog] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [tokenJustRegenerated, setTokenJustRegenerated] = useState(false);
  const regenerateTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // 仅在首次拿到 settings.data 时灌入本地 state + 记录 baseline. 后续 mutate refetch
  // 不再回灌, 否则会覆盖用户正在 input 里编辑但尚未 blur 的值 (port/cors origin 跳光标).
  const initializedRef = useRef(false);
  // baseline = 进程启动时观测到的 proxy/tls 配置. 代理直到 app 重启才会按新值绑定, 所以
  // "需要重启"的判定要拿 baseline (而非 settings.data) 比.
  const baselineRef = useRef<{
    proxy_port: number;
    listen_all: boolean;
    proxy_mode: ProxyMode;
    https_port: number;
    tls_extra_sans: string[];
  } | null>(null);

  const generateTokenMut = useGenerateNewToken();

  useEffect(
    () => () => {
      if (regenerateTimerRef.current) clearTimeout(regenerateTimerRef.current);
    },
    [],
  );

  useEffect(() => {
    if (!settings.data || initializedRef.current) return;
    baselineRef.current = {
      proxy_port: settings.data.proxy_port,
      listen_all: settings.data.listen_all,
      proxy_mode: settings.data.proxy_mode ?? "http",
      https_port: settings.data.https_port ?? 23457,
      tls_extra_sans: settings.data.tls_extra_sans ?? [],
    };
    setPort(settings.data.proxy_port);
    setProxyMode(settings.data.proxy_mode ?? "http");
    setHttpsPort(settings.data.https_port ?? 23457);
    setListenAll(settings.data.listen_all);
    setAutostart(settings.data.autostart);
    setRetentionDays(settings.data.log_retention_days);
    setDbLimitMb(settings.data.db_size_limit_mb);
    setAuthEnabled(settings.data.auth_enabled);
    setCorsEnabled(settings.data.cors_enabled);
    setCorsAllowOrigin(settings.data.cors_allow_origin);
    setPreferredLanguage(settings.data.preferred_language ?? "system");
    setDebugMode(settings.data.debug_mode ?? false);
    initializedRef.current = true;
  }, [settings.data]);

  const needsRestart =
    baselineRef.current !== null &&
    (port !== baselineRef.current.proxy_port ||
      listenAll !== baselineRef.current.listen_all ||
      proxyMode !== baselineRef.current.proxy_mode ||
      httpsPort !== baselineRef.current.https_port ||
      !arraysEqual(
        settings.data?.tls_extra_sans ?? [],
        baselineRef.current.tls_extra_sans,
      ));

  const httpsEnabled = proxyMode === "https" || proxyMode === "both";

  // 失败保留本地 state 以便用户看到自己改了什么; 不做乐观回滚.
  async function patch(p: Parameters<typeof updateMut.mutateAsync>[0]) {
    try {
      await updateMut.mutateAsync(p);
    } catch (e) {
      alert(`${t("settings.saveFailed")}: ${e}`);
    }
  }

  async function changeLanguage(next: LanguagePref) {
    setPreferredLanguage(next);
    await patch({ preferred_language: next });
  }

  async function changeUpdateSource(next: UpdateSource) {
    await patch({ update_source: next });
  }

  // 调试模式即时生效:pipeline 每次出站读 settings.debug_mode 决定是否落盘.
  async function changeDebugMode(next: boolean) {
    setDebugMode(next);
    await patch({ debug_mode: next });
  }

  async function changeListenAll(next: boolean) {
    setListenAll(next);
    await patch({ listen_all: next });
  }
  async function changeProxyPort(next: number) {
    setPort(next);
    await patch({ proxy_port: next });
  }
  async function changeProxyMode(next: ProxyMode) {
    setProxyMode(next);
    await patch({ proxy_mode: next });
  }
  async function changeHttpsPort(next: number) {
    setHttpsPort(next);
    await patch({ https_port: next });
  }
  async function changeAutostart(next: boolean) {
    setAutostart(next);
    await patch({ autostart: next });
  }
  async function changeRetentionDays(next: number) {
    setRetentionDays(next);
    await patch({ log_retention_days: next });
  }
  async function changeDbLimit(next: number) {
    setDbLimitMb(next);
    await patch({ db_size_limit_mb: next });
  }
  async function changeAuthEnabled(next: boolean) {
    setAuthEnabled(next);
    await patch({ auth_enabled: next });
  }
  async function changeCorsEnabled(next: boolean) {
    setCorsEnabled(next);
    await patch({ cors_enabled: next });
  }
  async function changeCorsOrigin(next: string) {
    await patch({ cors_allow_origin: next });
  }

  async function openDumps() {
    try {
      await api.openDebugDumpDir();
    } catch (e) {
      alert(`${t("settings.debug.open.alertFailed")}: ${e}`);
    }
  }

  async function confirmClearDumps() {
    setClearingDumps(true);
    try {
      await api.clearDebugDumps();
      setClearDumpsDialog(false);
    } catch (e) {
      alert(`${t("settings.debug.clear.alertFailed")}: ${e}`);
    } finally {
      setClearingDumps(false);
    }
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
          <div className="setting-row">
            <div className="label-col">{t("settings.proxy.autostart.label")}</div>
            <Toggle
              checked={autostart}
              onChange={(v) => void changeAutostart(v)}
              aria-label={t("settings.proxy.autostart.label")}
            />
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
              value={settings.data?.update_source ?? "china"}
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
          {/* 协议模式三选一 */}
          <div className="setting-row">
            <div className="label-col">
              {t("settings.proxy.mode.label")}
              <div className="desc">{t("settings.proxy.mode.desc")}</div>
            </div>
            <div
              className="radio-group"
              role="radiogroup"
              aria-label={t("settings.proxy.mode.label")}
              style={{ display: "flex", maxWidth: 360 }}
            >
              <button
                type="button"
                className={proxyMode === "http" ? "on" : ""}
                onClick={() => void changeProxyMode("http")}
                role="radio"
                aria-checked={proxyMode === "http"}
                style={{ flex: 1 }}
              >
                {t("settings.proxy.mode.http")}
              </button>
              <button
                type="button"
                className={proxyMode === "https" ? "on" : ""}
                onClick={() => void changeProxyMode("https")}
                role="radio"
                aria-checked={proxyMode === "https"}
                style={{ flex: 1 }}
              >
                {t("settings.proxy.mode.https")}
              </button>
              <button
                type="button"
                className={proxyMode === "both" ? "on" : ""}
                onClick={() => void changeProxyMode("both")}
                role="radio"
                aria-checked={proxyMode === "both"}
                style={{ flex: 1 }}
              >
                {t("settings.proxy.mode.both")}
              </button>
            </div>
          </div>

          {/* HTTP 端口 (Http / Both 时可见) */}
          {(proxyMode === "http" || proxyMode === "both") && (
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
                  onBlur={() => {
                    if (settings.data && port !== settings.data.proxy_port) {
                      void changeProxyPort(port);
                    }
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                  }}
                  style={{ width: 120 }}
                />
                <span style={{ fontSize: 12, color: "var(--ink-4)" }}>
                  {t("settings.proxy.port.actual")}
                  <span className="mono"> {proxy.data?.http_port ?? "-"}</span>
                </span>
              </div>
            </div>
          )}

          {/* HTTPS 端口 (Https / Both 时可见) */}
          {httpsEnabled && (
            <div className="setting-row">
              <div className="label-col">
                {t("settings.proxy.httpsPort.label")}
                <div className="desc">{t("settings.proxy.httpsPort.desc")}</div>
              </div>
              <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
                <input
                  className="input mono"
                  type="number"
                  value={httpsPort}
                  onChange={(e) => setHttpsPort(Number(e.target.value) || 23457)}
                  onBlur={() => {
                    if (settings.data && httpsPort !== settings.data.https_port) {
                      void changeHttpsPort(httpsPort);
                    }
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                  }}
                  style={{ width: 120 }}
                />
                <span style={{ fontSize: 12, color: "var(--ink-4)" }}>
                  {t("settings.proxy.port.actual")}
                  <span className="mono"> {proxy.data?.https_port ?? "-"}</span>
                </span>
              </div>
            </div>
          )}

          <div className="setting-row">
            <div className="label-col">
              {t("settings.proxy.bind.label")}
              <div className="desc">{t("settings.proxy.bind.desc")}</div>
            </div>
            <div>
              <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
                <div
                  className="radio-group"
                  role="radiogroup"
                  aria-label={t("settings.proxy.bind.label")}
                >
                  <button
                    type="button"
                    className={!listenAll ? "on" : ""}
                    onClick={() => void changeListenAll(false)}
                    role="radio"
                    aria-checked={!listenAll}
                  >
                    {t("settings.proxy.bind.local")}
                  </button>
                  <button
                    type="button"
                    className={listenAll ? "on" : ""}
                    onClick={() => void changeListenAll(true)}
                    role="radio"
                    aria-checked={listenAll}
                  >
                    {t("settings.proxy.bind.lan")}
                  </button>
                </div>
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

          {needsRestart && (
            <div className="alert warn">
              <AlertTriangle size={14} />
              {t("settings.proxy.needsRestart")}
            </div>
          )}
        </div>
      </div>

      {/* HTTPS 证书 (cc-router 自签 CA) — 仅 proxy_mode 包含 https 时显示 */}
      {httpsEnabled && <HttpsCertSection />}

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
                  onChange={(v) => void changeAuthEnabled(v)}
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
                  onChange={(v) => void changeCorsEnabled(v)}
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
                    onBlur={() => {
                      if (
                        settings.data &&
                        corsAllowOrigin !== settings.data.cors_allow_origin
                      ) {
                        void changeCorsOrigin(corsAllowOrigin);
                      }
                    }}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                    }}
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
              onChange={(e) => void changeRetentionDays(Number(e.target.value))}
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
              onChange={(e) => void changeDbLimit(Number(e.target.value))}
            >
              <option value="100">100 MB</option>
              <option value="500">500 MB</option>
              <option value="1024">1 GB</option>
              <option value="10240">{t("settings.storage.dbLimit.unlimited")}</option>
            </select>
          </div>
        </div>
      </div>

      {/* 调试 */}
      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("settings.section.debug")}</div>
        </div>
        <div className="card-body">
          <div className="setting-row">
            <div className="label-col">
              {t("settings.debug.mode.label")}
              <div className="desc">{t("settings.debug.mode.desc")}</div>
            </div>
            <Toggle
              checked={debugMode}
              onChange={(v) => void changeDebugMode(v)}
              aria-label={t("settings.debug.mode.label")}
            />
          </div>
          <div className="setting-row">
            <div className="label-col">
              {t("settings.debug.dumps.label")}
              <div className="desc">{t("settings.debug.dumps.desc")}</div>
            </div>
            <div style={{ display: "flex", gap: 8 }}>
              <button className="btn" type="button" onClick={openDumps}>
                {t("settings.debug.open.button")}
              </button>
              <button
                className="btn"
                type="button"
                onClick={() => setClearDumpsDialog(true)}
              >
                {t("settings.debug.clear.button")}
              </button>
            </div>
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

      {/* 清空 dump 确认弹窗 */}
      <Dialog
        open={clearDumpsDialog}
        onOpenChange={(v) => !clearingDumps && setClearDumpsDialog(v)}
      >
        <DialogContent className="cc-dialog">
          <DialogHeader>
            <DialogTitle>{t("settings.debug.clear.dialog.title")}</DialogTitle>
            <DialogDescription>
              {t("settings.debug.clear.dialog.desc")}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <button
              className="btn"
              onClick={() => setClearDumpsDialog(false)}
              disabled={clearingDumps}
              type="button"
            >
              {t("common.cancel")}
            </button>
            <button
              className="btn danger"
              onClick={confirmClearDumps}
              disabled={clearingDumps}
              type="button"
            >
              {clearingDumps && <Spinner />}
              {t("settings.debug.clear.dialog.confirm")}
            </button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

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

/** TLS 证书管理子组件: 显示 CA 指纹 + 导出 + 重新生成 leaf + 自定义 SAN. 仅 proxy_mode 包含 https 时挂载. */
function HttpsCertSection() {
  const { t } = useT();
  const qc = useQueryClient();
  const settings = useSettings();
  const updateMut = useUpdateSettings();
  // CA 在 app 生命周期内不变, 由 tlsRegenerateLeaf 显式失效, 不需要 focus 重 fetch.
  const tlsStatus = useQuery<TlsStatus>({
    queryKey: ["tlsStatus"],
    queryFn: () => api.tlsGetStatus(),
    staleTime: Infinity,
    refetchOnWindowFocus: false,
  });

  async function onSansBlur(e: React.FocusEvent<HTMLTextAreaElement>) {
    const next = e.target.value
      .split("\n")
      .map((s) => s.trim())
      .filter(Boolean);
    if (arraysEqual(next, settings.data?.tls_extra_sans ?? [])) return;
    try {
      await updateMut.mutateAsync({ tls_extra_sans: next });
      const fresh = await api.tlsRegenerateLeaf();
      qc.setQueryData(["tlsStatus"], fresh);
      alert(t("settings.https.cert.sans.regenerated"));
    } catch (err) {
      alert(`${t("settings.https.cert.regenerateFailed")}: ${err}`);
    }
  }

  async function onExportCa() {
    try {
      const dest = await save({
        defaultPath: "cc-router-ca.pem",
        filters: [{ name: "PEM", extensions: ["pem", "crt"] }],
      });
      if (!dest) return;
      await api.tlsExportCaPem(dest);
      alert(t("settings.https.cert.exportOk"));
    } catch (e) {
      alert(`${t("settings.https.cert.exportFailed")}: ${e}`);
    }
  }

  async function onRegenerate() {
    if (!confirm(t("settings.https.cert.regenerateConfirm"))) return;
    try {
      const fresh = await api.tlsRegenerateLeaf();
      qc.setQueryData(["tlsStatus"], fresh);
      alert(t("settings.https.cert.regenerateOk"));
    } catch (e) {
      alert(`${t("settings.https.cert.regenerateFailed")}: ${e}`);
    }
  }

  const fp = tlsStatus.data?.ca_fingerprint_sha256;
  const shortFp = fp ? `${fp.slice(0, 8)}…${fp.slice(-8)}` : "—";

  return (
    <div className="card section">
      <div className="card-head">
        <div className="card-title">{t("settings.section.https")}</div>
      </div>
      <div className="card-body">
        <div className="setting-row">
          <div className="label-col">
            {t("settings.https.cert.fingerprint.label")}
            <div className="desc">{t("settings.https.cert.fingerprint.desc")}</div>
          </div>
          <span className="mono" style={{ fontSize: 12, color: "var(--ink-2)" }}>
            {shortFp}
          </span>
        </div>
        <div className="setting-row" style={{ display: "block" }}>
          <div className="label-col" style={{ marginBottom: 6 }}>
            {t("settings.https.cert.sans.label")}
            <div className="desc">{t("settings.https.cert.sans.desc")}</div>
          </div>
          <textarea
            // key 让 settings.data 第一次到达时强制 remount, defaultValue 才能生效;
            // 后续 react-query refetch 同值 join 后 key 不变, 不会覆盖用户编辑中的输入.
            key={(settings.data?.tls_extra_sans ?? []).join("\n")}
            className="input mono"
            rows={3}
            placeholder={"192.168.1.5\nmy-laptop.local"}
            style={{ minHeight: 72, fontSize: 12 }}
            defaultValue={(settings.data?.tls_extra_sans ?? []).join("\n")}
            onBlur={onSansBlur}
          />
        </div>
        <div className="setting-row">
          <div className="label-col">
            {t("settings.https.cert.export.label")}
            <div className="desc">{t("settings.https.cert.export.desc")}</div>
          </div>
          <div style={{ display: "flex", gap: 8 }}>
            <button className="btn" type="button" onClick={onExportCa}>
              {t("settings.https.cert.export.button")}
            </button>
            <button className="btn" type="button" onClick={onRegenerate}>
              {t("settings.https.cert.regenerate.button")}
            </button>
          </div>
        </div>
        <div className="setting-row" style={{ display: "block" }}>
          <div className="label-col" style={{ marginBottom: 8 }}>
            {t("settings.https.cert.howto.title")}
          </div>
          <div style={{ fontSize: 12, color: "var(--ink-3)", lineHeight: 1.7 }}>
            <div style={{ marginBottom: 6 }}>
              <strong>macOS</strong>: {t("settings.https.cert.howto.macos")}
            </div>
            <div style={{ marginBottom: 6 }}>
              <strong>Windows</strong>: {t("settings.https.cert.howto.windows")}
            </div>
            <div>
              <strong>Linux</strong>: {t("settings.https.cert.howto.linux")}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
