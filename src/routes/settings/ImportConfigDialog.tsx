import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Toggle } from "@/components/Toggle";
import { Spinner } from "@/components/Spinner";
import { ProviderLogo } from "@/components/ProviderLogo";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { api } from "@/api/tauri";
import { useT } from "@/i18n";
import { errorText } from "@/lib/errorText";
import { useProviders } from "@/hooks/useProviders";
import { SUBSCRIPTIONS_KEY, SUBSCRIPTION_DETAIL_KEY } from "@/hooks/useSubscriptions";
import { ENV_SNIPPET_KEY, SETTINGS_KEY } from "@/hooks/useSettings";
import type { ImportPreview, ImportReport } from "@/types";

const MAX_BYTES = 5 * 1024 * 1024;

export function ImportConfigDialog({
  open,
  onOpenChange,
  onImported,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  onImported?: (report: ImportReport) => void;
}) {
  const { t, locale } = useT();
  const qc = useQueryClient();
  const providers = useProviders();
  const fileInput = useRef<HTMLInputElement>(null);

  const [text, setText] = useState<string | null>(null);
  const [preview, setPreview] = useState<ImportPreview | null>(null);
  const [password, setPassword] = useState("");
  const [skipSecrets, setSkipSecrets] = useState(false);
  const [importToken, setImportToken] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<ImportReport | null>(null);

  useEffect(() => {
    if (!open) reset();
  }, [open]);

  function reset() {
    setText(null);
    setPreview(null);
    setPassword("");
    setSkipSecrets(false);
    setImportToken(false);
    setError(null);
    setReport(null);
    if (fileInput.current) fileInput.current.value = "";
  }

  async function onFile(file: File | undefined) {
    if (!file) return;
    setError(null);
    setReport(null);
    if (file.size > MAX_BYTES) {
      setError(t("backup.import.fileTooLarge"));
      return;
    }
    setBusy(true);
    try {
      const content = await file.text();
      const p = await api.previewConfigImport(content);
      setText(content);
      setPreview(p);
      // 替换令牌会让本机已配好的 Claude Code 立刻 401, 只在「新机器迁移」(本机还没有订阅) 时默认勾选
      setImportToken(p.has_secrets && p.local_subscription_count === 0);
    } catch (e) {
      setText(null);
      setPreview(null);
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  const newCount = preview?.subscriptions.filter((s) => s.status === "new").length ?? 0;
  const needsPassword = !!preview?.has_secrets && !skipSecrets;
  const tokenOnly = newCount === 0 && needsPassword && importToken;
  const canSubmit = !busy && !!text && newCount + (needsPassword && importToken ? 1 : 0) > 0 && (!needsPassword || password.length > 0);

  async function submit() {
    if (!text) return;
    setBusy(true);
    setError(null);
    try {
      const r = await api.applyConfigImport(text, {
        password: needsPassword ? password : undefined,
        skip_secrets: skipSecrets,
        import_token: needsPassword && importToken,
      });
      setReport(r);
      setPreview(null);
      setText(null);
      setPassword("");
      qc.invalidateQueries({ queryKey: SUBSCRIPTIONS_KEY });
      qc.invalidateQueries({ queryKey: SUBSCRIPTION_DETAIL_KEY });
      qc.invalidateQueries({ queryKey: ["virtual-models"] });
      if (r.token_imported) {
        qc.invalidateQueries({ queryKey: SETTINGS_KEY });
        qc.invalidateQueries({ queryKey: ENV_SNIPPET_KEY });
      }
      onImported?.(r);
    } catch (e) {
      setError(`${t("backup.import.failed")}: ${errorText(e)}`);
    } finally {
      setBusy(false);
    }
  }

  function providerLabel(id: string, fallback: string) {
    return providers.data?.find((p) => p.id === id)?.display_name ?? fallback;
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(v) => {
        if (busy) return;
        onOpenChange(v);
      }}
    >
      <DialogContent className="cc-dialog">
        <DialogHeader>
          <DialogTitle>{t("backup.import.title")}</DialogTitle>
          <DialogDescription>{t("settings.backup.desc")}</DialogDescription>
        </DialogHeader>

        <input
          ref={fileInput}
          type="file"
          accept=".json,application/json"
          style={{ display: "none" }}
          onChange={(e) => {
            const file = e.target.files?.[0];
            // Clear so re-picking the same file (after a failed preview) fires change again.
            e.target.value = "";
            void onFile(file);
          }}
        />
        {!report && (
          <button className="btn" type="button" disabled={busy} onClick={() => fileInput.current?.click()}>
            {busy && !preview && <Spinner />}
            {t("backup.import.pick")}
          </button>
        )}

        {preview && (
          <div style={{ display: "flex", flexDirection: "column", gap: 10, marginTop: 12 }}>
            <div className="field-hint">
              {t("backup.import.meta", {
                version: preview.app_version,
                time: new Date(preview.exported_at).toLocaleString(locale),
              })}
            </div>
            <div style={{ maxHeight: 240, overflowY: "auto" }}>
              <table className="table">
                <tbody>
                  {preview.subscriptions.map((s) => (
                    <tr key={s.id} style={{ opacity: s.status === "new" ? 1 : 0.55 }}>
                      <td>
                        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                          <ProviderLogo iconId={s.provider_icon} size={18} />
                          <span>{s.display_name}</span>
                          <span className="field-hint">{providerLabel(s.provider_id, s.provider_display_name)}</span>
                        </div>
                        {s.invalid_reason && (
                          <div className="field-hint" style={{ color: "var(--err)", marginTop: 2 }}>{s.invalid_reason}</div>
                        )}
                      </td>
                      <td style={{ whiteSpace: "nowrap" }}>
                        <span className={s.status === "new" ? "pill" : "pill tag"}>
                          {t(`backup.import.status.${s.status}`)}
                        </span>
                        {s.status === "new" && !s.has_api_key && (
                          <span className="pill tag" style={{ marginLeft: 4 }}>{t("backup.import.noKey")}</span>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>

            {preview.custom_model_conflicts?.length > 0 && (
              <p className="field-hint">
                {t("customModels.importConflicts", { names: preview.custom_model_conflicts.join(", ") })}
              </p>
            )}
            {preview.has_secrets ? (
              <>
                {!skipSecrets && (
                  <input
                    className="input mono"
                    type="password"
                    autoComplete="off"
                    placeholder={t("backup.import.password")}
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                  />
                )}
                <div className="setting-row">
                  <div className="label-col">{t("backup.import.skipSecrets")}</div>
                  <Toggle checked={skipSecrets} onChange={setSkipSecrets} aria-label={t("backup.import.skipSecrets")} />
                </div>
                {!skipSecrets && (
                  <>
                    <div className="setting-row">
                      <div className="label-col">{t("backup.import.importToken")}</div>
                      <Toggle checked={importToken} onChange={setImportToken} aria-label={t("backup.import.importToken")} />
                    </div>
                    <div className="field-hint">{t("backup.import.importTokenHint")}</div>
                  </>
                )}
              </>
            ) : (
              <div className="field-hint">{t("backup.import.noSecretsHint")}</div>
            )}
          </div>
        )}

        {report && (
          <ul style={{ paddingLeft: 20, fontSize: 13, lineHeight: 1.8 }}>
            <li>{t("backup.import.report.imported", { count: report.imported })}</li>
            {report.skipped_existing > 0 && (
              <li>{t("backup.import.report.skippedExisting", { count: report.skipped_existing })}</li>
            )}
            {report.skipped_oauth.length > 0 && (
              <li>{t("backup.import.report.skippedOauth", { names: report.skipped_oauth.join(t("backup.import.nameSeparator")) })}</li>
            )}
            {report.skipped_invalid.length > 0 && (
              <li>
                {t("backup.import.report.skippedInvalid", { count: report.skipped_invalid.length })}
                <ul style={{ paddingLeft: 18 }}>
                  {report.skipped_invalid.map((e) => (
                    <li key={e.name + e.reason}>{t("backup.import.report.invalidItem", { name: e.name, reason: e.reason })}</li>
                  ))}
                </ul>
              </li>
            )}
            {(report.skipped_bindings?.length ?? 0) > 0 && (
              <li>
                {t("backup.import.report.skippedBindings", { count: report.skipped_bindings!.length })}
                <ul style={{ paddingLeft: 18 }}>
                  {report.skipped_bindings!.map((e) => (
                    <li key={e.name + e.reason}>{t("backup.import.report.invalidItem", { name: e.name, reason: e.reason })}</li>
                  ))}
                </ul>
              </li>
            )}
            {report.disabled_missing_key.length > 0 && (
              <li>{t("backup.import.report.missingKey", { names: report.disabled_missing_key.join(t("backup.import.nameSeparator")) })}</li>
            )}
            {report.token_imported && <li>{t("backup.import.report.tokenImported")}</li>}
            {report.token_error && (
              <li style={{ color: "var(--err)" }}>{t("backup.import.report.tokenFailed", { reason: report.token_error })}</li>
            )}
          </ul>
        )}

        {error && <div className="field-hint" style={{ marginTop: 12, color: "var(--err)" }}>{error}</div>}

        <DialogFooter>
          <button className="btn" type="button" onClick={() => onOpenChange(false)} disabled={busy}>
            {t("common.close")}
          </button>
          {preview && (
            <button className="btn primary" type="button" onClick={submit} disabled={!canSubmit}>
              {busy && <Spinner />}
              {newCount > 0
                ? t("backup.import.confirm", { count: newCount })
                : tokenOnly
                  ? t("backup.import.confirmTokenOnly")
                  : t("backup.import.nothing")}
            </button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
