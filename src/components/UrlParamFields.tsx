import { useT } from "@/i18n";
import type { ProviderEndpointInfo, ProviderInfo } from "@/types";

/** 该端点用到的参数里, 空的与格式不对的 (pattern 与后端同一份, 后端会再校验一次)。 */
export function urlParamErrors(
  provider: ProviderInfo,
  endpoint: ProviderEndpointInfo,
  values: Record<string, string>,
): Record<string, "empty" | "format"> {
  const out: Record<string, "empty" | "format"> = {};
  for (const id of endpoint.url_params_used) {
    const decl = provider.url_params.find((u) => u.id === id);
    if (!decl) continue;
    const v = (values[id] ?? "").trim();
    if (!v) out[id] = "empty";
    else if (!new RegExp(decl.pattern).test(v)) out[id] = "format";
  }
  return out;
}

export function UrlParamFields({
  provider,
  endpoint,
  values,
  onChange,
  showErrors,
}: {
  provider: ProviderInfo;
  endpoint: ProviderEndpointInfo;
  values: Record<string, string>;
  onChange: (id: string, v: string) => void;
  showErrors: boolean;
}) {
  const { t } = useT();
  if (endpoint.url_params_used.length === 0) return null;
  const errors = showErrors ? urlParamErrors(provider, endpoint, values) : {};
  return (
    <>
      {endpoint.url_params_used.map((id) => {
        const decl = provider.url_params.find((u) => u.id === id);
        if (!decl) return null;
        const err = errors[id];
        return (
          <div key={id} style={{ marginBottom: 20 }}>
            <label className="field-label">{decl.label}</label>
            <input
              className="input mono"
              value={values[id] ?? ""}
              onChange={(e) => onChange(id, e.target.value)}
              placeholder={decl.placeholder}
              aria-invalid={!!err}
            />
            {err && (
              <div className="field-hint" style={{ color: "var(--err)" }}>
                {err === "empty"
                  ? t("subscriptionNew.urlParam.empty", { label: decl.label })
                  : t("subscriptionNew.urlParam.format", { label: decl.label })}
              </div>
            )}
          </div>
        );
      })}
    </>
  );
}
