/**
 * 主题 API v1 的公开 token 表 —— 第三方主题能填什么, 全在这一个文件里。
 *
 * 公开名 (插件里写的键) 与内部 CSS 变量刻意分开: 内部变量以后改名 / 拆分只改这里的映射,
 * 已发布的主题不受影响。这张表**只增不删**; 删改公开名必须升 THEME_API 主版本 (见设计文档 §3)。
 *
 * 本文件零依赖, 社区仓库的 CI 用 Node 直接 import (与 scripts/check-*.mjs 同一做法)。
 */

export const THEME_API = 1;

/** 第三方主题允许的底板。用户决定: 第一期只开放「简洁」。 */
export const THEME_BASES = ["quiet"] as const;
export type ThemeBase = (typeof THEME_BASES)[number];

export type ValueKind = "color" | "radius" | "shadow";

export interface TokenDef {
  /** 写到哪些内部变量上 (同一个值) */
  vars: readonly string[];
  kind: ValueKind;
}

/** `light.colors` / `dark.colors` */
export const COLOR_TOKENS: Readonly<Record<string, TokenDef>> = {
  canvas: { vars: ["--bg"], kind: "color" },
  surface: { vars: ["--surface"], kind: "color" },
  /** 弹层; 不写时取 surface */
  popover: { vars: ["--q-pop"], kind: "color" },
  sidebar: { vars: ["--surface-2"], kind: "color" },
  well: { vars: ["--surface-3"], kind: "color" },
  selected: { vars: ["--q-sel"], kind: "color" },
  line: { vars: ["--line"], kind: "color" },
  "line-strong": { vars: ["--line-2"], kind: "color" },
  hairline: { vars: ["--q-hair"], kind: "color" },
  edge: { vars: ["--q-edge"], kind: "color" },
  hover: { vars: ["--q-hover"], kind: "color" },
  text: { vars: ["--ink"], kind: "color" },
  "text-2": { vars: ["--ink-2"], kind: "color" },
  "text-3": { vars: ["--ink-3"], kind: "color" },
  "text-4": { vars: ["--ink-4"], kind: "color" },
  "text-5": { vars: ["--ink-5"], kind: "color" },
  accent: { vars: ["--accent"], kind: "color" },
  "accent-strong": { vars: ["--accent-ink"], kind: "color" },
  "accent-soft": { vars: ["--accent-bg"], kind: "color" },
  "accent-line": { vars: ["--accent-line"], kind: "color" },
  "on-accent": { vars: ["--on-accent"], kind: "color" },
  flow: { vars: ["--flow"], kind: "color" },
  ok: { vars: ["--ok"], kind: "color" },
  "ok-soft": { vars: ["--ok-bg"], kind: "color" },
  warn: { vars: ["--warn"], kind: "color" },
  "warn-soft": { vars: ["--q-warn-bg"], kind: "color" },
  err: { vars: ["--err"], kind: "color" },
  "err-soft": { vars: ["--err-bg"], kind: "color" },
};

/** `shape` (两种模式共用) */
export const SHAPE_TOKENS: Readonly<Record<string, TokenDef>> = {
  "radius-card": { vars: ["--q-r-card"], kind: "radius" },
  "radius-control": { vars: ["--q-r-ctl"], kind: "radius" },
  "shadow-card": { vars: ["--q-sh-card"], kind: "shadow" },
  "shadow-hero": { vars: ["--q-sh-hero"], kind: "shadow" },
  "shadow-control": { vars: ["--q-sh-pill"], kind: "shadow" },
  "shadow-popover": { vars: ["--shadow-pop"], kind: "shadow" },
};

/**
 * `light.textures` / `dark.textures` 的位置 → 内部变量前缀 (--q-tx-<x>, -size, -repeat, -pos) + 作用的元素。
 * 规则由生成器按需输出 (只有主题真的给了纹理才有), 内置主题不受影响; 选择器是内部实现, 可随版本调整。
 */
export const TEXTURE_SLOTS: Readonly<Record<string, { var: string; selector: string }>> = {
  canvas: { var: "--q-tx-canvas", selector: ".main" },
  sidebar: { var: "--q-tx-sidebar", selector: ".sidebar" },
  card: { var: "--q-tx-card", selector: ".card, .slot-card, .stats-section" },
};

/** `fonts` 槽 → 内部变量 + 回退栈 (与 styles.css / plain.css 的同名变量一致) */
export const FONT_SLOTS: Readonly<Record<string, { var: string; fallback: string }>> = {
  body: {
    var: "--font-sans",
    fallback: '-apple-system, BlinkMacSystemFont, "Segoe UI Variable Text", "Segoe UI", Inter, Roboto, Arial, var(--cjk-sans), system-ui, sans-serif',
  },
  display: {
    var: "--font-display",
    fallback: '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Arial, var(--cjk-sans), system-ui, sans-serif',
  },
  mono: {
    var: "--font-mono",
    fallback: 'ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, "DejaVu Sans Mono", var(--cjk-mono), monospace',
  },
  hand: {
    var: "--font-hand",
    fallback: "var(--cjk-hand), cursive",
  },
};

/** 由颜色 token 自动推导的 shadcn HSL 变量 (ui/* 组件读它们)。作者不用写。 */
export const DERIVED_HSL: Readonly<Record<string, string>> = {
  "--background": "canvas",
  "--foreground": "text",
  "--card": "surface",
  "--popover": "popover",
  "--primary": "accent",
  "--ring": "accent",
  "--primary-foreground": "on-accent",
  "--secondary": "well",
  "--muted": "well",
  "--muted-foreground": "text-3",
  "--accent-foreground": "accent-strong",
  "--border": "line-strong",
  "--input": "line-strong",
};
