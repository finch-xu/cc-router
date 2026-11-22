/**
 * 主题 token 值的语法白名单。只认少数几种写法, 其余一律拒绝 ——
 * 值会被原样写进 CSS, 所以这里就是防注入的那道门 (分号 / 花括号 / url( / var( / 引号都进不来)。
 * 零依赖, 社区 CI 直接 import。
 */

const NUM = String.raw`-?(?:\d+\.?\d*|\.\d+)`;
const LEN = String.raw`(?:0|${NUM}(?:px|rem|em|%))`;
const LEN_RE = new RegExp(`^${LEN}$`);
const FORBIDDEN = /[;{}\\"'<>]|url\(|var\(|expression|@/i;

const HEX_RE = /^#(?:[0-9a-f]{3}|[0-9a-f]{4}|[0-9a-f]{6}|[0-9a-f]{8})$/i;
const FUNC_RE = /^(rgba?|hsla?)\(\s*([0-9.%\s,/+-]+)\)$/i;

export function isColor(v: string): boolean {
  if (FORBIDDEN.test(v)) return false;
  const s = v.trim();
  if (s === "transparent") return true;
  if (HEX_RE.test(s)) return true;
  const m = FUNC_RE.exec(s);
  if (!m) return false;
  const parts = m[2].split(/[\s,/]+/).filter(Boolean);
  if (parts.length < 3 || parts.length > 4) return false;
  return parts.every((p) => new RegExp(`^${NUM}(?:%|deg)?$`).test(p));
}

export function isLength(v: string): boolean {
  return !FORBIDDEN.test(v) && LEN_RE.test(v.trim());
}

/** 1–4 个长度, 可选 `/` 后再 1–4 个 (椭圆角) */
export function isRadius(v: string): boolean {
  if (FORBIDDEN.test(v)) return false;
  const halves = v.trim().split("/");
  if (halves.length > 2) return false;
  return halves.every((h) => {
    const parts = h.trim().split(/\s+/).filter(Boolean);
    return parts.length >= 1 && parts.length <= 4 && parts.every((p) => LEN_RE.test(p));
  });
}

/** `none` 或逗号分隔的若干层: 可选 inset + 2–4 个长度 + 可选颜色 (颜色可在前或后) */
export function isShadow(v: string): boolean {
  if (FORBIDDEN.test(v)) return false;
  const s = v.trim();
  if (s === "none") return true;
  // 按顶层逗号切层 (颜色函数里的逗号不算)
  const layers: string[] = [];
  let depth = 0;
  let cur = "";
  for (const ch of s) {
    if (ch === "(") depth++;
    if (ch === ")") depth--;
    if (depth < 0) return false;
    if (ch === "," && depth === 0) {
      layers.push(cur);
      cur = "";
    } else cur += ch;
  }
  if (depth !== 0) return false;
  layers.push(cur);
  if (layers.length > 8) return false;
  return layers.every(isShadowLayer);
}

function isShadowLayer(layer: string): boolean {
  // 把颜色函数当成一个整体 token
  const tokens = layer.trim().match(/(?:rgba?|hsla?)\([^)]*\)|\S+/gi) ?? [];
  let inset = 0;
  let lengths = 0;
  let colors = 0;
  for (const t of tokens) {
    if (t === "inset") inset++;
    else if (LEN_RE.test(t)) lengths++;
    else if (isColor(t)) colors++;
    else return false;
  }
  return inset <= 1 && colors <= 1 && lengths >= 2 && lengths <= 4;
}

const SIZE_WORDS = new Set(["auto", "cover", "contain"]);
const REPEAT_WORDS = new Set(["repeat", "no-repeat", "repeat-x", "repeat-y", "round", "space"]);
const POS_WORDS = new Set(["top", "bottom", "left", "right", "center"]);

export function isBgSize(v: string): boolean {
  if (FORBIDDEN.test(v)) return false;
  const parts = v.trim().split(/\s+/);
  if (parts.length === 1 && SIZE_WORDS.has(parts[0])) return true;
  return parts.length >= 1 && parts.length <= 2 && parts.every((p) => p === "auto" || LEN_RE.test(p));
}

export function isBgRepeat(v: string): boolean {
  return REPEAT_WORDS.has(v.trim());
}

export function isBgPosition(v: string): boolean {
  if (FORBIDDEN.test(v)) return false;
  const parts = v.trim().split(/\s+/);
  return parts.length >= 1 && parts.length <= 2 && parts.every((p) => POS_WORDS.has(p) || LEN_RE.test(p));
}

/** hex / rgb() → shadcn 用的 "H S% L%"; 其他写法 (hsl / transparent / 带透明度) 返回 null, 调用方跳过推导 */
export function toHslTriplet(v: string): string | null {
  const s = v.trim();
  let r: number;
  let g: number;
  let b: number;
  const hex = HEX_RE.exec(s);
  if (hex) {
    let h = s.slice(1);
    if (h.length === 3 || h.length === 4) h = [...h].map((c) => c + c).join("");
    if (h.length === 8 && h.slice(6) !== "ff") return null;
    r = parseInt(h.slice(0, 2), 16);
    g = parseInt(h.slice(2, 4), 16);
    b = parseInt(h.slice(4, 6), 16);
  } else {
    const m = FUNC_RE.exec(s);
    if (!m || !m[1].toLowerCase().startsWith("rgb")) return null;
    const parts = m[2].split(/[\s,/]+/).filter(Boolean);
    if (parts.length === 4 && Number.parseFloat(parts[3]) !== 1 && parts[3] !== "100%") return null;
    const ch = (p: string) => (p.endsWith("%") ? (Number.parseFloat(p) / 100) * 255 : Number.parseFloat(p));
    [r, g, b] = parts.slice(0, 3).map(ch);
  }
  if (![r, g, b].every((x) => Number.isFinite(x) && x >= 0 && x <= 255)) return null;
  const [R, G, B] = [r / 255, g / 255, b / 255];
  const max = Math.max(R, G, B);
  const min = Math.min(R, G, B);
  const l = (max + min) / 2;
  let hDeg = 0;
  let sat = 0;
  if (max !== min) {
    const d = max - min;
    sat = l > 0.5 ? d / (2 - max - min) : d / (max + min);
    if (max === R) hDeg = (G - B) / d + (G < B ? 6 : 0);
    else if (max === G) hDeg = (B - R) / d + 2;
    else hDeg = (R - G) / d + 4;
    hDeg *= 60;
  }
  return `${Math.round(hDeg)} ${Math.round(sat * 100)}% ${Math.round(l * 100)}%`;
}
