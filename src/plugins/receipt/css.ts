/**
 * 小票 CSS 的白名单校验 + 作用域 (零依赖, 社区 CI 共用)。
 * 只接受「选择器 { 声明; … }」: 没有 @ 规则、没有 url()、没有反斜杠转义、没有 position: fixed。
 * 输出时每个选择器都加上作用域前缀, 小票样式碰不到 app 的其他部分。
 */
import type { Issue } from "../manifest";
import { lineOf } from "./template";

const FORBIDDEN_VALUE = /url\s*\(|expression|javascript:|@|\\|<|behavior|-moz-binding|image-set|src\s*\(|attr\s*\(/i;
const SELECTOR_RE = /^[A-Za-z0-9_\-.\s>+~:()*,]+$/;
const GLOBAL_RE = /(^|[\s>+~,(])(html|body|:root)\b|:root/i;
const PROP_RE = /^-?[a-z][a-z-]*$/;
const MAX_CSS = 64 * 1024;

export interface CssRule {
  selectors: string[];
  decls: [string, string][];
}

export function parseReceiptCss(src: string, file = "style"): { rules: CssRule[] | null; errors: Issue[] } {
  const errors: Issue[] = [];
  const err = (offset: number, detail: string) => errors.push({ path: `${file}:${lineOf(src, offset)}`, code: "invalid_value", detail });
  if (src.length > MAX_CSS) return { rules: null, errors: [{ path: file, code: "too_many", detail: "max 64 KB" }] };
  // 去注释, 保留换行 (行号不变)
  const css = src.replace(/\/\*[\s\S]*?\*\//g, (c) => c.replace(/[^\n]/g, " "));
  if (css.includes("/*")) err(css.indexOf("/*"), "unclosed comment");
  const rules: CssRule[] = [];
  let i = 0;
  while (i < css.length && errors.length < 20) {
    const open = css.indexOf("{", i);
    if (open < 0) {
      if (css.slice(i).trim()) err(i, "text outside a rule");
      break;
    }
    const close = css.indexOf("}", open);
    if (close < 0) {
      err(open, "unclosed {");
      break;
    }
    const selText = css.slice(i, open).trim();
    const body = css.slice(open + 1, close);
    const at = i + (css.slice(i, open).length - css.slice(i, open).trimStart().length);
    i = close + 1;
    if (body.includes("{")) {
      err(open, "nested blocks / @ rules are not allowed");
      continue;
    }
    if (!selText || !SELECTOR_RE.test(selText) || selText.includes("@")) {
      err(at, `selector "${selText.slice(0, 40)}" is not allowed`);
      continue;
    }
    if (GLOBAL_RE.test(selText)) {
      err(at, "html / body / :root can't be styled; style your own top-level element instead");
      continue;
    }
    const selectors = selText.split(",").map((s) => s.trim().replace(/\s+/g, " "));
    if (selectors.some((s) => !s)) {
      err(at, "empty selector");
      continue;
    }
    const decls: [string, string][] = [];
    for (const d of body.split(";")) {
      if (!d.trim()) continue;
      const c = d.indexOf(":");
      const prop = (c < 0 ? d : d.slice(0, c)).trim().toLowerCase();
      const value = c < 0 ? "" : d.slice(c + 1).trim();
      if (c < 0 || !PROP_RE.test(prop) || !value) {
        err(open, `bad declaration "${d.trim().slice(0, 40)}"`);
        continue;
      }
      if (FORBIDDEN_VALUE.test(value) || /["']/.test(value.replace(/"[^"\\<\n]*"|'[^'\\<\n]*'/g, ""))) {
        err(open, `${prop}: value is not allowed`);
        continue;
      }
      if (prop === "position" && /fixed/i.test(value)) {
        err(open, "position: fixed is not allowed");
        continue;
      }
      decls.push([prop, value]);
    }
    rules.push({ selectors, decls });
  }
  return errors.length ? { rules: null, errors } : { rules, errors };
}

/** 加作用域前缀后输出。scope 是小票外壳上的类名 */
export function scopedCss(rules: CssRule[], scope: string): string {
  if (!/^[a-z0-9-]+$/.test(scope)) throw new Error(`bad scope ${scope}`);
  return rules
    .map((r) => `${r.selectors.map((s) => `.${scope} ${s}`).join(", ")} {\n${r.decls.map(([p, v]) => `  ${p}: ${v};`).join("\n")}\n}`)
    .join("\n");
}
