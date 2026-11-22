/**
 * `theme.json` (themeApi 1) 的校验。只认 tokens.ts 里登记的键, 值过 values.ts 的白名单。
 * 未知的键只算警告: 新版主题装到旧版 app 上只是少几项效果, 不会装不上。零依赖。
 */
import { FONT_EXTS, IMAGE_EXTS, extOf, type Issue } from "../manifest";
import { COLOR_TOKENS, FONT_SLOTS, SHAPE_TOKENS, TEXTURE_SLOTS, THEME_API, THEME_BASES, type ThemeBase, type ValueKind } from "./tokens";
import { isBgPosition, isBgRepeat, isBgSize, isColor, isRadius, isShadow } from "./values";

export interface Texture {
  image: string;
  size?: string;
  repeat?: string;
  position?: string;
}

export interface ThemeMode {
  colors: Record<string, string>;
  textures: Record<string, Texture>;
}

export interface Theme {
  themeApi: 1;
  base: ThemeBase;
  light: ThemeMode;
  dark: ThemeMode | null;
  shape: Record<string, string>;
  fonts: Record<string, string>;
}

const CHECK: Record<ValueKind, (v: string) => boolean> = { color: isColor, radius: isRadius, shadow: isShadow };
const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const MAX_VALUE_LEN = 300;

export function validateTheme(raw: unknown, assets: readonly string[]): { theme: Theme | null; errors: Issue[]; warnings: Issue[] } {
  const errors: Issue[] = [];
  const warnings: Issue[] = [];
  if (!isObj(raw)) return { theme: null, errors: [{ path: "", code: "invalid_type", detail: "expected a JSON object" }], warnings };

  const known = new Set(["themeApi", "base", "light", "dark", "shape", "fonts", "$schema"]);
  for (const k of Object.keys(raw)) if (!known.has(k)) warnings.push({ path: k, code: "unknown_key" });

  if (raw.themeApi !== THEME_API) errors.push({ path: "themeApi", code: raw.themeApi === undefined ? "missing" : "unsupported_version", detail: `this app supports themeApi ${THEME_API}` });
  const base = raw.base ?? "quiet";
  if (typeof base !== "string" || !(THEME_BASES as readonly string[]).includes(base)) {
    errors.push({ path: "base", code: "invalid_value", detail: `one of: ${THEME_BASES.join(", ")}` });
  }

  const tokenMap = (obj: unknown, path: string, table: Record<string, { kind: ValueKind }>): Record<string, string> => {
    const out: Record<string, string> = {};
    if (obj === undefined) return out;
    if (!isObj(obj)) {
      errors.push({ path, code: "invalid_type" });
      return out;
    }
    for (const [k, v] of Object.entries(obj)) {
      const def = table[k];
      if (!def) {
        warnings.push({ path: `${path}.${k}`, code: "unknown_key" });
        continue;
      }
      if (typeof v !== "string" || v.length > MAX_VALUE_LEN || !CHECK[def.kind](v)) {
        errors.push({ path: `${path}.${k}`, code: "invalid_value", detail: `expected ${def.kind}` });
        continue;
      }
      out[k] = v.trim();
    }
    return out;
  };

  const assetRef = (v: unknown, path: string, exts: string[]): string | null => {
    if (typeof v !== "string") {
      errors.push({ path, code: v === undefined ? "missing" : "invalid_type" });
      return null;
    }
    if (!assets.includes(v)) {
      errors.push({ path, code: "not_in_assets", detail: v });
      return null;
    }
    if (!exts.includes(extOf(v))) {
      errors.push({ path, code: "wrong_file_kind", detail: v });
      return null;
    }
    return v;
  };

  const textures = (obj: unknown, path: string): Record<string, Texture> => {
    const out: Record<string, Texture> = {};
    if (obj === undefined) return out;
    if (!isObj(obj)) {
      errors.push({ path, code: "invalid_type" });
      return out;
    }
    for (const [slot, t] of Object.entries(obj)) {
      const p = `${path}.${slot}`;
      if (!(slot in TEXTURE_SLOTS)) {
        warnings.push({ path: p, code: "unknown_key" });
        continue;
      }
      if (!isObj(t)) {
        errors.push({ path: p, code: "invalid_type" });
        continue;
      }
      for (const k of Object.keys(t)) if (!["image", "size", "repeat", "position"].includes(k)) warnings.push({ path: `${p}.${k}`, code: "unknown_key" });
      const image = assetRef(t.image, `${p}.image`, IMAGE_EXTS);
      const opt = (k: "size" | "repeat" | "position", ok: (v: string) => boolean): string | undefined => {
        const v = t[k];
        if (v === undefined) return undefined;
        if (typeof v !== "string" || v.length > MAX_VALUE_LEN || !ok(v)) {
          errors.push({ path: `${p}.${k}`, code: "invalid_value" });
          return undefined;
        }
        return v.trim();
      };
      const tex: Texture | null = image ? { image } : null;
      const size = opt("size", isBgSize);
      const repeat = opt("repeat", isBgRepeat);
      const position = opt("position", isBgPosition);
      if (tex) {
        if (size) tex.size = size;
        if (repeat) tex.repeat = repeat;
        if (position) tex.position = position;
        out[slot] = tex;
      }
    }
    return out;
  };

  const mode = (obj: unknown, path: string): ThemeMode | null => {
    if (obj === undefined) return null;
    if (!isObj(obj)) {
      errors.push({ path, code: "invalid_type" });
      return null;
    }
    for (const k of Object.keys(obj)) if (k !== "colors" && k !== "textures") warnings.push({ path: `${path}.${k}`, code: "unknown_key" });
    return { colors: tokenMap(obj.colors, `${path}.colors`, COLOR_TOKENS), textures: textures(obj.textures, `${path}.textures`) };
  };

  const light = mode(raw.light, "light") ?? { colors: {}, textures: {} };
  const dark = mode(raw.dark, "dark");
  const shape = tokenMap(raw.shape, "shape", SHAPE_TOKENS);

  const fonts: Record<string, string> = {};
  if (raw.fonts !== undefined) {
    if (!isObj(raw.fonts)) errors.push({ path: "fonts", code: "invalid_type" });
    else {
      for (const [slot, v] of Object.entries(raw.fonts)) {
        if (!(slot in FONT_SLOTS)) {
          warnings.push({ path: `fonts.${slot}`, code: "unknown_key" });
          continue;
        }
        const ref = assetRef(v, `fonts.${slot}`, FONT_EXTS);
        if (ref) fonts[slot] = ref;
      }
    }
  }

  if (errors.length > 0) return { theme: null, errors, warnings };
  return { theme: { themeApi: 1, base: base as ThemeBase, light, dark, shape, fonts }, errors, warnings };
}
