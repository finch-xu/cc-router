/**
 * 界面主题注册表。
 *
 * 一个主题 = 一套配色 + 一种画风, 分别打在 <html> 上:
 * - data-theme = 主题 id, 配色 token 按它切换 (classic.css; 手绘的配色就是 styles.css 本身);
 * - data-art   = 画风, 外形规则 (圆角 / 描边 / 字体) 与画风相关的组件按它切换
 *   (plain.css / win2k.css; 侧栏图标、Logo、实时路由图等组件读 useTheme().art)。
 *   画风可以继承: win2k 建在 plain 的结构上 (同样的实时路由页、像素 Logo、细线布局),
 *   只多一层立体边框, 所以 <html data-art="plain win2k">, CSS 用 [data-art~="plain"] 匹配。
 * 以后加主题: 同一画风下换一套配色只需新增一个 id + 一段配色 CSS;
 * 新画风才需要动组件。
 *
 * 主题也可以继承: 「麻雀」(sparrow) 建在「简洁」(quiet) 的全部外形规则上, 只换配色 token,
 * 所以 <html data-theme="quiet sparrow">, quiet.css 用 [data-theme~="quiet"] 匹配,
 * sparrow.css 只写 token (在 main.tsx 里排在 quiet.css 之后, 同权重靠顺序覆盖)。
 * 这也是将来第三方主题插件的形状: 选一个内置主题做底, 只覆盖 token。
 *
 * index.html 里有一段首帧前的内联脚本同步这两个属性, 增删主题时要一起改。
 */
export type BuiltinThemeId = "classic" | "sketch" | "win2000" | "quiet" | "sparrow";
/** 已安装的主题插件: `plugin:<插件 id>`, 一律建在「简洁」之上 (第一期只开放这一个底板) */
export type PluginThemeId = `plugin:${string}`;
export type ThemeId = BuiltinThemeId | PluginThemeId;
export type ThemeArt = "plain" | "sketch" | "win2k";

export interface ThemeDef {
  id: ThemeId;
  art: ThemeArt;
  /** 主题名的 i18n key */
  labelKey: string;
  /** 继承哪个主题的外形规则 (只换配色); 写进 data-theme 的前面 */
  base?: BuiltinThemeId;
}

/** 顺序即侧栏主题按钮的轮换顺序、设置页里按钮的排列顺序 */
export const THEMES: readonly (ThemeDef & { id: BuiltinThemeId })[] = [
  { id: "classic", art: "plain", labelKey: "theme.name.classic" },
  { id: "sketch", art: "sketch", labelKey: "theme.name.sketch" },
  { id: "win2000", art: "win2k", labelKey: "theme.name.win2000" },
  { id: "quiet", art: "plain", labelKey: "theme.name.quiet" },
  { id: "sparrow", art: "plain", labelKey: "theme.name.sparrow", base: "quiet" },
];

/** 画风 → data-art 的值 (含继承链) */
const ART_ATTR: Record<ThemeArt, string> = {
  plain: "plain",
  sketch: "sketch",
  win2k: "plain win2k",
};

/** 是否建在「素净」结构上 (经典 / Win2000): 这类画风共用改版前的实时路由页、像素 Logo 等组件 */
export function isPlainBased(art: ThemeArt): boolean {
  return art !== "sketch";
}

export const DEFAULT_THEME_ID: ThemeId = "quiet";

/** localStorage key; 与 index.html 内联脚本保持一致 */
export const THEME_ID_KEY = "cc-router-ui-theme";

const PLUGIN_THEME_RE = /^plugin:[a-z0-9][a-z0-9-]{1,39}$/;

export function isPluginThemeId(v: unknown): v is PluginThemeId {
  return typeof v === "string" && PLUGIN_THEME_RE.test(v);
}

/** `plugin:<id>` → `<id>`; 内置主题返回 null */
export function pluginIdOf(id: ThemeId): string | null {
  return isPluginThemeId(id) ? id.slice("plugin:".length) : null;
}

export function isThemeId(v: unknown): v is ThemeId {
  return THEMES.some((t) => t.id === v) || isPluginThemeId(v);
}

export function themeDef(id: ThemeId): ThemeDef {
  if (isPluginThemeId(id)) return { id, art: "plain", labelKey: "theme.name.plugin", base: "quiet" };
  return THEMES.find((t) => t.id === id) ?? THEMES[0];
}

/** 轮换到下一个主题 (侧栏主题按钮) */
export function nextThemeId(id: ThemeId): ThemeId {
  // 侧栏按钮只在内置主题间轮换; 当前是插件主题时回到第一个
  const i = THEMES.findIndex((t) => t.id === id);
  return THEMES[(i + 1) % THEMES.length].id;
}

export function applyThemeAttrs(id: ThemeId) {
  const root = document.documentElement;
  const def = themeDef(id);
  const pid = pluginIdOf(id);
  const own = pid ? `ccr-${pid}` : id;
  root.dataset.theme = def.base ? `${def.base} ${own}` : own;
  root.dataset.art = ART_ATTR[def.art];
}
