import { useEffect, useState } from "react";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Spinner } from "@/components/Spinner";
import { api } from "@/api/tauri";
import { useT } from "@/i18n";
import { errorText } from "@/lib/errorText";
import { localized, type LocalizedText } from "@/plugins/manifest";
import { runtime } from "@/runtime";
import type { InstalledPlugin, RuleParamDecl } from "@/types";

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

function buildPayload(decls: RuleParamDecl[], draft: Record<string, string | boolean>): { ok: true; data: Record<string, unknown> } | { ok: false; field: string } {
  const data: Record<string, unknown> = {};
  for (const decl of decls) {
    const v = draft[decl.id];
    if (decl.type === "bool") {
      data[decl.id] = Boolean(v);
      continue;
    }
    const text = typeof v === "string" ? v.trim() : "";
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
 * 规则插件的参数设置对话框: 按插件声明的 `param_decls` 动态生成表单, 保存后由 Rust 按类型校验。
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
  const [draft, setDraft] = useState<Record<string, string | boolean>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setDraft(initialDraft(plugin));
    setError(null);
    setBusy(false);
  }, [plugin]);

  const decls = plugin?.param_decls ?? [];

  async function submit(andEnable: boolean) {
    if (!plugin) return;
    const built = buildPayload(decls, draft);
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

  return (
    <Dialog open={!!plugin} onOpenChange={(v) => !v && !busy && onClose()}>
      <DialogContent className="cc-dialog">
        {plugin && (
          <>
            <DialogHeader>
              <DialogTitle>{t("plugins.params.title", { name: pluginName(plugin, locale) })}</DialogTitle>
              <DialogDescription>{t("plugins.params.desc")}</DialogDescription>
            </DialogHeader>

            {promptEnable && <div className="pg-alert warn">{t("plugins.params.requiredBeforeEnable")}</div>}

            <div className="pg-params-form">
              {decls.map((decl) => {
                const label = localized(decl.label, locale) || decl.id;
                const required = decl.default === undefined;
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
                return (
                  <div key={decl.id} className="pg-param-row">
                    <label className="pg-param-label" htmlFor={`pg-param-${decl.id}`}>
                      <span>{label}</span>
                      {required && <span className="pg-param-req">{t("plugins.params.required")}</span>}
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
                    ) : (
                      <input
                        id={`pg-param-${decl.id}`}
                        type={decl.type === "number" ? "number" : "text"}
                        step={decl.type === "number" ? "any" : undefined}
                        className="input"
                        value={typeof value === "string" ? value : ""}
                        placeholder={decl.default !== undefined ? String(decl.default) : undefined}
                        onChange={(e) => setDraft((prev) => ({ ...prev, [decl.id]: e.target.value }))}
                      />
                    )}
                  </div>
                );
              })}
            </div>

            {error && <div className="pg-alert err">{error}</div>}

            <DialogFooter>
              <button className="btn" type="button" onClick={onClose} disabled={busy}>
                {t("common.cancel")}
              </button>
              <button className={promptEnable && !plugin.enabled && isDesktop ? "btn" : "btn primary"} type="button" onClick={() => void submit(false)} disabled={busy}>
                {busy && <Spinner />}
                {t("common.save")}
              </button>
              {!plugin.enabled && isDesktop && (
                <button className="btn primary" type="button" onClick={() => void submit(true)} disabled={busy}>
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
