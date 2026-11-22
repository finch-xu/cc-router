import { useEffect, useState } from "react";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Spinner } from "@/components/Spinner";
import { api } from "@/api/tauri";
import { useT } from "@/i18n";
import { errorText } from "@/lib/errorText";
import { localized, type LocalizedText } from "@/plugins/manifest";
import { runtime } from "@/runtime";
import type { InstalledPlugin, NotifierDeliveryRecord, RuleParamDecl } from "@/types";

const VIRTUAL_MODEL_OPTIONS = [
  { value: "fable", label: "model-fable" },
  { value: "opus", label: "model-opus" },
  { value: "sonnet", label: "model-sonnet" },
  { value: "haiku", label: "model-haiku" },
  { value: "fallback", label: "model-fallback" },
];

function initialDraft(plugin: InstalledPlugin | null): Record<string, string | boolean> {
  const out: Record<string, string | boolean> = {};
  if (!plugin?.param_decls) return out;
  for (const decl of plugin.param_decls) {
    if (decl.type === "secret") {
      // secret 永不回显明文, 始终从空输入开始; 留空保存时后端保留已有密文
      out[decl.id] = "";
      continue;
    }
    const raw = plugin.params?.[decl.id] ?? decl.default;
    if (decl.type === "bool") {
      out[decl.id] = typeof raw === "boolean" ? raw : false;
    } else if (raw !== undefined && raw !== null) {
      out[decl.id] = String(raw);
    } else {
      out[decl.id] = "";
    }
  }
  return out;
}

function buildPayload(
  decls: RuleParamDecl[],
  draft: Record<string, string | boolean>,
  clearedSecrets: Record<string, boolean>,
): { ok: true; data: Record<string, unknown> } | { ok: false; field: string } {
  const data: Record<string, unknown> = {};
  for (const decl of decls) {
    const v = draft[decl.id];
    if (decl.type === "bool") {
      data[decl.id] = Boolean(v);
      continue;
    }
    const text = typeof v === "string" ? v.trim() : "";
    if (decl.type === "secret") {
      if (clearedSecrets[decl.id]) {
        data[decl.id] = null;
      } else if (text) {
        data[decl.id] = text;
      }
      // 留空且未点清除 → 不传该键, 后端保留原密文
      continue;
    }
    if (!text) {
      data[decl.id] = null;
      continue;
    }
    if (decl.type === "number") {
      const n = Number(text);
      if (!Number.isFinite(n)) return { ok: false, field: decl.id };
      data[decl.id] = n;
    } else {
      data[decl.id] = text;
    }
  }
  return { ok: true, data };
}

function pluginName(p: InstalledPlugin, locale: string): string {
  const m = p.manifest as { name?: LocalizedText } | null;
  return localized(m?.name, locale) || p.id;
}

/**
 * 规则 / 通知插件的参数设置对话框: 按插件声明的 `param_decls` 动态生成表单, 保存后由 Rust 按类型校验。
 * 通知插件额外支持测试推送与最近发送记录查看。
 * `promptEnable`: 用户尝试启用缺参的插件时打开, 顶部给出提示并提供「保存并启用」。
 */
