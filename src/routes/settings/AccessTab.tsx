import { useEffect, useRef, useState } from "react";
import { RefreshCw, Check, Copy } from "lucide-react";
import { Toggle } from "@/components/Toggle";
import { Spinner } from "@/components/Spinner";
import { CopyableBlock } from "@/components/CopyableBlock";
import { useGenerateNewToken, useLanAddresses, useProxyStatus } from "@/hooks/useSettings";
import { useT } from "@/i18n";
import { runtime } from "@/runtime";
import type { SettingsForm } from "./useSettingsForm";

/** 安全与访问: 鉴权 token / CORS / 网页界面. */
export function AccessTab({ form }: { form: SettingsForm }) {
  const { t } = useT();
  const { settings } = form;
  const generateTokenMut = useGenerateNewToken();
  const [tokenJustRegenerated, setTokenJustRegenerated] = useState(false);
  const [tokenCopied, setTokenCopied] = useState(false);
  const regenerateTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const copyTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (regenerateTimerRef.current) clearTimeout(regenerateTimerRef.current);
      if (copyTimerRef.current) clearTimeout(copyTimerRef.current);
    },
    [],
  );

  async function copyToken() {
    if (!settings.data) return;
    try {
      await runtime.copyText(settings.data.auth_token);
    } catch {
      /* ignore: 与 CopyableBlock 一致, 剪贴板失败不打断 */
    }
    setTokenCopied(true);
    if (copyTimerRef.current) clearTimeout(copyTimerRef.current);
    copyTimerRef.current = setTimeout(() => setTokenCopied(false), 1500);
  }

  async function regenerateToken() {
    try {
      await generateTokenMut.mutateAsync();
      setTokenJustRegenerated(true);
      if (regenerateTimerRef.current) clearTimeout(regenerateTimerRef.current);
      regenerateTimerRef.current = setTimeout(() => setTokenJustRegenerated(false), 2000);
    } catch (e) {
      alert(`${t("settings.auth.token.alertFailed")}: ${e}`);
    }
  }

  return (
    <>
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
                  checked={form.authEnabled}
                  onChange={(v) => void form.changeAuthEnabled(v)}
                  aria-label={t("settings.auth.token.label")}
                />
                <span style={{ fontSize: 12, color: "var(--ink-2)" }}>
                  {form.authEnabled
                    ? t("settings.auth.token.enabled")
                    : t("settings.auth.token.disabled")}
                </span>
              </div>
              {form.authEnabled && settings.data && (
                <div style={{ display: "flex", gap: 8 }}>
                  <input
                    className="input mono"
                    value={settings.data.auth_token}
                    readOnly
                    style={{ fontSize: 11.5, color: "var(--ink-2)" }}
                  />
                  <button className="btn" onClick={copyToken} type="button">
                    {tokenCopied ? (
                      <Check size={12} style={{ color: "var(--ok)" }} />
                    ) : (
                      <Copy size={12} />
                    )}
                    {tokenCopied ? t("common.copied") : t("common.copy")}
                  </button>
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
                {form.corsEnabled
                  ? t("settings.auth.cors.descEnabled")
                  : t("settings.auth.cors.descDisabled")}
              </div>
            </div>
            <div>
              <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 10 }}>
                <Toggle
                  checked={form.corsEnabled}
                  onChange={(v) => void form.changeCorsEnabled(v)}
                  aria-label={t("settings.auth.cors.label")}
                />
                <span style={{ fontSize: 12, color: "var(--ink-2)" }}>
                  {form.corsEnabled
                    ? t("settings.auth.cors.statusEnabled")
                    : t("settings.auth.cors.statusDisabled")}
                </span>
              </div>
              {form.corsEnabled && (
                <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                  <input
                    className="input mono"
                    value={form.corsAllowOrigin}
                    onChange={(e) => form.setCorsAllowOrigin(e.target.value)}
                    onBlur={() => {
                      if (
                        settings.data &&
                        form.corsAllowOrigin !== settings.data.cors_allow_origin
                      ) {
                        void form.changeCorsOrigin(form.corsAllowOrigin);
                      }
                    }}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                    }}
                    placeholder="*"
                    style={{ maxWidth: 280 }}
                  />
                  <span className="mono" style={{ fontSize: 11.5, color: "var(--ink-4)" }}>
                    Access-Control-Allow-Origin
                  </span>
                </div>
              )}
            </div>
          </div>
        </div>
      </div>

      {/* 网页界面 */}
      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("settings.section.webUi")}</div>
        </div>
        <div className="card-body">
          <div className="setting-row">
            <div className="label-col">
              {t("settings.webUi.enabled.label")}
              <div className="desc">{t("settings.webUi.enabled.desc")}</div>
            </div>
            <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
              <Toggle
                checked={form.webUiEnabled}
                onChange={(v) => void form.changeWebUiEnabled(v)}
                aria-label={t("settings.webUi.enabled.label")}
              />
              <span style={{ fontSize: 12, color: "var(--ink-2)" }}>
                {form.webUiEnabled ? t("settings.webUi.enabled.on") : t("settings.webUi.enabled.off")}
              </span>
            </div>
          </div>
          <div className="setting-row">
            <div className="label-col">
              {t("settings.webUi.auth.label")}
              <div className="desc">{t("settings.webUi.auth.desc")}</div>
            </div>
            <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
              <Toggle
                checked={form.webUiAuthEnabled}
                disabled={!form.webUiEnabled}
                onChange={(v) => void form.changeWebUiAuthEnabled(v)}
                aria-label={t("settings.webUi.auth.label")}
              />
              <span style={{ fontSize: 12, color: "var(--ink-2)" }}>
                {form.webUiAuthEnabled ? t("settings.webUi.auth.on") : t("settings.webUi.auth.off")}
              </span>
            </div>
          </div>
          {form.webUiEnabled && (
            <WebUiAddresses listenAll={form.listenAll} authEnabled={form.webUiAuthEnabled} />
          )}
        </div>
      </div>
    </>
  );
}

function WebUiAddresses({ listenAll, authEnabled }: { listenAll: boolean; authEnabled: boolean }) {
  const { t } = useT();
  const proxy = useProxyStatus();
  const lan = useLanAddresses(listenAll);
  const scheme = proxy.data?.http_port ? "http" : "https";
  const port = proxy.data?.http_port ?? proxy.data?.https_port ?? 23456;
  const urls = [`${scheme}://127.0.0.1:${port}/ui/`];
  if (listenAll) {
    for (const ip of lan.data ?? []) urls.push(`${scheme}://${ip}:${port}/ui/`);
  }
  return (
    <div className="setting-row">
      <div className="label-col">
        {t("settings.webUi.addresses.label")}
        {!listenAll && <div className="desc">{t("settings.webUi.addresses.localOnly")}</div>}
      </div>
      <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
        {urls.map((u) => (
          <CopyableBlock key={u} text={u} variant="inline" />
        ))}
        {listenAll && (
          <div className={authEnabled ? "alert warn" : "alert err"} style={{ marginTop: 6 }}>
            {authEnabled ? t("settings.webUi.warn.lanWithAuth") : t("settings.webUi.warn.lanNoAuth")}
          </div>
        )}
      </div>
    </div>
  );
}
