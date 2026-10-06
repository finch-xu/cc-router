import { useId, type ChangeEvent } from "react";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { useT } from "@/i18n";
import type { ProviderEndpointInfo, ProviderInfo } from "@/types";

/** pattern 编译不了就当作通过 (后端会再校验一次, 不能让前端卡死在一个坏正则上)。 */
function matchesPattern(pattern: string, value: string): boolean {
  try {
    return new RegExp(pattern).test(value);
  } catch {
    return true;
  }
}

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
    else if (!matchesPattern(decl.pattern, v)) out[id] = "format";
  }
  return out;
}

/**
 * 厂商参数输入框。`layout`:
 * - `stacked` (新建页): 标签在上、输入框在下, 与新建页其它字段一致;
 * - `grid` (编辑页): 每个参数一行 120px 标签列 + 输入列, 与编辑页相邻行对齐。
 */
export function UrlParamFields({
  provider,
  endpoint,
  values,
  onChange,
  showErrors,
  layout = "stacked",
}: {
  provider: ProviderInfo;
  endpoint: ProviderEndpointInfo;
  values: Record<string, string>;
  onChange: (id: string, v: string) => void;
  showErrors: boolean;
  layout?: "stacked" | "grid";
}) {
  const { t } = useT();
  const baseId = useId();
  if (endpoint.url_params_used.length === 0) return null;
  const errors = showErrors ? urlParamErrors(provider, endpoint, values) : {};
  return (
    <>
      {endpoint.url_params_used.map((id) => {
        const decl = provider.url_params.find((u) => u.id === id);
        if (!decl) return null;
        const err = errors[id];
        const inputId = `${baseId}-${id}`;
        const errId = `${inputId}-err`;
        const errText = err
          ? err === "empty"
            ? t("subscriptionNew.urlParam.empty", { label: decl.label })
            : t("subscriptionNew.urlParam.format", { label: decl.label })
          : null;
        const inputProps = {
          id: inputId,
          value: values[id] ?? "",
          onChange: (e: ChangeEvent<HTMLInputElement>) => onChange(id, e.target.value),
          placeholder: decl.placeholder,
          "aria-invalid": !!err,
          "aria-describedby": errText ? errId : undefined,
        };
        if (layout === "grid") {
          return (
            <div key={id} className="grid grid-cols-[120px_1fr] gap-3 items-start">
              <Label htmlFor={inputId} className="mt-2">
                {decl.label}
              </Label>
              <div className="space-y-1">
                <Input className="font-mono" {...inputProps} />
                {errText && (
                  <div id={errId} className="text-xs" style={{ color: "var(--err)" }}>
                    {errText}
                  </div>
                )}
              </div>
            </div>
          );
        }
        return (
          <div key={id} style={{ marginBottom: 20 }}>
            <label className="field-label" htmlFor={inputId}>
              {decl.label}
            </label>
            <input className="input mono" {...inputProps} />
            {errText && (
              <div id={errId} className="field-hint" style={{ color: "var(--err)" }}>
                {errText}
              </div>
            )}
          </div>
        );
      })}
    </>
  );
}
