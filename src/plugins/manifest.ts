/**
 * 插件清单 `cc-router-plugin.json` 的校验。规则与 Rust `plugins/files.rs` 的路径规则一致,
 * 社区仓库的 CI 也直接 import 本文件。零依赖。
 */

import { safeHttpsUrl } from "./links";

export const MANIFEST_VERSION = 1;
export const MANIFEST_FILE = "cc-router-plugin.json";

/** 本版 app 能加载的插件类型。旧版 app 遇到新类型只报 unsupported_type, 不会尝试加载。 */
export const SUPPORTED_TYPES = ["theme", "receipt"] as const;
export type PluginType = (typeof SUPPORTED_TYPES)[number];

/** 内置主题 / 画风的名字不能当插件 id, 避免设置里出现两个「简洁」 */
export const RESERVED_IDS = ["classic", "sketch", "win2000", "win2k", "quiet", "sparrow", "plain", "cc-router"];

export const MAX_ASSETS = 128;
export const IMAGE_EXTS = ["png", "jpg", "jpeg", "webp", "gif", "svg"];
export const FONT_EXTS = ["woff2", "woff", "ttf", "otf"];
/** 文本资源: 只给小票插件用 (模板 / 样式) */
export const TEXT_EXTS = ["html", "css"];
export const LANGS = ["zh", "zh-Hant", "en", "ja"] as const;
export type Lang = (typeof LANGS)[number];

export type IssueCode =
  | "missing"
  | "invalid_type"
  | "invalid_value"
  | "unknown_key"
  | "unsupported_type"
  | "unsupported_version"
  | "needs_newer_app"
  | "reserved_id"
  | "bad_path"
  | "not_in_assets"
  | "wrong_file_kind"
  | "duplicate"
  | "too_many";

export interface Issue {
  /** 出问题的位置, 如 `assets[2]`、`light.colors.canvas` */
  path: string;
  code: IssueCode;
  /** 给作者看的补充 (英文开发者文字; 界面按 code 翻译主句) */
  detail?: string;
}

export type LocalizedText = string | Partial<Record<Lang, string>>;

export interface Manifest {
  manifest: 1;
  id: string;
  type: PluginType;
  version: string;
  name: LocalizedText;
  description?: LocalizedText;
  author: string;
  license?: string;
  homepage?: string;
  entry: string;
  assets: string[];
  minApp?: string;
}

const ID_RE = /^[a-z0-9][a-z0-9-]{1,39}$/;
const SEMVER_RE = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;
const SEG_RE = /^[A-Za-z0-9._-]+$/;

/** 相对路径: 不以 / 开头、不含 . / .. 段、只含 [A-Za-z0-9._-] 与 / */
export function isSafePath(p: unknown): p is string {
  if (typeof p !== "string" || p.length === 0 || p.length > 200) return false;
  const segs = p.split("/");
  return segs.every((s) => SEG_RE.test(s) && s !== "." && s !== "..");
}

export function extOf(p: string): string {
  const i = p.lastIndexOf(".");
  return i < 0 ? "" : p.slice(i + 1).toLowerCase();
}

/** 比较 a.b.c (忽略预发布后缀); a < b 返回负数 */
export function compareVersions(a: string, b: string): number {
  const pa = a.split("-")[0].split(".").map(Number);
  const pb = b.split("-")[0].split(".").map(Number);
  for (let i = 0; i < 3; i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d !== 0) return d;
  }
  return 0;
}

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

function checkText(v: unknown, path: string, issues: Issue[], required: boolean, max: number): LocalizedText | undefined {
  if (v === undefined) {
    if (required) issues.push({ path, code: "missing" });
    return undefined;
  }
  if (typeof v === "string") {
    if (!v.trim() || v.length > max) issues.push({ path, code: "invalid_value" });
    return v;
  }
  if (!isObj(v)) {
    issues.push({ path, code: "invalid_type", detail: "expected a string or { zh, zh-Hant, en, ja }" });
    return undefined;
  }
  const out: Partial<Record<Lang, string>> = {};
  for (const [k, val] of Object.entries(v)) {
    if (!(LANGS as readonly string[]).includes(k)) {
      issues.push({ path: `${path}.${k}`, code: "unknown_key" });
      continue;
    }
    if (typeof val !== "string" || !val.trim() || val.length > max) {
      issues.push({ path: `${path}.${k}`, code: "invalid_value" });
      continue;
    }
    out[k as Lang] = val;
  }
  if (Object.keys(out).length === 0) issues.push({ path, code: "missing", detail: "at least one language" });
  return out;
}

const KNOWN_KEYS = new Set([
  "manifest", "id", "type", "version", "name", "description", "author", "license", "homepage", "entry", "assets", "minApp",
]);

/**
 * 校验清单。`appVersion` 给了就检查 minApp。
 * 返回的 `manifest` 只在没有错误时非空; `unknown_key` 只算警告 (新版清单字段装到旧 app 上不至于失败)。
 */
