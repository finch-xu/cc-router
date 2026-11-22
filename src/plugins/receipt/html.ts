/**
 * 小票模板的 HTML 白名单校验 (零依赖, 社区 CI 共用)。不做「清洗后接受」: 有任何不合规就整份拒绝。
 * 模板里的 {{…}} 先被换成标记字符; 标记出现在标签里 (标签名 / 属性) 即报错 —— {{ }} 只能出现在文本里。
 */
import type { Issue } from "../manifest";
import { lineOf, mustacheTags } from "./template";

export const ALLOWED_TAGS = new Set([
  "div", "span", "p", "b", "strong", "i", "em", "small", "s", "u", "br", "hr",
  "h1", "h2", "h3", "h4", "h5", "h6", "header", "footer", "section",
  "table", "thead", "tbody", "tfoot", "tr", "td", "th", "ul", "ol", "li", "pre", "code",
  "ccr-qr", "ccr-barcode", "ccr-logo",
]);
const VOID_TAGS = new Set(["br", "hr"]);
/** app 提供的元素 → 允许的数字属性与范围 */
export const CCR_ELEMENTS: Record<string, Record<string, [number, number]>> = {
  "ccr-qr": { size: [40, 200] },
  "ccr-barcode": { height: [20, 120] },
  "ccr-logo": { size: [16, 128] },
};
const MARK = "\u0001";

export function validateTemplateHtml(src: string, file = "template"): Issue[] {
  const errors: Issue[] = [];
  // {{…}} → 等长标记, 偏移量不变 (报行号用)
  let masked = src;
  for (const [s, e] of mustacheTags(src)) masked = masked.slice(0, s) + MARK.repeat(e - s) + masked.slice(e);
  const err = (offset: number, detail: string) => errors.push({ path: `${file}:${lineOf(src, offset)}`, code: "invalid_value", detail });

  const stack: { name: string; at: number }[] = [];
  let i = 0;
  while (i < masked.length && errors.length < 20) {
    const lt = masked.indexOf("<", i);
    if (lt < 0) break;
    if (masked.startsWith("<!--", lt)) {
      const end = masked.indexOf("-->", lt + 4);
      if (end < 0) {
        err(lt, "unclosed comment");
        break;
      }
      i = end + 3;
      continue;
    }
    const gt = masked.indexOf(">", lt);
    if (gt < 0) {
      err(lt, "unclosed tag");
      break;
    }
    const inner = masked.slice(lt + 1, gt);
    i = gt + 1;
    if (inner.includes(MARK)) {
      err(lt, "{{ }} can only be used in text, not inside a tag");
      continue;
    }
    if (inner.includes("<")) {
      err(lt, "stray '<'");
      continue;
    }
    const close = /^\/([a-z][a-z0-9-]*)\s*$/.exec(inner);
    if (close) {
      const name = close[1];
      const top = stack.pop();
      if (!top || top.name !== name) {
        err(lt, `unexpected </${name}>`);
        if (top) stack.push(top);
      }
      continue;
    }
    const open = /^([a-z][a-z0-9-]*)((?:\s+[^\s=/>]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s"'=<>`]+))?)*)\s*(\/?)$/.exec(inner);
    if (!open) {
      err(lt, "malformed tag");
      continue;
    }
    const [, name, attrText, selfClose] = open;
    if (!ALLOWED_TAGS.has(name)) {
      err(lt, `tag <${name}> is not allowed`);
      continue;
    }
    for (const a of attrText.matchAll(/([^\s=/>]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+)))?/g)) {
      const attr = a[1].toLowerCase();
      const value = a[2] ?? a[3] ?? a[4] ?? "";
      const ccr = CCR_ELEMENTS[name];
      if (ccr) {
        const range = ccr[attr];
        const n = Number(value);
        if (!range || !/^\d+$/.test(value) || n < range[0] || n > range[1]) err(lt, `<${name}> ${attr}="${value}" not allowed`);
      } else if (attr === "class") {
        if (!/^[A-Za-z0-9_\-\s]*$/.test(value)) err(lt, `class="${value}" may only contain letters, digits, - and _`);
      } else if (attr === "colspan" || attr === "rowspan") {
        if (!/^\d{1,2}$/.test(value)) err(lt, `${attr} must be a small number`);
      } else {
        err(lt, `attribute ${attr} is not allowed`);
      }
    }
    if (!VOID_TAGS.has(name) && !selfClose) stack.push({ name, at: lt });
    else if (selfClose && !VOID_TAGS.has(name) && !CCR_ELEMENTS[name]) err(lt, `<${name}/> must be closed with </${name}>`);
  }
  for (const s of stack.slice(0, 5)) err(s.at, `<${s.name}> is never closed`);
  // 文本里不能有 '<' 以外的问题; 但 '>' 单独出现是合法文本
  return errors;
}
