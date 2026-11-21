#!/usr/bin/env node
// i18n 一致性检查 (零依赖, Node 22.18+ 原生 type stripping 直接 import src 里的纯 TS 模块)。
// 被 import 的模块只能有 type-only import, 或 import 同样满足这一条的模块。
//   1. 系统语言探测: src/i18n/locale-cases.json (托盘 / TUI 的 Rust 测试读同一份)
//   2. 繁中简体残留: zh-Hant.json 的值与 release-notes/*/zh-Hant.md
//   3. zh-Hant.json 与 zh.json 键集合一致
//   4. 发版说明的语言回退 (Task 4 追加)
import { execFileSync } from "node:child_process";
import { registerHooks } from "node:module";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";

const ROOT = fileURLToPath(new URL("../", import.meta.url));
const SRC = `${ROOT}src/`;

// `@/x` → src/x(.ts); `.ts` 文件里的无扩展名相对路径补 `.ts`。只为让检查能 import 纯 TS 模块。
const isFile = (p) => existsSync(p) && statSync(p).isFile();
registerHooks({
  resolve(specifier, context, nextResolve) {
    const fromTs = context.parentURL?.endsWith(".ts");
    let base = null;
    if (specifier.startsWith("@/")) base = SRC + specifier.slice(2);
    else if (fromTs && specifier.startsWith(".") && !/\.[cm]?[jt]sx?$|\.json$/.test(specifier)) {
      base = fileURLToPath(new URL(specifier, context.parentURL));
    }
    if (base) {
      for (const cand of [base, `${base}.ts`, `${base}/index.ts`]) {
        if (isFile(cand)) return { url: pathToFileURL(cand).href, shortCircuit: true };
      }
    }
    return nextResolve(specifier, context);
  },
});

const failures = [];
const check = (name, ok, detail = "") => {
  if (!ok) failures.push(`${name}${detail ? `: ${detail}` : ""}`);
};
const readJson = (rel) => JSON.parse(readFileSync(ROOT + rel, "utf8"));

// ---- 1. 系统语言探测 ----
const { localeFromTag, resolvePref } = await import("../src/i18n/localeTag.ts");
for (const { tag, expect } of readJson("src/i18n/locale-cases.json")) {
  const got = localeFromTag(tag);
  check(`localeFromTag(${JSON.stringify(tag)})`, got === expect, `期望 ${expect}, 实际 ${got}`);
}
for (const [pref, sys, expect] of [
  ["zh-Hant", "en-US", "zh-Hant"],
  ["zh", "zh-TW", "zh"],
  ["system", "zh-HK", "zh-Hant"],
  [undefined, "ja-JP", "ja"],
  ["", "zh-TW", "zh-Hant"],
  ["fr", "zh-TW", "zh-Hant"], // 未知值跟随系统, 不白屏
  ["fr", "", "en"],
]) {
  const got = resolvePref(pref, sys);
  check(`resolvePref(${JSON.stringify(pref)}, ${JSON.stringify(sys)})`, got === expect, `期望 ${expect}, 实际 ${got}`);
}

// ---- 2. 简体残留 ----
const SIMPLIFIED = new Set(
  readFileSync(`${SRC}i18n/simplified-only.txt`, "utf8")
    .split("\n")
    .filter((l) => !l.startsWith("#"))
    .flatMap((l) => [...l])
    .filter((c) => c.trim()),
);
const residue = (text) => [...new Set([...text].filter((c) => SIMPLIFIED.has(c)))];
// 字表健全性: 多义字 / 繁体正字不许在表里, 典型简体字必须在表里
check("字表误收简繁共用字", residue("后台里只面干余系才皇后台灣這裡月之暗面").length === 0, residue("后台里只面干余系才皇后台灣這裡月之暗面").join(""));
check("字表漏收典型简体字", residue("这们说对时发开关设务网络软").length === 13, `只认出 ${residue("这们说对时发开关设务网络软").join("")}`);