export function validateManifest(
  raw: unknown,
  appVersion?: string,
): { manifest: Manifest | null; errors: Issue[]; warnings: Issue[] } {
  const issues: Issue[] = [];
  if (!isObj(raw)) {
    return { manifest: null, errors: [{ path: "", code: "invalid_type", detail: "expected a JSON object" }], warnings: [] };
  }
  for (const k of Object.keys(raw)) if (!KNOWN_KEYS.has(k)) issues.push({ path: k, code: "unknown_key" });

  if (raw.manifest !== MANIFEST_VERSION) {
    issues.push({ path: "manifest", code: raw.manifest === undefined ? "missing" : "unsupported_version" });
  }
  const id = raw.id;
  if (typeof id !== "string" || !ID_RE.test(id)) issues.push({ path: "id", code: id === undefined ? "missing" : "invalid_value", detail: "^[a-z0-9][a-z0-9-]{1,39}$" });
  else if (RESERVED_IDS.includes(id)) issues.push({ path: "id", code: "reserved_id" });

  const type = raw.type;
  if (typeof type !== "string") issues.push({ path: "type", code: "missing" });
  else if (!(SUPPORTED_TYPES as readonly string[]).includes(type)) issues.push({ path: "type", code: "unsupported_type", detail: type });

  const version = raw.version;
  if (typeof version !== "string" || !SEMVER_RE.test(version)) issues.push({ path: "version", code: version === undefined ? "missing" : "invalid_value" });

  const name = checkText(raw.name, "name", issues, true, 60);
  const description = checkText(raw.description, "description", issues, false, 400);

  const author = raw.author;
  if (typeof author !== "string" || !author.trim() || author.length > 80) issues.push({ path: "author", code: author === undefined ? "missing" : "invalid_value" });
  if (raw.license !== undefined && (typeof raw.license !== "string" || raw.license.length > 64)) issues.push({ path: "license", code: "invalid_value" });
  if (raw.homepage !== undefined && safeHttpsUrl(raw.homepage) === undefined) {
    issues.push({ path: "homepage", code: "invalid_value", detail: "https URL, at most 300 characters" });
  }

  const entry = raw.entry;
  if (!isSafePath(entry)) issues.push({ path: "entry", code: entry === undefined ? "missing" : "bad_path" });
  else if (extOf(entry) !== "json") issues.push({ path: "entry", code: "wrong_file_kind", detail: "entry must be a .json file" });

  const assets: string[] = [];
  if (raw.assets !== undefined) {
    if (!Array.isArray(raw.assets)) issues.push({ path: "assets", code: "invalid_type" });
    else {
      if (raw.assets.length > MAX_ASSETS) issues.push({ path: "assets", code: "too_many", detail: `max ${MAX_ASSETS}` });
      raw.assets.forEach((a, i) => {
        if (!isSafePath(a)) issues.push({ path: `assets[${i}]`, code: "bad_path" });
        else if (![...IMAGE_EXTS, ...FONT_EXTS, ...TEXT_EXTS].includes(extOf(a))) issues.push({ path: `assets[${i}]`, code: "wrong_file_kind", detail: a });
        else if (type !== "receipt" && TEXT_EXTS.includes(extOf(a))) issues.push({ path: `assets[${i}]`, code: "wrong_file_kind", detail: `${a}: html / css only in receipt plugins` });
        else if (assets.includes(a) || a === entry) issues.push({ path: `assets[${i}]`, code: "duplicate", detail: a });
        else assets.push(a);
      });
    }
  }

  const minApp = raw.minApp;
  if (minApp !== undefined) {
    if (typeof minApp !== "string" || !SEMVER_RE.test(minApp)) issues.push({ path: "minApp", code: "invalid_value" });
    else if (appVersion && compareVersions(appVersion, minApp) < 0) issues.push({ path: "minApp", code: "needs_newer_app", detail: minApp });
  }

  const errors = issues.filter((i) => i.code !== "unknown_key");
  const warnings = issues.filter((i) => i.code === "unknown_key");
  if (errors.length > 0) return { manifest: null, errors, warnings };
  return {
    manifest: {
      manifest: 1,
      id: id as string,
      type: type as PluginType,
      version: version as string,
      name: name as LocalizedText,
      description,
      author: author as string,
      license: raw.license as string | undefined,
      homepage: raw.homepage as string | undefined,
      entry: entry as string,
      assets,
      minApp: minApp as string | undefined,
    },
    errors,
    warnings,
  };
}

/** 按界面语言取文字: 自身 → 繁中回退简中 → en → zh → 任意一个 */
export function localized(text: LocalizedText | undefined, lang: string): string {
  if (text === undefined) return "";
  if (typeof text === "string") return text;
  const order = [lang, lang === "zh-Hant" ? "zh" : "", "en", "zh", ...LANGS];
  for (const l of order) {
    const v = text[l as Lang];
    if (v) return v;
  }
  return "";
}
