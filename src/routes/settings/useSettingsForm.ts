import { useEffect, useRef, useState } from "react";
import { useProxyStatus, useRestartProxy, useSettings, useUpdateSettings } from "@/hooks/useSettings";
import { useT, type LanguagePref } from "@/i18n";
import { webRestartWarning, webUrlAfterRestart } from "@/lib/proxyRestart";
import { runtime } from "@/runtime";
import type { ProxyMode, RestartProxyResult, UpdateSource } from "@/types";
import { errorText } from "@/lib/errorText";

export function arraysEqual<T>(a: readonly T[], b: readonly T[]): boolean {
  return a.length === b.length && a.every((v, i) => v === b[i]);
}

/**
 * 设置页的表单状态 + 保存 handler, 由 SettingsPage 持有、以 `form` prop 传给各 tab.
 *
 * 必须放在页面层而不是各 tab 里: tab 切换会卸载子组件, 若本地 state 跟着
 * 子组件走, 切一次 tab 就会把正在编辑的值丢掉.
 */
export function useSettingsForm() {
  const { t } = useT();
  const settings = useSettings();
  const proxy = useProxyStatus();
  const updateMut = useUpdateSettings();
  const restartMut = useRestartProxy();
  // 上一次重启的结果, 取自 mutation 自身的 data, 下一次设置保存成功时 reset
  const restartResult: RestartProxyResult | null = restartMut.data ?? null;
  // 需要「重启代理服务」才生效的判定由后端比较生效配置得出, 刷新页面 / 切走再回来都不丢.
  const restartPending = proxy.data?.restart_pending ?? false;

  const [port, setPort] = useState<number>(23456);
  const [proxyMode, setProxyMode] = useState<ProxyMode>("http");
  const [httpsPort, setHttpsPort] = useState<number>(23457);
  const [listenAll, setListenAll] = useState(false);
  const [maxBodyMb, setMaxBodyMb] = useState(128);
  const [autostart, setAutostart] = useState(false);
  const [retentionDays, setRetentionDays] = useState(30);
  const [dbLimitMb, setDbLimitMb] = useState(500);
  const [authEnabled, setAuthEnabled] = useState(true);
  const [corsEnabled, setCorsEnabled] = useState(true);
  const [corsAllowOrigin, setCorsAllowOrigin] = useState("*");
  const [preferredLanguage, setPreferredLanguage] = useState<LanguagePref>("system");
  const [debugMode, setDebugMode] = useState(false);
  const [logRequestHeaders, setLogRequestHeaders] = useState(true);
  const [webUiEnabled, setWebUiEnabled] = useState(false);
  const [webUiAuthEnabled, setWebUiAuthEnabled] = useState(true);
  const [tuiEnabled, setTuiEnabled] = useState(false);

  // 仅在首次拿到 settings.data 时灌入本地 state. 后续 mutate refetch
  // 不再回灌, 否则会覆盖用户正在 input 里编辑但尚未 blur 的值 (port/cors origin 跳光标).
  const initializedRef = useRef(false);

  useEffect(() => {
    if (!settings.data || initializedRef.current) return;
    setPort(settings.data.proxy_port);
    setProxyMode(settings.data.proxy_mode ?? "http");
    setHttpsPort(settings.data.https_port ?? 23457);
    setListenAll(settings.data.listen_all);
    setMaxBodyMb(settings.data.max_request_body_mb ?? 128);
    setAutostart(settings.data.autostart);
    setRetentionDays(settings.data.log_retention_days);
    setDbLimitMb(settings.data.db_size_limit_mb);
    setAuthEnabled(settings.data.auth_enabled);
    setCorsEnabled(settings.data.cors_enabled);
    setCorsAllowOrigin(settings.data.cors_allow_origin);
    setPreferredLanguage(settings.data.preferred_language ?? "system");
    setDebugMode(settings.data.debug_mode ?? false);
    setLogRequestHeaders(settings.data.log_request_headers ?? true);
    setWebUiEnabled(settings.data.web_ui_enabled);
    setWebUiAuthEnabled(settings.data.web_ui_auth_enabled);
    setTuiEnabled(settings.data.tui_enabled);
    initializedRef.current = true;
  }, [settings.data]);

  const httpsEnabled = proxyMode === "https" || proxyMode === "both";

  // 最近一次保存的 promise 与「重启是否在途」: 输入框失焦触发保存后紧接着点重启按钮时,
  // 重启要等保存落盘再读设置; 保存完成也不能清掉在途重启的结果.
  const lastPatchRef = useRef<Promise<void> | null>(null);
  const restartInFlightRef = useRef(false);

  // 失败保留本地 state 以便用户看到自己改了什么; 不做乐观回滚.
  function patch(p: Parameters<typeof updateMut.mutateAsync>[0]): Promise<void> {
    const run = (async () => {
      try {
        await updateMut.mutateAsync(p);
        if (!restartInFlightRef.current) restartMut.reset();
      } catch (e) {
        alert(`${t("settings.saveFailed")}: ${errorText(e)}`);
      }
    })();
    lastPatchRef.current = run;
    return run;
  }

  async function changeLanguage(next: LanguagePref) {
    setPreferredLanguage(next);
    await patch({ preferred_language: next });
  }
  async function changeUpdateSource(next: UpdateSource) {
    await patch({ update_source: next });
  }
  // 与更新源一样不留本地副本: 更新页也有同一个开关, 两边都读 settings 缓存
  async function changeAutoCheckUpdates(next: boolean) {
    await patch({ auto_check_updates: next });
  }
  // 调试模式即时生效:pipeline 每次出站读 settings.debug_mode 决定是否落盘.
  async function changeDebugMode(next: boolean) {
    setDebugMode(next);
    await patch({ debug_mode: next });
  }
  async function changeLogRequestHeaders(next: boolean) {
    setLogRequestHeaders(next);
    await patch({ log_request_headers: next });
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

  async function restartProxy() {
    // 保存失败已在 patch 里提示过, 这里只等它结束
    await lastPatchRef.current?.catch(() => {});
    // 网页端要按刚保存的设置判断重启后地址会不会变, 缓存里的可能还是旧的
    const s = runtime.kind === "web" ? (await settings.refetch()).data : settings.data;
    if (runtime.kind === "web" && s) {
      const warn = webRestartWarning(window.location, {
        proxy_mode: s.proxy_mode ?? "http",
        proxy_port: s.proxy_port,
        https_port: s.https_port ?? 23457,
        listen_all: s.listen_all,
      });
      if (warn && !confirm(t(warn))) return;
    }
    restartInFlightRef.current = true;
    try {
      const r = await restartMut.mutateAsync();
      if (runtime.kind === "web" && r.outcome === "applied") {
        const target = webUrlAfterRestart(window.location, r.status);
        if (target) window.location.replace(target);
      }
    } catch (e) {
      alert(`${t("settings.proxy.restart.requestFailed")}: ${errorText(e)}`);
    } finally {
      restartInFlightRef.current = false;
    }
  }

  return {
    settings,
    proxy,
    restartPending,
    restartResult,
    restarting: restartMut.isPending,
    restartProxy,
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
    logRequestHeaders,
    webUiEnabled,
    webUiAuthEnabled,
    tuiEnabled,
    changeLanguage,
    changeUpdateSource,
    changeAutoCheckUpdates,
    changeDebugMode,
    changeLogRequestHeaders,
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
