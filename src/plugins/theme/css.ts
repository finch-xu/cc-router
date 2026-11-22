/**
 * 把校验过的主题翻译成 CSS。只能吃 validateTheme 的输出 —— 值的安全性靠那边的白名单,
 * 这里唯一自己拼进去的外部内容是资源 data URL, 拼之前再按严格格式检查一次。零依赖。
 *
 * 作用域: <html data-theme="quiet ccr-<id>">
 *   - 浅色块写成 `:root:where(...):not(.dark)` (权重 0,2,0): 只在浅色生效, 也就不要求作者
 *     把每个浅色 token 在深色里再写一遍 (内置的 sparrow.css 没有这个便利, 见其文件头)。
 *   - 深色块 `.dark:where(...)` 与 quiet 的深色块同权重, 注入的 <style> 排在最后, 靠顺序胜出。
 *   - 形状 / 字体两种模式共用, `:root:where(...)`。
 *   - 纹理规则 (background-image 等) 只在主题给了纹理时输出; 注入的 <style> 不在任何 @layer 里,
 *     天然压过 quiet.css 的层内规则。
 */
import { COLOR_TOKENS, DERIVED_HSL, FONT_SLOTS, SHAPE_TOKENS, TEXTURE_SLOTS } from "./tokens";
import type { Theme, ThemeMode } from "./validate";
import { toHslTriplet } from "./values";

export const PLUGIN_THEME_PREFIX = "plugin:";
const ID_RE = /^[a-z0-9][a-z0-9-]{1,39}$/;
const DATA_URL_RE = /^data:[a-z0-9.+/-]+;base64,[A-Za-z0-9+/]+=*$/;

/** <html data-theme> 里插件主题自己的那个词 */
export function pluginThemeAttr(pluginId: string): string {
  return `ccr-${pluginId}`;
}

export interface ThemeCss {
  /** 全部: token + 质感 + 字体 */
  full: string;
  /** 只有 token (不含图片 / 字体, 体积小), 缓存给首帧用 */
  tokensOnly: string;
}

function decls(vars: [string, string][]): string {
  return vars.map(([k, v]) => `  ${k}: ${v};`).join("\n");
}

function colorVars(mode: ThemeMode): [string, string][] {
  const out: [string, string][] = [];
  // 回退: 弹层不写就跟卡片一样
  const colors = { ...mode.colors };
  if (colors.surface && !colors.popover) colors.popover = colors.surface;
  for (const [key, value] of Object.entries(colors)) {
    for (const v of COLOR_TOKENS[key].vars) out.push([v, value]);
  }
  for (const [hslVar, key] of Object.entries(DERIVED_HSL)) {
    const v = colors[key];
    const hsl = v ? toHslTriplet(v) : null;
    if (hsl) out.push([hslVar, hsl]);
  }
  return out;
}

function textureVars(mode: ThemeMode, url: (path: string) => string | null): [string, string][] {
  const out: [string, string][] = [];
  for (const [slot, t] of Object.entries(mode.textures)) {
    const u = url(t.image);
    if (!u) continue;
    const pre = TEXTURE_SLOTS[slot].var;
    out.push([pre, `url("${u}")`]);
    if (t.size) out.push([`${pre}-size`, t.size]);
    if (t.repeat) out.push([`${pre}-repeat`, t.repeat]);
    if (t.position) out.push([`${pre}-pos`, t.position]);
  }
  return out;
}

const FONT_FORMAT: Record<string, string> = { woff2: "woff2", woff: "woff", ttf: "truetype", otf: "opentype" };

/**
 * @param assetUrl 资源路径 → data URL (由 Rust `load_plugin` 给出); 返回 null 的资源被跳过
 */
export function themeCss(pluginId: string, theme: Theme, assetUrl: (path: string) => string | null): ThemeCss {
  if (!ID_RE.test(pluginId)) throw new Error(`bad plugin id: ${pluginId}`);
  const attr = pluginThemeAttr(pluginId);
  const safeUrl = (p: string) => {
    const u = assetUrl(p);
    return u && DATA_URL_RE.test(u) ? u : null;
  };

  const rootSel = `:root:where([data-theme~="${attr}"])`;
  const lightSel = `${rootSel}:not(.dark)`;
  const darkSel = `.dark:where([data-theme~="${attr}"])`;
  const block = (sel: string, vars: [string, string][]) => (vars.length ? `${sel} {\n${decls(vars)}\n}\n` : "");

  const shapeVars: [string, string][] = [];
  for (const [key, value] of Object.entries(theme.shape)) for (const v of SHAPE_TOKENS[key].vars) shapeVars.push([v, value]);

  const tokens =
    block(rootSel, shapeVars) +
    block(lightSel, colorVars(theme.light)) +
    (theme.dark ? block(darkSel, colorVars(theme.dark)) : "");

  let fontFaces = "";
  const fontVars: [string, string][] = [];
  for (const [slot, path] of Object.entries(theme.fonts)) {
    const u = safeUrl(path);
    if (!u) continue;
    const family = `ccr-${pluginId}-${slot}`;
    const ext = path.slice(path.lastIndexOf(".") + 1).toLowerCase();
    fontFaces += `@font-face {\n  font-family: "${family}";\n  src: url("${u}") format("${FONT_FORMAT[ext] ?? "woff2"}");\n  font-weight: 100 900;\n  font-display: swap;\n}\n`;
    fontVars.push([FONT_SLOTS[slot].var, `"${family}", ${FONT_SLOTS[slot].fallback}`]);
  }

  // 用到的纹理位置各出一条规则; 某个模式没给图时变量未定义, background-image 落回 none
  const slots = new Set([...Object.keys(theme.light.textures), ...Object.keys(theme.dark?.textures ?? {})]);
  let textureRules = "";
  for (const slot of slots) {
    const { var: v, selector } = TEXTURE_SLOTS[slot];
    textureRules +=
      `:where([data-theme~="${attr}"]) :is(${selector}) {\n` +
      `  background-image: var(${v});\n  background-size: var(${v}-size, auto);\n` +
      `  background-repeat: var(${v}-repeat, repeat);\n  background-position: var(${v}-pos, 0 0);\n}\n`;
  }

  const assets =
    block(lightSel, textureVars(theme.light, safeUrl)) +
    (theme.dark ? block(darkSel, textureVars(theme.dark, safeUrl)) : "") +
    textureRules +
    fontFaces +
    block(rootSel, fontVars);

  return { full: tokens + assets, tokensOnly: tokens };
}
