import { useEffect, useRef, useState } from "react";
import { useProxyStatus, useSettings, useUpdateSettings } from "@/hooks/useSettings";
import { useT, type LanguagePref } from "@/i18n";
import { runtime } from "@/runtime";
import type { ProxyMode, UpdateSource } from "@/types";

export function arraysEqual<T>(a: readonly T[], b: readonly T[]): boolean {
  return a.length === b.length && a.every((v, i) => v === b[i]);
}

/**
 * 设置页的表单状态 + 保存 handler, 由 SettingsPage 持有、以 `form` prop 传给各 tab.
 *
 * 必须放在页面层而不是各 tab 里: tab 切换会卸载子组件, 若本地 state / baseline 跟着
 * 子组件走, 切一次 tab 就会把「需要重启」的判定基准重置成已保存的新值, 提示凭空消失.
 */
export function useSettingsForm() {
  const { t } = useT();
  const settings = useSettings();
  const proxy = useProxyStatus();
  const updateMut = useUpdateSettings();

  const [port, setPort] = useState<number>(23456);
  const [proxyMode, setProxyMode] = useState<ProxyMode>("http");
  const [httpsPort, setHttpsPort] = useState<number>(23457);
  const [listenAll, setListenAll] = useState(false);
  const [maxBodyMb, setMaxBodyMb] = useState(32);
  const [autostart, setAutostart] = useState(false);
  const [retentionDays, setRetentionDays] = useState(30);
  const [dbLimitMb, setDbLimitMb] = useState(500);
  const [authEnabled, setAuthEnabled] = useState(true);
  const [corsEnabled, setCorsEnabled] = useState(true);
  const [corsAllowOrigin, setCorsAllowOrigin] = useState("*");
  const [preferredLanguage, setPreferredLanguage] = useState<LanguagePref>("system");
  const [debugMode, setDebugMode] = useState(false);
  const [webUiEnabled, setWebUiEnabled] = useState(false);
  const [webUiAuthEnabled, setWebUiAuthEnabled] = useState(true);
  const [tuiEnabled, setTuiEnabled] = useState(false);

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
    https_enable_h2: boolean;
    max_request_body_mb: number;
  } | null>(null);

  useEffect(() => {
    if (!settings.data || initializedRef.current) return;
    baselineRef.current = {
      proxy_port: settings.data.proxy_port,
      listen_all: settings.data.listen_all,
      proxy_mode: settings.data.proxy_mode ?? "http",
      https_port: settings.data.https_port ?? 23457,
      tls_extra_sans: settings.data.tls_extra_sans ?? [],
      https_enable_h2: settings.data.https_enable_h2 ?? true,
      max_request_body_mb: settings.data.max_request_body_mb ?? 32,
    };
    setPort(settings.data.proxy_port);
    setProxyMode(settings.data.proxy_mode ?? "http");
    setHttpsPort(settings.data.https_port ?? 23457);
    setListenAll(settings.data.listen_all);
    setMaxBodyMb(settings.data.max_request_body_mb ?? 32);
    setAutostart(settings.data.autostart);
    setRetentionDays(settings.data.log_retention_days);
    setDbLimitMb(settings.data.db_size_limit_mb);
    setAuthEnabled(settings.data.auth_enabled);
    setCorsEnabled(settings.data.cors_enabled);
    setCorsAllowOrigin(settings.data.cors_allow_origin);
    setPreferredLanguage(settings.data.preferred_language ?? "system");
    setDebugMode(settings.data.debug_mode ?? false);
    setWebUiEnabled(settings.data.web_ui_enabled);
    setWebUiAuthEnabled(settings.data.web_ui_auth_enabled);
    setTuiEnabled(settings.data.tui_enabled);
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
      ) ||
      (settings.data?.https_enable_h2 ?? true) !==
        baselineRef.current.https_enable_h2 ||
      maxBodyMb !== baselineRef.current.max_request_body_mb);

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
  async function changeMaxBodyMb(next: number) {
    setMaxBodyMb(next);
    await patch({ max_request_body_mb: next });
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
  async function changeWebUiEnabled(next: boolean) {
    if (!next && runtime.kind === "web" && !confirm(t("settings.webUi.selfDisable.confirm"))) return;
    setWebUiEnabled(next);
    await patch({ web_ui_enabled: next });
  }
  async function changeWebUiAuthEnabled(next: boolean) {
    if (!next && !confirm(t("settings.webUi.auth.confirmOff"))) return;
    if (next && runtime.kind === "web") alert(t("settings.webUi.auth.relogin"));
    setWebUiAuthEnabled(next);
    await patch({ web_ui_auth_enabled: next });
  }
  // 即时生效: 中间件每请求读 settings.tui_enabled。
  async function changeTuiEnabled(next: boolean) {
    setTuiEnabled(next);
    await patch({ tui_enabled: next });
  }
  async function changeCorsEnabled(next: boolean) {
    setCorsEnabled(next);
    await patch({ cors_enabled: next });
  }
  async function changeCorsOrigin(next: string) {
    await patch({ cors_allow_origin: next });
  }

  return {
    settings,
    proxy,
    needsRestart,
    httpsEnabled,
    port,
    setPort,
    proxyMode,
    httpsPort,
    setHttpsPort,
    listenAll,
    maxBodyMb,
    autostart,
    retentionDays,
    dbLimitMb,
    authEnabled,
    corsEnabled,
    corsAllowOrigin,
    setCorsAllowOrigin,
    preferredLanguage,
    debugMode,
    webUiEnabled,
    webUiAuthEnabled,
    tuiEnabled,
    changeLanguage,
    changeUpdateSource,
    changeDebugMode,
    changeListenAll,
    changeMaxBodyMb,
    changeProxyPort,
    changeProxyMode,
    changeHttpsPort,
    changeAutostart,
    changeRetentionDays,
    changeDbLimit,
    changeAuthEnabled,
    changeWebUiEnabled,
    changeWebUiAuthEnabled,
    changeTuiEnabled,
    changeCorsEnabled,
    changeCorsOrigin,
  };
}

export type SettingsForm = ReturnType<typeof useSettingsForm>;
