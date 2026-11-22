#!/usr/bin/env node
// 插件格式检查 (零依赖, Node 22.18+ 原生 type stripping 直接 import 纯 TS 模块):
//   1. 值的语法白名单 (含注入尝试)
//   2. 清单 cc-router-plugin.json 的规则
//   3. theme.json 校验 (未知键只警告, 资源引用必须登记)
//   4. 主题生成器: examples/plugins/sparrow-sun 生成的 CSS 必须与内置 src/themes/sparrow.css 逐项一致
//      —— 证明公开 token 表能完整表达一个内置主题
//   5. 包哈希测试向量 (Rust 侧读同一份文件)
import { registerHooks } from "node:module";
import { existsSync, readFileSync, statSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";

const ROOT = fileURLToPath(new URL("../", import.meta.url));
const SRC = `${ROOT}src/`;

// 与 check-i18n.mjs 相同的解析钩子: `@/x` → src/x(.ts); `.ts` 里的无扩展名相对路径补 `.ts`
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

const values = await import(`${SRC}plugins/theme/values.ts`);
const { validateManifest, isSafePath, localized, RESERVED_IDS, SUPPORTED_TYPES } = await import(`${SRC}plugins/manifest.ts`);
const { validateTheme } = await import(`${SRC}plugins/theme/validate.ts`);
const { themeCss } = await import(`${SRC}plugins/theme/css.ts`);
const { COLOR_TOKENS } = await import(`${SRC}plugins/theme/tokens.ts`);
const { packageHash } = await import(`${ROOT}scripts/lib/plugin-hash.mjs`);

// ---------- 1. 值 ----------
const good = {
  isColor: ["#fff", "#ffffff", "#ffffff80", "rgb(90 55 20 / 10%)", "rgba(0,0,0,.5)", "hsl(20 74% 42%)", "transparent"],
  isRadius: ["16px", "0", "9px 0 0 9px", "255px 15px 225px 15px / 15px 225px 15px 255px", "50%"],
  isShadow: ["none", "0 0 0 1px rgb(41 37 36 / 8%)", "5px 5px 0 #e3dacc", "inset 1px 1px 0 #fff, inset -1px -1px 0 #808080"],
  isBgSize: ["cover", "320px", "auto 50%"],
  isBgPosition: ["top right", "0 0", "center"],
};
const bad = {
  isColor: ["red", "#ggg", "#fff;", "rgb(0 0 0); } body { color: red", "url(x)", "var(--accent)", "rgb(0 0 0", "expression(alert(1))", "#fff' onload", "rgb(1 2 3 4 5)"],
  isRadius: ["16", "16px;", "1px 2px 3px 4px 5px", "1px / 2px / 3px", "calc(1px + 2px)", "var(--r)"],
  isShadow: ["0 #fff", "0 0 0 1px red", "0 0 0 1px #fff}", "url(x) 0 0", "0 0 0 0 0 #fff", "inset inset 0 0 #fff"],
  isBgSize: ["cover;", "1px 2px 3px"],
  isBgPosition: ["top; x", "middle"],
};
for (const [fn, list] of Object.entries(good)) for (const v of list) check(`${fn} 接受 ${JSON.stringify(v)}`, values[fn](v) === true);
for (const [fn, list] of Object.entries(bad)) for (const v of list) check(`${fn} 拒绝 ${JSON.stringify(v)}`, values[fn](v) === false);
check("toHslTriplet hex", values.toHslTriplet("#b9501c") === "20 74% 42%", values.toHslTriplet("#b9501c"));
check("toHslTriplet rgb", values.toHslTriplet("rgb(255 255 255)") === "0 0% 100%");
check("toHslTriplet 带透明度跳过", values.toHslTriplet("rgb(255 255 255 / 10%)") === null);

// ---------- 2. 清单 ----------
const exManifest = readJson("examples/plugins/sparrow-sun/cc-router-plugin.json");
{
  const r = validateManifest(exManifest, "6.7.0");
  check("范例清单合法", r.manifest !== null && r.errors.length === 0, JSON.stringify(r.errors));
  check("范例清单无警告", r.warnings.length === 0, JSON.stringify(r.warnings));
}
const mErr = (patch, code, app = "6.8.0") => {
  const r = validateManifest({ ...exManifest, ...patch }, app);
  check(`清单 ${JSON.stringify(patch)} → ${code}`, r.manifest === null && r.errors.some((e) => e.code === code), JSON.stringify(r.errors));
};
mErr({ id: "Bad_ID" }, "invalid_value");
mErr({ id: "quiet" }, "reserved_id");
mErr({ type: "system" }, "unsupported_type");
mErr({ manifest: 2 }, "unsupported_version");
mErr({ entry: "../theme.json" }, "bad_path");
mErr({ entry: "theme.css" }, "wrong_file_kind");
mErr({ assets: ["a.exe"] }, "wrong_file_kind");
mErr({ assets: ["a.png", "a.png"] }, "duplicate");
mErr({ minApp: "99.0.0" }, "needs_newer_app");
mErr({ name: {} }, "missing");
mErr({ homepage: "http://insecure.example" }, "invalid_value");
{
  const r = validateManifest({ ...exManifest, futureField: 1 }, "6.8.0");
  check("清单未知键只警告", r.manifest !== null && r.warnings.some((w) => w.path === "futureField"));
}
for (const p of ["a.png", "assets/paper.png", "a/b/c.woff2"]) check(`安全路径 ${p}`, isSafePath(p));
for (const p of ["", "/etc/passwd", "../x", "a/../b", "a//b", "a/./b", "a\\b", "中文.png", "a b.png"]) check(`拒绝路径 ${JSON.stringify(p)}`, !isSafePath(p));
check("localized 回退", localized({ zh: "中", en: "En" }, "ja") === "En" && localized({ zh: "中" }, "zh-Hant") === "中" && localized("x", "en") === "x");

// ---------- 3. theme.json ----------
const exTheme = readJson("examples/plugins/sparrow-sun/theme.json");
const vt = validateTheme(exTheme, exManifest.assets);
check("范例主题合法", vt.theme !== null, JSON.stringify(vt.errors));
check("范例主题无警告", vt.warnings.length === 0, JSON.stringify(vt.warnings));
const tErr = (raw, assets, code, path) => {
  const r = validateTheme(raw, assets);
  check(`主题 → ${code} @ ${path}`, r.theme === null && r.errors.some((e) => e.code === code && e.path === path), JSON.stringify(r.errors));
};
tErr({ themeApi: 1, light: { colors: { canvas: "#fff;} html{background:red" } } }, [], "invalid_value", "light.colors.canvas");
tErr({ themeApi: 1, shape: { "radius-card": "url(x)" } }, [], "invalid_value", "shape.radius-card");
tErr({ themeApi: 2 }, [], "unsupported_version", "themeApi");
tErr({ themeApi: 1, base: "sketch" }, [], "invalid_value", "base");
tErr({ themeApi: 1, fonts: { body: "assets/a.woff2" } }, [], "not_in_assets", "fonts.body");
tErr({ themeApi: 1, fonts: { body: "assets/a.png" } }, ["assets/a.png"], "wrong_file_kind", "fonts.body");
tErr({ themeApi: 1, light: { textures: { canvas: { image: "a.woff2" } } } }, ["a.woff2"], "wrong_file_kind", "light.textures.canvas.image");
tErr({ themeApi: 1, light: { textures: { canvas: { image: "a.png", size: "cover; x" } } } }, ["a.png"], "invalid_value", "light.textures.canvas.size");
{
  const r = validateTheme({ themeApi: 1, light: { colors: { canvas: "#fff", sparkle: "#000" } }, icons: "x/" }, []);
  check("主题未知键只警告", r.theme !== null && r.warnings.length === 2, JSON.stringify(r.warnings));
}

// ---------- 4. 生成器 vs 内置 sparrow.css ----------
/** 取出 CSS 里某个选择器块的变量表 */
function varsOf(css, selector) {
  const i = css.indexOf(`${selector} {`);
  if (i < 0) return null;
  const body = css.slice(i + selector.length + 2, css.indexOf("}", i));
  const out = {};
  for (const m of body.matchAll(/(--[a-z0-9-]+)\s*:\s*([^;]+);/g)) out[m[1]] = m[2].trim();
  return out;
}
const sameMap = (a, b) => JSON.stringify(Object.entries(a).sort()) === JSON.stringify(Object.entries(b).sort());
const diffMap = (a, b) =>
  [...new Set([...Object.keys(a), ...Object.keys(b)])]
    .filter((k) => a[k] !== b[k])
    .map((k) => `${k}: 生成=${a[k] ?? "∅"} 内置=${b[k] ?? "∅"}`)
    .join("; ");
if (vt.theme) {
  const css = themeCss("sparrow-sun", vt.theme, () => null);
  const builtin = readFileSync(`${SRC}themes/sparrow.css`, "utf8");
  const genLight = varsOf(css.full, ':root:where([data-theme~="ccr-sparrow-sun"]):not(.dark)');
  const genDark = varsOf(css.full, '.dark:where([data-theme~="ccr-sparrow-sun"])');
  const inLight = varsOf(builtin, ':root:where([data-theme~="sparrow"])');
  const inDark = varsOf(builtin, '.dark:where([data-theme~="sparrow"])');
  check("生成浅色块存在", genLight && inLight);
  check("生成深色块存在", genDark && inDark);
  if (genLight && inLight) check("浅色: 插件版 == 内置 sparrow.css", sameMap(genLight, inLight), diffMap(genLight, inLight));
  if (genDark && inDark) check("深色: 插件版 == 内置 sparrow.css", sameMap(genDark, inDark), diffMap(genDark, inDark));
  check("没有资源时 full == tokensOnly", css.full === css.tokensOnly);
  // 全部颜色 token 都写了, 范例才算完整的「示范」
  check("范例覆盖全部颜色 token (浅色, popover 回退除外)", Object.keys(COLOR_TOKENS).filter((k) => k !== "popover").every((k) => k in exTheme.light.colors));
}
{
  // 资源: 合法 data URL 进 CSS, 非法的被丢弃
  const t = validateTheme(
    { themeApi: 1, light: { textures: { canvas: { image: "p.png", size: "320px" } } }, fonts: { hand: "h.woff2" } },
    ["p.png", "h.woff2"],
  ).theme;
  const ok = themeCss("demo-x", t, (p) => (p === "p.png" ? "data:image/png;base64,iVBORw0KGgo=" : "data:font/woff2;base64,d09GMg=="));
  check("质感进 CSS", ok.full.includes('--q-tx-canvas: url("data:image/png;base64,iVBORw0KGgo=")') && ok.full.includes("--q-tx-canvas-size: 320px"));
  check("纹理规则只给用到的位置", ok.full.includes(':where([data-theme~="ccr-demo-x"]) :is(.main)') && !ok.full.includes(":is(.sidebar)"));
  check("字体进 CSS", ok.full.includes('font-family: "ccr-demo-x-hand"') && ok.full.includes('--font-hand: "ccr-demo-x-hand"'));
  check("tokensOnly 不含资源", !ok.tokensOnly.includes("data:"));
  const evil = themeCss("demo-x", t, () => 'data:image/png;base64,AAAA"); } html { background: red');
  check("非法 data URL 被丢弃", !evil.full.includes("html {") && !evil.full.includes("--q-tx-canvas:"));
  let threw = false;
  try {
    themeCss('x"]{}', t, () => null);
  } catch {
    threw = true;
  }
  check("非法插件 id 拒绝生成", threw);
}

// ---------- 5. Rust 与 TS 两份名单一致 ----------
{
  const rs = readFileSync(`${ROOT}src-tauri/src/plugins/mod.rs`, "utf8");
  const listOf = (name) => {
    const m = new RegExp(`pub const ${name}: &\\[&str\\] = &\\[([^\\]]*)\\]`).exec(rs);
    return m ? [...m[1].matchAll(/"([^"]+)"/g)].map((x) => x[1]) : null;
  };
  const same = (a, b) => a && JSON.stringify([...a].sort()) === JSON.stringify([...b].sort());
  check("RESERVED_IDS: Rust == TS", same(listOf("RESERVED_IDS"), RESERVED_IDS), JSON.stringify(listOf("RESERVED_IDS")));
  check("SUPPORTED_TYPES: Rust == TS", same(listOf("SUPPORTED_TYPES"), SUPPORTED_TYPES), JSON.stringify(listOf("SUPPORTED_TYPES")));
}

// ---------- 6. 包哈希 ----------
for (const [i, v] of readJson("src/plugins/hash-vectors.json").entries()) {
  check(`哈希向量 #${i}`, packageHash(v.files) === v.sha256);
}

if (failures.length) {
  console.error(`check-plugins: ${failures.length} 项失败`);
  for (const f of failures) console.error(`  ✗ ${f}`);
  process.exit(1);
}
console.log("check-plugins: 全部通过");
