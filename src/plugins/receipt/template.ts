/**
 * 小票模板语言 (无逻辑, 零依赖, 社区 CI 共用):
 *   {{path}}                    输出, 一律 HTML 转义 (没有原样输出 HTML 的写法)
 *   {{#each path}}…{{/each}}    循环; 块内先在当前项里找字段, 再往外层找; {{@index}} {{@first}} {{@last}}
 *   {{#if path}}…{{else}}…{{/if}} / {{#unless path}}…{{/unless}}
 * 路径只能是 a.b.c 形式的标识符链。HTML 结构的合法性由 html.ts 另行校验。
 */
import type { Issue } from "../manifest";

export type TNode =
  | { t: "text"; v: string }
  | { t: "var"; path: string[] }
  | { t: "each"; path: string[]; body: TNode[] }
  | { t: "if"; path: string[]; neg: boolean; then: TNode[]; else: TNode[] };

const PATH_RE = /^(?:@(?:index|first|last)|[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*)$/;
const MAX_DEPTH = 8;

/** `{{` … `}}` 之间的标签; 返回 [开始, 结束(不含), 内容] */
export function mustacheTags(src: string): [number, number, string][] {
  const out: [number, number, string][] = [];
  let i = 0;
  while ((i = src.indexOf("{{", i)) >= 0) {
    const end = src.indexOf("}}", i + 2);
    if (end < 0) {
      out.push([i, src.length, src.slice(i + 2)]);
      break;
    }
    out.push([i, end + 2, src.slice(i + 2, end).trim()]);
    i = end + 2;
  }
  return out;
}

export function lineOf(src: string, offset: number): number {
  let n = 1;
  for (let i = 0; i < offset && i < src.length; i++) if (src.charCodeAt(i) === 10) n++;
  return n;
}

export function parseTemplate(src: string, file = "template"): { nodes: TNode[] | null; errors: Issue[] } {
  const errors: Issue[] = [];
  const root: TNode[] = [];
  type Frame = { kind: "each" | "if"; node: TNode; target: TNode[]; line: number };
  const stack: Frame[] = [];
  let cur = root;
  let last = 0;
  const err = (offset: number, detail: string) => errors.push({ path: `${file}:${lineOf(src, offset)}`, code: "invalid_value", detail });
  const path = (s: string, offset: number): string[] | null => {
    if (!PATH_RE.test(s)) {
      err(offset, `bad field path "${s}"`);
      return null;
    }
    return s.split(".");
  };

  for (const [start, end, raw] of mustacheTags(src)) {
    if (start > last) cur.push({ t: "text", v: src.slice(last, start) });
    last = end;
    if (end === src.length && !src.endsWith("}}")) {
      err(start, "unclosed {{");
      break;
    }
    if (raw.startsWith("{") || raw.startsWith("&")) {
      err(start, "raw HTML output is not supported");
      continue;
    }
    const m = /^([#/])(each|if|unless)\b\s*(.*)$/.exec(raw);
    if (m) {
      const [, sigil, kind, arg] = m;
      if (sigil === "#") {
        if (stack.length >= MAX_DEPTH) {
          err(start, `blocks nested deeper than ${MAX_DEPTH}`);
          continue;
        }
        const p = path(arg.trim(), start);
        if (!p) continue;
        const node: TNode =
          kind === "each" ? { t: "each", path: p, body: [] } : { t: "if", path: p, neg: kind === "unless", then: [], else: [] };
        cur.push(node);
        stack.push({ kind: kind === "each" ? "each" : "if", node, target: cur, line: start });
        cur = node.t === "each" ? node.body : (node as Extract<TNode, { t: "if" }>).then;
      } else {
        const top = stack.pop();
        const want = kind === "each" ? "each" : "if";
        if (!top || top.kind !== want || (want === "if" && (top.node as Extract<TNode, { t: "if" }>).neg !== (kind === "unless"))) {
          err(start, `unexpected {{/${kind}}}`);
          if (top) stack.push(top);
          continue;
        }
        cur = top.target;
      }
      continue;
    }
    if (raw === "else") {
      const top = stack[stack.length - 1];
      if (!top || top.kind !== "if") {
        err(start, "{{else}} outside {{#if}}");
        continue;
      }
      cur = (top.node as Extract<TNode, { t: "if" }>).else;
      continue;
    }
    const p = path(raw, start);
    if (p) cur.push({ t: "var", path: p });
  }
  if (last < src.length) cur.push({ t: "text", v: src.slice(last) });
  for (const f of stack) err(f.line, `unclosed {{#${f.kind}}}`);
  return errors.length ? { nodes: null, errors } : { nodes: root, errors };
}

export function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c] as string);
}

type Scope = Record<string, unknown>;

/** 只认自有属性: {{constructor.name}} 之类沿原型链的写法一律取不到值 */
const own = (o: unknown, k: string): boolean => !!o && typeof o === "object" && Object.prototype.hasOwnProperty.call(o, k);

function lookup(scopes: Scope[], path: string[]): unknown {
  for (let i = scopes.length - 1; i >= 0; i--) {
    const s = scopes[i];
    if (own(s, path[0])) {
      let v: unknown = s[path[0]];
      for (const k of path.slice(1)) {
        if (own(v, k)) v = (v as Scope)[k];
        else return undefined;
      }
      return v;
    }
  }
  return undefined;
}

function truthy(v: unknown): boolean {
  if (Array.isArray(v)) return v.length > 0;
  return !!v;
}

function show(v: unknown): string {
  if (v === undefined || v === null || typeof v === "object") return "";
  return escapeHtml(String(v));
}

export function renderTemplate(nodes: TNode[], data: Scope): string {
  const out: string[] = [];
  const walk = (list: TNode[], scopes: Scope[]) => {
    for (const n of list) {
      if (n.t === "text") out.push(n.v);
      else if (n.t === "var") out.push(show(lookup(scopes, n.path)));
      else if (n.t === "if") walk(truthy(lookup(scopes, n.path)) !== n.neg ? n.then : n.else, scopes);
      else {
        const arr = lookup(scopes, n.path);
        if (!Array.isArray(arr)) continue;
        arr.forEach((item, i) => {
          const meta: Scope = { "@index": i, "@first": i === 0, "@last": i === arr.length - 1 };
          walk(n.body, [...scopes, meta, item && typeof item === "object" ? (item as Scope) : {}]);
        });
      }
    }
  };
  walk(nodes, [data]);
  return out.join("");
}

/** 模板里用到的全部字段路径 (给校验器查「字段表里没有」用) */
export function usedPaths(nodes: TNode[]): { path: string[]; inEach: number }[] {
  const out: { path: string[]; inEach: number }[] = [];
  const walk = (list: TNode[], depth: number) => {
    for (const n of list) {
      if (n.t === "var") out.push({ path: n.path, inEach: depth });
      else if (n.t === "if") {
        out.push({ path: n.path, inEach: depth });
        walk(n.then, depth);
        walk(n.else, depth);
      } else if (n.t === "each") {
        out.push({ path: n.path, inEach: depth });
        walk(n.body, depth + 1);
      }
    }
  };
  walk(nodes, 0);
  return out;
}
