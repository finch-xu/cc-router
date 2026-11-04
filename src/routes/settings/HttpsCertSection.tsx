import type React from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Toggle } from "@/components/Toggle";
import { api } from "@/api/tauri";
import { useSettings, useUpdateSettings } from "@/hooks/useSettings";
import { useT } from "@/i18n";
import { runtime } from "@/runtime";
import type { TlsStatus } from "@/types";
import { arraysEqual } from "./useSettingsForm";
import { errorText } from "@/lib/errorText";

/** TLS 证书管理子组件: 显示 CA 指纹 + 导出 + 重新生成 leaf + 自定义 SAN. 仅 proxy_mode 包含 https 时挂载. */
export function HttpsCertSection() {
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
      alert(`${t("settings.https.cert.regenerateFailed")}: ${errorText(err)}`);
    }
  }

  async function onExportCa() {
    try {
      // zip 里同一份 PEM 落 .pem + .crt 两个后缀: Windows 双击 .crt 即装, 其余平台用 .pem。
      const name = caZipName(new Date());
      if (runtime.kind === "web") {
        const bytes = await api.tlsGetCaZipBytes();
        runtime.downloadBytes(name, new Uint8Array(bytes), "application/zip");
        alert(t("settings.https.cert.exportOk"));
        return;
      }
      const dest = await runtime.pickSavePath({
        defaultName: name,
        filters: [{ name: "Zip", extensions: ["zip"] }],
      });
      if (!dest) return;
      await api.tlsExportCaZip(/\.zip$/i.test(dest) ? dest : `${dest}.zip`);
      alert(t("settings.https.cert.exportOk"));
    } catch (e) {
      alert(`${t("settings.https.cert.exportFailed")}: ${errorText(e)}`);
    }
  }

  async function onRegenerate() {
    if (!confirm(t("settings.https.cert.regenerateConfirm"))) return;
    try {
      const fresh = await api.tlsRegenerateLeaf();
      qc.setQueryData(["tlsStatus"], fresh);
      alert(t("settings.https.cert.regenerateOk"));
    } catch (e) {
      alert(`${t("settings.https.cert.regenerateFailed")}: ${errorText(e)}`);
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
            {t("settings.https.h2.label")}
            <div className="desc">{t("settings.https.h2.desc")}</div>
          </div>
          <Toggle
            checked={settings.data?.https_enable_h2 ?? true}
            onChange={(v) => {
              void (async () => {
                try {
                  await updateMut.mutateAsync({ https_enable_h2: v });
                } catch (e) {
                  alert(`${t("settings.saveFailed")}: ${errorText(e)}`);
                }
              })();
            }}
            aria-label={t("settings.https.h2.label")}
          />
        </div>
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

/** `cc-router-ca-YYYYMMDD-HHMMSS.zip`, 本地时间 (本地 getter, 不用 toISOString)。 */
export function caZipName(d: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  const day = `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}`;
  const time = `${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
  return `cc-router-ca-${day}-${time}.zip`;
}
