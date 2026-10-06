// 检查更新页的「新版本内容」: 解析 latest.json.notes (即 GitHub Release 正文) 里的 Markdown。
// 正文由 scripts/release-body.mjs 生成 (`# v<版本>` + 三语段落, `---` 分隔), 但预发布版本可能是手写的,
// 所以这里是宽松解析: 认不出的写法一律当普通文本, 绝不抛错。新版本的说明不在当前 app 里内嵌,
// 用不了 Rust 侧的严格解析器 (src-tauri/src/release_notes/markdown.rs)。
// 输出是结构化数据, 由 React 渲染, 全程不注入 HTML。
import type { NotesInline, NotesLang } from "@/types";

export interface MdListItem {
  text: NotesInline[];
  children: MdListItem[];
}

export type MdBlock =
  | { kind: "heading"; level: number; text: NotesInline[] }
  | { kind: "paragraph"; text: NotesInline[] }
  | { kind: "list"; ordered: boolean; items: MdListItem[] }
  | { kind: "code"; text: string }
  | { kind: "rule" };

/** release-body.mjs 写在每段开头的语言标签 (单独一行的粗体) */
const LANG_LABELS: Record<NotesLang, string> = { zh: "中文", en: "English", ja: "日本語" };

const RULE_RE = /^ {0,3}([-*_])( *\1){2,} *$/;
const HEADING_RE = /^ {0,3}(#{1,6})\s+(.*?)\s*#*\s*$/;
const LIST_RE = /^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$/;
const FENCE_RE = /^ {0,3}(```|~~~)/;

/** 三语正文只取界面语言那一段; 想要的语言缺了按 en → zh 回退; 认不出三语结构就原样返回 */
export function pickNotesLang(body: string, want: NotesLang): string {
  const text = body.replace(/\r\n/g, "\n").trim();
  const chunks = text.split(/\n[ \t]*---[ \t]*\n/);
  const byLang = new Map<NotesLang, string>();
  for (const chunk of chunks) {
    const lines = chunk.trim().split("\n");
    const label = lines[0]?.trim().match(/^\*\*(.+)\*\*$/)?.[1];
    const lang = (Object.keys(LANG_LABELS) as NotesLang[]).find((l) => LANG_LABELS[l] === label);
    if (lang && !byLang.has(lang)) byLang.set(lang, lines.slice(1).join("\n").trim());
  }
  for (const l of [want, "en", "zh"] as const) {
    const hit = byLang.get(l);
    if (hit) return hit;
  }
  return text;
}

export function parseMarkdown(src: string): MdBlock[] {
  const lines = src.replace(/\r\n/g, "\n").split("\n");
  const blocks: MdBlock[] = [];
  let para: string[] = [];
  // 列表栈: 每层记住缩进与所在的 items 数组, 按缩进深浅进出
  let list: { ordered: boolean; items: MdListItem[] } | null = null;
  let stack: { indent: number; items: MdListItem[] }[] = [];

  const flushPara = () => {
    if (para.length) blocks.push({ kind: "paragraph", text: parseInline(para.join(" ")) });
    para = [];
  };
  const closeList = () => {
    if (list) blocks.push({ kind: "list", ...list });
    list = null;
    stack = [];
  };

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (!line.trim()) {
      flushPara();
      continue;
    }
    if (FENCE_RE.test(line)) {
      flushPara();
      closeList();
      const fence = line.trim().slice(0, 3);
      const body: string[] = [];
      for (i++; i < lines.length && !lines[i].trim().startsWith(fence); i++) body.push(lines[i]);
      blocks.push({ kind: "code", text: body.join("\n") });
      continue;
    }
    if (RULE_RE.test(line)) {
      flushPara();
      closeList();
      blocks.push({ kind: "rule" });
      continue;
    }
    const h = line.match(HEADING_RE);
    if (h) {
      flushPara();
      closeList();
      blocks.push({ kind: "heading", level: h[1].length, text: parseInline(h[2]) });
      continue;
    }
    const li = line.match(LIST_RE);
    if (li) {
      flushPara();
      const indent = li[1].replace(/\t/g, "  ").length;
      const ordered = /\d/.test(li[2]);
      if (!list) {
        list = { ordered, items: [] };
        stack = [{ indent, items: list.items }];
      }
      while (stack.length > 1 && indent < stack[stack.length - 1].indent) stack.pop();
      let top = stack[stack.length - 1];
      if (indent > top.indent && top.items.length > 0) {
        const parent = top.items[top.items.length - 1];
        top = { indent, items: parent.children };
        stack.push(top);
      }
      top.items.push({ text: parseInline(li[3]), children: [] });
      continue;
    }
    // 列表项的续行 (缩进了的非列表行) 接到上一项后面; 否则是普通段落
    if (list && /^\s+\S/.test(line)) {
      const items = stack[stack.length - 1].items;
      const last = items[items.length - 1];
      last.text = [...last.text, { kind: "text", text: " " }, ...parseInline(line.trim())];
      continue;
    }
    closeList();
    para.push(line.trim());
  }
  flushPara();
  closeList();
  return blocks;
}

const INLINE_RE = /\*\*(.+?)\*\*|__(.+?)__|`([^`]+)`|\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g;

export function parseInline(src: string): NotesInline[] {
  const out: NotesInline[] = [];
  let last = 0;
  for (const m of src.matchAll(INLINE_RE)) {
    const at = m.index ?? 0;
    if (at > last) out.push({ kind: "text", text: src.slice(last, at) });
    if (m[1] !== undefined || m[2] !== undefined) out.push({ kind: "bold", text: m[1] ?? m[2] });
    else if (m[3] !== undefined) out.push({ kind: "code", text: m[3] });
    else out.push({ kind: "link", text: m[4], url: m[5] });
    last = at + m[0].length;
  }
  if (last < src.length) out.push({ kind: "text", text: src.slice(last) });
  return out;
}
