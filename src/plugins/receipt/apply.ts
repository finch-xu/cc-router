/**
 * 在浏览器里加载小票插件: 读 → 校验 (每次都重新校验, 不信任磁盘) → 编译好的模板 + 作用域样式。
 */
import { useQuery } from "@tanstack/react-query";
import { api } from "@/api/tauri";
import { pluginReceiptCss } from "@/components/receipts/PluginSlip";
import { version as APP_VERSION } from "../../../package.json";
import { validateManifest, type Issue, type Manifest } from "../manifest";
import { PluginThemeError } from "../theme/apply";
import { validateReceipt, type CompiledReceipt } from "./validate";

export interface CheckedReceipt {
  manifest: Manifest;
  receipt: CompiledReceipt;
  warnings: Issue[];
}

export function checkReceiptPackage(rawManifest: unknown, entryText: string, texts: Record<string, string>): CheckedReceipt {
  const m = validateManifest(rawManifest, APP_VERSION);
  if (!m.manifest) throw new PluginThemeError("manifest", m.errors);
  if (m.manifest.type !== "receipt") throw new PluginThemeError("not a receipt");
  let json: unknown;
  try {
    json = JSON.parse(entryText);
  } catch {
    throw new PluginThemeError("entry", [{ path: m.manifest.entry, code: "invalid_type", detail: "not valid JSON" }]);
  }
  const r = validateReceipt(json, m.manifest.assets, texts);
  if (!r.receipt) throw new PluginThemeError("receipt", r.errors);
  return { manifest: m.manifest, receipt: r.receipt, warnings: [...m.warnings, ...r.warnings] };
}

export interface LoadedReceipt {
  manifest: Manifest;
  receipt: CompiledReceipt;
  css: string;
}

export async function loadPluginReceipt(pluginId: string): Promise<LoadedReceipt> {
  const content = await api.loadPlugin(pluginId);
  const { manifest, receipt } = checkReceiptPackage(content.manifest, content.entry, content.texts);
  if (manifest.id !== pluginId) throw new PluginThemeError("id mismatch");
  return { manifest, receipt, css: pluginReceiptCss(pluginId, receipt, (p) => content.assets[p] ?? null) };
}

export function usePluginReceipt(pluginId: string | null) {
  return useQuery({
    queryKey: ["plugins", "receipt", pluginId],
    queryFn: () => loadPluginReceipt(pluginId as string),
    enabled: !!pluginId,
    staleTime: Infinity,
    retry: false,
  });
}