const zhHantPath = `${SRC}i18n/locales/zh-Hant.json`;
if (existsSync(zhHantPath)) {
  const zhHant = JSON.parse(readFileSync(zhHantPath, "utf8"));
  for (const [key, value] of Object.entries(zhHant)) {
    const r = residue(value);
    check(`zh-Hant.json ${key}`, r.length === 0, `含简体字 ${r.join("")}`);
  }
  // ---- 3. 键集合 ----
  const zh = readJson("src/i18n/locales/zh.json");
  const missing = Object.keys(zh).filter((k) => !(k in zhHant));
  const extra = Object.keys(zhHant).filter((k) => !(k in zh));
  check("zh-Hant.json 缺键", missing.length === 0, missing.join(", "));
  check("zh-Hant.json 多余键", extra.length === 0, extra.join(", "));
}
const notesDir = `${ROOT}release-notes/`;
if (existsSync(notesDir)) {
  for (const ver of readdirSync(notesDir)) {
    const p = `${notesDir}${ver}/zh-Hant.md`;
    if (!isFile(p)) continue;
    const r = residue(readFileSync(p, "utf8"));
    check(`release-notes/${ver}/zh-Hant.md`, r.length === 0, `含简体字 ${r.join("")}`);
  }
}

// ---- 4. 发版说明 ----
const { pickNotesLang } = await import("../src/lib/updateNotesMarkdown.ts");
const { resolveLang } = await import("../src/components/release-notes/pickLang.ts");
// 加繁中之前的 Release 正文, 简中段的标签是「中文」
const oldBody = "# v6.4.0\n\n---\n\n**中文**\n\n简中正文\n\n---\n\n**English**\n\nEnglish body";
check("旧正文: 繁中用户回退简中", pickNotesLang(oldBody, "zh-Hant") === "简中正文", pickNotesLang(oldBody, "zh-Hant"));
check("旧正文: 简中用户", pickNotesLang(oldBody, "zh") === "简中正文");
const newBody = "# v6.5.0\n\n---\n\n**简体中文**\n\n简中\n\n---\n\n**繁體中文**\n\n繁中\n\n---\n\n**English**\n\nEN";
check("新正文: 繁中", pickNotesLang(newBody, "zh-Hant") === "繁中");
check("新正文: 简中", pickNotesLang(newBody, "zh") === "简中");
check("新正文: 日文回退英文", pickNotesLang(newBody, "ja") === "EN");
// 原型链上的名字不能被当成语言标签
const weird = "**constructor**\n\nx";
check("未知标签原样返回", pickNotesLang(weird, "zh") === weird);
const v = { notes: { zh: {}, en: {} } };
check("内嵌说明: 繁中缺失回退简中", resolveLang("zh-Hant", v) === "zh", resolveLang("zh-Hant", v));
check("内嵌说明: 日文缺失回退英文", resolveLang("ja", v) === "en");

// 旧版 app (6.4 及以前) 的解析器只认「中文 / English / 日本語」三个标签, 简中段改名会让它们的
// 简中用户在更新页落到英文段 —— --plain 输出必须继续给简中段写「中文」。
const releaseBodyScript = readFileSync(`${ROOT}scripts/release-body.mjs`, "utf8");
check("release-body.mjs 简中段标签必须是「中文」(旧版解析器只认它)", releaseBodyScript.includes('["zh.md", "中文"]'));
check("新解析器仍认「中文」", pickNotesLang("**中文**\n\n简中", "zh") === "简中");