export function ParamsDialog({
  plugin,
  promptEnable = false,
  onClose,
}: {
  plugin: InstalledPlugin | null;
  promptEnable?: boolean;
  onClose: () => void;
}) {
  const { t, locale } = useT();
  const isDesktop = runtime.kind === "desktop";
  const isNotifier = plugin?.plugin_type === "notifier";
  const secretsSet = new Set(plugin?.secret_params_set ?? []);
  const [draft, setDraft] = useState<Record<string, string | boolean>>({});
  const [clearedSecrets, setClearedSecrets] = useState<Record<string, boolean>>({});
  const [busy, setBusy] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<NotifierDeliveryRecord | null>(null);
  const [deliveries, setDeliveries] = useState<NotifierDeliveryRecord[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setDraft(initialDraft(plugin));
    setClearedSecrets({});
    setError(null);
    setBusy(false);
    setTesting(false);
    setTestResult(null);
    if (plugin && plugin.plugin_type === "notifier") {
      api.listNotifierDeliveries(plugin.id).then(setDeliveries).catch(() => setDeliveries([]));
    } else {
      setDeliveries([]);
    }
  }, [plugin]);

  const decls = plugin?.param_decls ?? [];

  async function submit(andEnable: boolean) {
    if (!plugin) return;
    const built = buildPayload(decls, draft, clearedSecrets);
    if (!built.ok) {
      setError(t("plugins.params.invalidNumber", { field: built.field }));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await api.setPluginParams(plugin.id, built.data);
      if (andEnable && !plugin.enabled && isDesktop) {
        await api.enablePlugin(plugin.id);
      }
      onClose();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  async function saveAndTest() {
    if (!plugin || !isNotifier || !isDesktop) return;
    const built = buildPayload(decls, draft, clearedSecrets);
    if (!built.ok) {
      setError(t("plugins.params.invalidNumber", { field: built.field }));
      return;
    }
    setTesting(true);
    setError(null);
    setTestResult(null);
    try {
      await api.setPluginParams(plugin.id, built.data);
      const rec = await api.testNotifierPlugin(plugin.id);
      setTestResult(rec);
      const list = await api.listNotifierDeliveries(plugin.id).catch(() => []);
      setDeliveries(list);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setTesting(false);
    }
  }

  const eventLabel = (ev: string) => {
    const key = `notify.event.${ev}`;
    const label = t(key);
    return label === key ? ev : label;
  };

  return (
    <Dialog open={!!plugin} onOpenChange={(v) => !v && !busy && !testing && onClose()}>
      <DialogContent className="cc-dialog">
        {plugin && (
          <>
            <DialogHeader>
              <DialogTitle>{t("plugins.params.title", { name: pluginName(plugin, locale) })}</DialogTitle>
              <DialogDescription>{isNotifier ? t("plugins.params.notifierDesc") : t("plugins.params.desc")}</DialogDescription>
            </DialogHeader>

            {promptEnable && <div className="pg-alert warn">{t("plugins.params.requiredBeforeEnable")}</div>}

            <div className="pg-params-form">
              {decls.map((decl) => {
                const label = localized(decl.label, locale) || decl.id;
                const required = decl.type === "secret" ? !decl.optional : !decl.optional && decl.default === undefined;
                const value = draft[decl.id];
                if (decl.type === "bool") {
                  return (
                    <label key={decl.id} className="pg-param-bool">
                      <input
                        type="checkbox"
                        checked={Boolean(value)}
                        onChange={(e) => setDraft((prev) => ({ ...prev, [decl.id]: e.target.checked }))}
                      />
                      <span>{label}</span>
                    </label>
                  );
                }
                const hasSavedSecret = decl.type === "secret" && secretsSet.has(decl.id) && !clearedSecrets[decl.id];
                return (
                  <div key={decl.id} className="pg-param-row">
                    <label className="pg-param-label" htmlFor={`pg-param-${decl.id}`}>
                      <span>{label}</span>
                      {required && <span className="pg-param-req">{t("plugins.params.required")}</span>}
                      {hasSavedSecret && <span className="pg-badge ok">{t("plugins.params.secretSaved")}</span>}
                    </label>
                    {decl.type === "virtual_model" ? (
                      <select
                        id={`pg-param-${decl.id}`}
                        className="input"
                        value={typeof value === "string" ? value : ""}
                        onChange={(e) => setDraft((prev) => ({ ...prev, [decl.id]: e.target.value }))}
                      >
                        {required && <option value="">{t("plugins.params.choose")}</option>}
                        {VIRTUAL_MODEL_OPTIONS.map((o) => (
                          <option key={o.value} value={o.value}>
                            {o.label}
                          </option>
                        ))}
                      </select>
                    ) : decl.type === "enum" ? (
                      <select
                        id={`pg-param-${decl.id}`}
                        className="input"
                        value={typeof value === "string" ? value : ""}
                        onChange={(e) => setDraft((prev) => ({ ...prev, [decl.id]: e.target.value }))}
                      >
                        {required && <option value="">{t("plugins.params.choose")}</option>}
                        {decl.values.map((v) => (
                          <option key={v} value={v}>
                            {v}
                          </option>
                        ))}
                      </select>
                    ) : decl.type === "secret" ? (
                      <div className="pg-secret-row">
                        <input
                          id={`pg-param-${decl.id}`}
                          type="password"
                          autoComplete="off"
                          className="input"
                          value={typeof value === "string" ? value : ""}
                          placeholder={hasSavedSecret ? t("plugins.params.secretKeepHint") : undefined}
                          onChange={(e) => {
                            const next = e.target.value;
                            setDraft((prev) => ({ ...prev, [decl.id]: next }));
                            if (next) setClearedSecrets((prev) => ({ ...prev, [decl.id]: false }));
                          }}
                        />
                        {hasSavedSecret && decl.optional && (
                          <button
                            type="button"
                            className="btn sm"
                            onClick={() => {
                              setDraft((prev) => ({ ...prev, [decl.id]: "" }));
                              setClearedSecrets((prev) => ({ ...prev, [decl.id]: true }));
                            }}
                          >
                            {t("plugins.params.clearSecret")}
                          </button>
                        )}
                      </div>
                    ) : (
                      <>
                        <input
                          id={`pg-param-${decl.id}`}
                          type={decl.type === "number" ? "number" : decl.type === "base_url" ? "url" : "text"}
                          step={decl.type === "number" ? "any" : undefined}
                          className="input"
                          value={typeof value === "string" ? value : ""}
                          placeholder={decl.default !== undefined ? String(decl.default) : undefined}
                          onChange={(e) => setDraft((prev) => ({ ...prev, [decl.id]: e.target.value }))}
                        />
                        {decl.type === "base_url" && (
                          <div className="pg-param-hint">{t("plugins.params.baseUrlHint")}</div>
                        )}
                      </>
                    )}
                  </div>
                );
              })}
            </div>

            {testResult && (
              <div className={testResult.ok ? "pg-alert info" : "pg-alert err"}>
                {testResult.ok
                  ? t("plugins.notifier.testOk", { status: testResult.http_status ?? 200 })
                  : t("plugins.notifier.testFailed", { error: testResult.error ?? `HTTP ${testResult.http_status ?? "?"}` })}
              </div>
            )}

            {isNotifier && deliveries.length > 0 && (
              <div className="pg-deliveries">
                <div className="pg-deliveries-title">{t("plugins.notifier.recentDeliveries")}</div>
                <ul className="pg-deliveries-list">
                  {deliveries.slice(0, 6).map((d, idx) => (
                    <li key={`${d.timestamp_ms}-${idx}`}>
                      <span className="pg-deliv-time mono">{new Date(d.timestamp_ms).toLocaleTimeString()}</span>
                      <span className="pg-deliv-ev">{eventLabel(d.event)}</span>
                      <span className={d.ok ? "pg-badge ok" : "pg-badge err"}>
                        {d.ok ? `OK ${d.http_status ?? ""}`.trim() : (d.error ?? `HTTP ${d.http_status ?? "?"}`)}
                      </span>
                    </li>
                  ))}
                </ul>
              </div>
            )}

            {error && <div className="pg-alert err">{error}</div>}

            <DialogFooter>
              {isNotifier && isDesktop && (
                <button className="btn" type="button" onClick={() => void saveAndTest()} disabled={busy || testing}>
                  {testing && <Spinner />}
                  {t("plugins.notifier.sendTest")}
                </button>
              )}
              <button className="btn" type="button" onClick={onClose} disabled={busy || testing}>
                {t("common.cancel")}
              </button>
              <button className={promptEnable && !plugin.enabled && isDesktop ? "btn" : "btn primary"} type="button" onClick={() => void submit(false)} disabled={busy || testing}>
                {busy && <Spinner />}
                {t("common.save")}
              </button>
              {!plugin.enabled && isDesktop && (
                <button className="btn primary" type="button" onClick={() => void submit(true)} disabled={busy || testing}>
                  {busy && <Spinner />}
                  {t("plugins.params.saveAndEnable")}
                </button>
              )}
            </DialogFooter>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}
