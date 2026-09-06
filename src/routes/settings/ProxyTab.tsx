import { TriangleAlert } from "lucide-react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useT } from "@/i18n";
import { HttpsCertSection } from "./HttpsCertSection";
import type { SettingsForm } from "./useSettingsForm";

/** 代理: 协议模式 / 端口 / 监听地址 / 请求体上限 + (https 时) 证书. */
export function ProxyTab({ form }: { form: SettingsForm }) {
  const { t } = useT();
  const { settings, proxy, proxyMode, port, httpsPort, listenAll, httpsEnabled } = form;

  return (
    <>
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
              {(["http", "https", "both"] as const).map((mode) => (
                <button
                  key={mode}
                  type="button"
                  className={proxyMode === mode ? "on" : ""}
                  onClick={() => void form.changeProxyMode(mode)}
                  role="radio"
                  aria-checked={proxyMode === mode}
                  style={{ flex: 1 }}
                >
                  {t(`settings.proxy.mode.${mode}`)}
                </button>
              ))}
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
                  onChange={(e) => form.setPort(Number(e.target.value) || 23456)}
                  onBlur={() => {
                    if (settings.data && port !== settings.data.proxy_port) {
                      void form.changeProxyPort(port);
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
                  onChange={(e) => form.setHttpsPort(Number(e.target.value) || 23457)}
                  onBlur={() => {
                    if (settings.data && httpsPort !== settings.data.https_port) {
                      void form.changeHttpsPort(httpsPort);
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
                    onClick={() => void form.changeListenAll(false)}
                    role="radio"
                    aria-checked={!listenAll}
                  >
                    {t("settings.proxy.bind.local")}
                  </button>
                  <button
                    type="button"
                    className={listenAll ? "on" : ""}
                    onClick={() => void form.changeListenAll(true)}
                    role="radio"
                    aria-checked={listenAll}
                  >
                    {t("settings.proxy.bind.lan")}
                  </button>
                </div>
                <span
                  className="mono"
                  style={{ fontSize: 12, color: listenAll ? "var(--ink-2)" : "var(--ink-4)" }}
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

          {/* 请求体上限: axum 默认 2 MiB 会让 Codex 多图请求 413 (issue #41) */}
          <div className="setting-row">
            <div className="label-col">
              {t("settings.proxy.bodyLimit.label")}
              <div className="desc">{t("settings.proxy.bodyLimit.desc")}</div>
            </div>
            <Select
              value={String(form.maxBodyMb)}
              onValueChange={(v) => void form.changeMaxBodyMb(Number(v))}
            >
              <SelectTrigger style={{ maxWidth: 200 }}>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="8">8 MB</SelectItem>
                <SelectItem value="16">16 MB</SelectItem>
                <SelectItem value="32">{t("settings.proxy.bodyLimit.default32")}</SelectItem>
                <SelectItem value="64">64 MB</SelectItem>
                <SelectItem value="128">128 MB</SelectItem>
              </SelectContent>
            </Select>
          </div>

          {form.needsRestart && (
            <div className="alert warn">
              <TriangleAlert size={14} />
              {t("settings.proxy.needsRestart")}
            </div>
          )}
        </div>
      </div>

      {/* HTTPS 证书 (cc-router 自签 CA) — 仅 proxy_mode 包含 https 时显示 */}
      {httpsEnabled && <HttpsCertSection />}
    </>
  );
}