// 折叠格式 (GitHub Release 默认展示简体中文, 其余语言折叠在 <details> 中)
const foldedBody = [
  "# v6.8.0 · Test",
  "",
  "简中摘要",
  "",
  "## 新功能",
  "- 条目",
  "",
  "---",
  "",
  "<details>",
  "<summary><strong>繁體中文</strong></summary>",
  "",
  "繁中摘要",
  "",
  "</details>",
  "",
  "<details>",
  "<summary><strong>English</strong></summary>",
  "",
  "EN summary",
  "",
  "</details>",
].join("\n");
check("折叠正文: 简中", pickNotesLang(foldedBody, "zh") === "简中摘要\n\n## 新功能\n- 条目", pickNotesLang(foldedBody, "zh"));
check("折叠正文: 繁中", pickNotesLang(foldedBody, "zh-Hant") === "繁中摘要", pickNotesLang(foldedBody, "zh-Hant"));
check("折叠正文: 英文", pickNotesLang(foldedBody, "en") === "EN summary", pickNotesLang(foldedBody, "en"));
check("折叠正文: 日文回退英文", pickNotesLang(foldedBody, "ja") === "EN summary", pickNotesLang(foldedBody, "ja"));
const nonLangDetails = "前文\n\n<details>\n<summary>Debug</summary>\n\nlog\n\n</details>";
check("非语言折叠块原样返回", pickNotesLang(nonLangDetails, "zh") === nonLangDetails);

// 端到端校验 release-body.mjs 两种输出模式 (取最新一个具备四语的版本目录)
const sampleVer = existsSync(notesDir)
  ? readdirSync(notesDir)
      .filter((v) => ["zh.md", "zh-Hant.md", "en.md", "ja.md"].every((f) => isFile(`${notesDir}${v}/${f}`)))
      .sort()
      .at(-1)
  : null;
if (sampleVer) {
  const ghOut = execFileSync(process.execPath, [`${ROOT}scripts/release-body.mjs`, `v${sampleVer}`], {
    encoding: "utf8",
  });
  const plainOut = execFileSync(process.execPath, [`${ROOT}scripts/release-body.mjs`, "--plain", `v${sampleVer}`], {
    encoding: "utf8",
  });
  const zhSrc = readFileSync(`${notesDir}${sampleVer}/zh.md`, "utf8").replace(/\r\n/g, "\n").trim();
  const zhHantSrc = readFileSync(`${notesDir}${sampleVer}/zh-Hant.md`, "utf8").replace(/\r\n/g, "\n").trim();
  const enSrc = readFileSync(`${notesDir}${sampleVer}/en.md`, "utf8").replace(/\r\n/g, "\n").trim();
  const jaSrc = readFileSync(`${notesDir}${sampleVer}/ja.md`, "utf8").replace(/\r\n/g, "\n").trim();

  check("GitHub 正文不应默认展开 <details open>", !/<details\b[^>]*\bopen\b/i.test(ghOut));
  check("GitHub 正文包含简中且未折叠", ghOut.includes(zhSrc) && !ghOut.includes("<summary><strong>中文</strong></summary>"));
  for (const label of ["繁體中文", "English", "日本語"]) {
    check(
      `GitHub 正文将「${label}」折叠且 </summary> 后有空行`,
      ghOut.includes(`<details>\n<summary><strong>${label}</strong></summary>\n\n`),
    );
  }
  check("GitHub 正文可被 pickNotesLang 提取四语", (
    pickNotesLang(ghOut, "zh") === zhSrc
    && pickNotesLang(ghOut, "zh-Hant") === zhHantSrc
    && pickNotesLang(ghOut, "en") === enSrc
    && pickNotesLang(ghOut, "ja") === jaSrc
  ));
  check("--plain 正文不含 <details>", !plainOut.includes("<details>"));
  check("--plain 正文保留「**中文**」兼容旧版 app", plainOut.includes("\n\n---\n\n**中文**\n\n"));
  check("--plain 正文可被 pickNotesLang 提取四语", (
    pickNotesLang(plainOut, "zh") === zhSrc
    && pickNotesLang(plainOut, "zh-Hant") === zhHantSrc
    && pickNotesLang(plainOut, "en") === enSrc
    && pickNotesLang(plainOut, "ja") === jaSrc
  ));
}

if (failures.length) {
  console.error(`check-i18n: ${failures.length} 项失败\n- ${failures.join("\n- ")}`);
  process.exit(1);
}
console.log("check-i18n: 全部通过");
