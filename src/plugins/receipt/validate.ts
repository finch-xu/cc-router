/**
 * 小票插件 `receipt.json` (receiptApi 1) 的整体校验: 入口文件 + 模板 + 样式 + 字体。
 * `texts` 是清单里 html / css 资源的原文。零依赖, 社区 CI 共用。
 */
import { FONT_EXTS, extOf, type Issue } from "../manifest";
import { isColor } from "../theme/values";
import { parseReceiptCss, type CssRule } from "./css";
import { FIELD_PATHS, RECEIPT_API } from "./fields";
import { validateTemplateHtml } from "./html";
import { parseTemplate, usedPaths, type TNode } from "./template";

export interface CompiledReceipt {
  width: number;
  paper: string | null;
  nodes: TNode[];
  rules: CssRule[];
  /** 字体名 → 资源路径 */
  fonts: Record<string, string>;
}

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const FONT_NAME_RE = /^[A-Za-z0-9][A-Za-z0-9 _-]{0,39}$/;

export function validateReceipt(
  raw: unknown,
  assets: readonly string[],
  texts: Readonly<Record<string, string>>,
): { receipt: CompiledReceipt | null; errors: Issue[]; warnings: Issue[] } {
  const errors: Issue[] = [];
  const warnings: Issue[] = [];
  if (!isObj(raw)) return { receipt: null, errors: [{ path: "", code: "invalid_type", detail: "expected a JSON object" }], warnings };
  for (const k of Object.keys(raw)) if (!["receiptApi", "template", "style", "width", "paper", "fonts", "$schema"].includes(k)) warnings.push({ path: k, code: "unknown_key" });
  if (raw.receiptApi !== RECEIPT_API) errors.push({ path: "receiptApi", code: raw.receiptApi === undefined ? "missing" : "unsupported_version", detail: `this app supports receiptApi ${RECEIPT_API}` });

  const textFile = (key: "template" | "style", ext: string, required: boolean): string | null => {
    const p = raw[key];
    if (p === undefined) {
      if (required) errors.push({ path: key, code: "missing" });
      return null;
    }
    if (typeof p !== "string" || !assets.includes(p)) {
      errors.push({ path: key, code: "not_in_assets", detail: String(p) });
      return null;
    }
    if (extOf(p) !== ext) {
      errors.push({ path: key, code: "wrong_file_kind", detail: `.${ext} expected` });
      return null;
    }
    const text = texts[p];
    if (text === undefined) {
      errors.push({ path: key, code: "missing", detail: p });
      return null;
    }
    return text;
  };

  const width = raw.width ?? 360;
  if (typeof width !== "number" || !Number.isInteger(width) || width < 280 || width > 480) errors.push({ path: "width", code: "invalid_value", detail: "integer 280-480" });
  const paper = raw.paper ?? null;
  if (paper !== null && (typeof paper !== "string" || !isColor(paper))) errors.push({ path: "paper", code: "invalid_value", detail: "a color" });

  const fonts: Record<string, string> = {};
  if (raw.fonts !== undefined) {
    if (!isObj(raw.fonts)) errors.push({ path: "fonts", code: "invalid_type" });
    else
      for (const [name, p] of Object.entries(raw.fonts)) {
        if (!FONT_NAME_RE.test(name)) errors.push({ path: `fonts.${name}`, code: "invalid_value", detail: "font name: letters, digits, space, - _" });
        else if (typeof p !== "string" || !assets.includes(p)) errors.push({ path: `fonts.${name}`, code: "not_in_assets", detail: String(p) });
        else if (!FONT_EXTS.includes(extOf(p))) errors.push({ path: `fonts.${name}`, code: "wrong_file_kind" });
        else fonts[name] = p;
      }
  }

  const tplPath = typeof raw.template === "string" ? raw.template : "template";
  const tpl = textFile("template", "html", true);
  let nodes: TNode[] | null = null;
  if (tpl !== null) {
    errors.push(...validateTemplateHtml(tpl, tplPath));
    const parsed = parseTemplate(tpl, tplPath);
    errors.push(...parsed.errors);
    nodes = parsed.nodes;
    if (nodes) {
      // 引用了字段表里没有的字段: 只警告 (新字段装到旧 app 上就是空字符串)
      const known = new Set([...FIELD_PATHS.top, ...FIELD_PATHS.group, ...FIELD_PATHS.line, "@index", "@first", "@last"]);
      for (const u of usedPaths(nodes)) {
        const p = u.path.join(".");
        if (!known.has(p)) warnings.push({ path: tplPath, code: "unknown_key", detail: `{{${p}}}` });
      }
    }
  }
  const stylePath = typeof raw.style === "string" ? raw.style : "style";
  const css = textFile("style", "css", false);
  let rules: CssRule[] = [];
  if (css !== null) {
    const parsed = parseReceiptCss(css, stylePath);
    errors.push(...parsed.errors);
    rules = parsed.rules ?? [];
  }

  if (errors.length || !nodes) return { receipt: null, errors, warnings };
  return { receipt: { width: width as number, paper: paper as string | null, nodes, rules, fonts }, errors, warnings };
}
