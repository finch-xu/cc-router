/**
 * 在浏览器里应用主题插件: 读 → 校验 → 生成 CSS → 注入 <style>。
 * 每次应用都重新校验 (不信任磁盘上的文件), 失败就抛错, 由 useTheme 回退到默认主题。
 *
 * 首帧: token 部分 (不含图片 / 字体) 缓存在 localStorage, index.html 的内联脚本先注入它,
 * 避免启动时先闪一下「简洁」再变色。键名与 index.html 保持一致。
 */
import { api } from "@/api/tauri";
import { version as APP_VERSION } from "../../../package.json";
import { validateManifest, type Issue, type Manifest } from "../manifest";
import { themeCss, type ThemeCss } from "./css";
import { validateTheme, type Theme } from "./validate";

export const PLUGIN_STYLE_ID = "ccr-plugin-theme";
export const PLUGIN_CSS_CACHE_KEY = "cc-router-plugin-theme-css";

export class PluginThemeError extends Error {
  constructor(
    message: string,
    readonly issues: Issue[] = [],
  ) {
    super(message);
  }
}

export interface CheckedTheme {
  manifest: Manifest;
  theme: Theme;
  warnings: Issue[];
}

/** 清单 + 入口文本 → 校验过的主题; 有错误抛 PluginThemeError (带全部问题) */
export function checkThemePackage(rawManifest: unknown, entryText: string): CheckedTheme {
  const m = validateManifest(rawManifest, APP_VERSION);
  if (!m.manifest) throw new PluginThemeError("manifest", m.errors);
  if (m.manifest.type !== "theme") throw new PluginThemeError("not a theme");
  let json: unknown;
  try {
    json = JSON.parse(entryText);
  } catch {
    throw new PluginThemeError("entry", [{ path: m.manifest.entry, code: "invalid_type", detail: "not valid JSON" }]);
  }
  const t = validateTheme(json, m.manifest.assets);
  if (!t.theme) throw new PluginThemeError("theme", t.errors);
  return { manifest: m.manifest, theme: t.theme, warnings: [...m.warnings, ...t.warnings] };
}

/** 读已安装的主题插件并生成 CSS */
export async function loadPluginThemeCss(pluginId: string): Promise<ThemeCss> {
  const content = await api.loadPlugin(pluginId);
  const { manifest, theme } = checkThemePackage(content.manifest, content.entry);
  if (manifest.id !== pluginId) throw new PluginThemeError("id mismatch");
  return themeCss(pluginId, theme, (p) => content.assets[p] ?? null);
}

export function injectPluginCss(css: string) {
  let el = document.getElementById(PLUGIN_STYLE_ID) as HTMLStyleElement | null;
  if (!el) {
    el = document.createElement("style");
    el.id = PLUGIN_STYLE_ID;
  }
  if (el.textContent !== css) el.textContent = css;
  // 每次都挪到 <head> 末尾: 同权重靠顺序胜出 (dev 下 Vite 会在后面追加样式)
  document.head.appendChild(el);
}

export function clearPluginCss() {
  document.getElementById(PLUGIN_STYLE_ID)?.remove();
}

export function cachePluginCss(pluginId: string | null, tokensOnly = "") {
  try {
    if (pluginId) localStorage.setItem(PLUGIN_CSS_CACHE_KEY, JSON.stringify({ id: pluginId, css: tokensOnly }));
    else localStorage.removeItem(PLUGIN_CSS_CACHE_KEY);
  } catch {
    // localStorage 不可用: 只是首帧会闪一下
  }
}
