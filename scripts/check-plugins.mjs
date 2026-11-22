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

// ---------- 4b. 小票插件 ----------
{
  const { parseTemplate, renderTemplate } = await import(`${SRC}plugins/receipt/template.ts`);
  const { validateTemplateHtml } = await import(`${SRC}plugins/receipt/html.ts`);
  const { parseReceiptCss, scopedCss } = await import(`${SRC}plugins/receipt/css.ts`);
  const { validateReceipt } = await import(`${SRC}plugins/receipt/validate.ts`);
  const { receiptData, sampleReceiptDto } = await import(`${SRC}plugins/receipt/fields.ts`);

  // 模板语言
  const tpl = (src, data) => {
    const r = parseTemplate(src);
    return r.nodes ? renderTemplate(r.nodes, data) : `ERR:${JSON.stringify(r.errors)}`;
  };
  check("模板: 输出转义", tpl("<b>{{x}}</b>", { x: "<img onerror=1>&" }) === "<b>&lt;img onerror=1&gt;&amp;</b>");
  check("模板: each + 外层字段 + @index", tpl("{{#each xs}}{{@index}}{{n}}{{k}};{{/each}}", { k: "K", xs: [{ n: "a" }, { n: "b" }] }) === "0aK;1bK;");
  check("模板: if / else / unless", tpl("{{#if a}}Y{{else}}N{{/if}}{{#unless a}}U{{/unless}}", { a: [] }) === "NU");
  check("模板: 对象不输出", tpl("[{{o}}]", { o: { a: 1 } }) === "[]");
  check("模板: 原型链字段取不到", tpl("[{{constructor.name}}][{{toString}}][{{xs.constructor}}]", { xs: [] }) === "[][][]");
  for (const bad of ["{{#each a}}", "{{/if}}", "{{{raw}}}", "{{a b}}", "{{#if a}}{{/each}}", "{{x", "{{constructor.prototype}}x{{__proto__.y}}"]) {
    const r = parseTemplate(bad);
    // 原型链字段语法上合法, 但 lookup 只认自有键: 渲染结果为空
    if (bad.includes("proto")) check(`模板: 原型链取不到值 ${bad}`, r.nodes && renderTemplate(r.nodes, {}) === "x");
    else check(`模板拒绝 ${JSON.stringify(bad)}`, r.nodes === null);
  }

  // HTML 白名单
  const htmlOk = (src) => validateTemplateHtml(src).length === 0;
  for (const good of ["<div class=\"a b\"><span>{{x}}</span></div>", "<p>a<br>b</p><hr>", "<ccr-qr size=\"80\"></ccr-qr><ccr-logo/>", "<table><tr><td colspan=\"2\">x</td></tr></table>", "<!-- note --><b>&amp; ok</b>"]) {
    check(`HTML 接受 ${good}`, htmlOk(good), JSON.stringify(validateTemplateHtml(good)));
  }
  for (const bad of [
    "<script>alert(1)</script>", "<img src=x onerror=alert(1)>", "<div onclick=\"x()\">a</div>", "<a href=\"https://evil\">x</a>",
    "<div style=\"color:red\">x</div>", "<div class=\"{{cls}}\">x</div>", "<div id=\"x\">a</div>", "<iframe></iframe>", "<svg><script/></svg>",
    "<div>", "</div>", "<ccr-qr size=\"999\"></ccr-qr>", "<ccr-qr onload=\"x\"></ccr-qr>", "<div class=\"a\"x=1>b</div>", "<div class='a\"><img'>x</div>",
    "<b>{{x}}</b><{{tag}}>", "<style>body{}</style>",
  ]) {
    check(`HTML 拒绝 ${bad}`, !htmlOk(bad));
  }

  // CSS 白名单 + 作用域
  const cssOk = (src) => parseReceiptCss(src).rules !== null;
  for (const good of [".a { color: #fff; font-family: \"Space Mono\", monospace }", "h1, .b > span:first-child { margin: 0 }", ".c::before { content: \"★ 小票\" }", "/* 注释; } */ .d { clip-path: polygon(0 0, 100% 0, 50% 100%) }"]) {
    check(`CSS 接受 ${good}`, cssOk(good), JSON.stringify(parseReceiptCss(good).errors));
  }
  for (const bad of [
    ".a { background: url(https://evil/x.png) }", "@import \"x.css\";", "@media print { .a { color: red } }", "body { color: red }", ":root { --x: 1 }",
    "html .a { color: red }", ".a { position: fixed }", ".a { width: expression(alert(1)) }", ".a { color: \\72 ed }", ".a { b\\ackground: red }",
    ".a[href] { color: red }", ".a { color: red", "x { content: \"</style><script>\" }", ".a { font-family: \"a\";} .b { color: \"x }",
    ".a { background: image-set(\"x.png\" 1x) }",
  ]) {
    check(`CSS 拒绝 ${bad}`, !cssOk(bad));
  }
  const scoped = scopedCss(parseReceiptCss("h1, .a span { color: red }").rules, "ccr-rcpt-demo");
  check("CSS 作用域前缀", scoped.startsWith(".ccr-rcpt-demo h1, .ccr-rcpt-demo .a span {"), scoped);

  // 范例: 校验 + 用样例数据渲染
  const dir = "examples/plugins/receipt-sparrow-bakery/";
  const man = readJson(`${dir}cc-router-plugin.json`);
  const mr = validateManifest(man, "6.7.0");
  check("小票范例清单合法", mr.manifest !== null && mr.warnings.length === 0, JSON.stringify([mr.errors, mr.warnings]));
  const texts = Object.fromEntries(man.assets.filter((a) => /\.(html|css)$/.test(a)).map((a) => [a, readFileSync(ROOT + dir + a, "utf8")]));
  const vr = validateReceipt(readJson(`${dir}receipt.json`), man.assets, texts);
  check("小票范例合法且无警告", vr.receipt !== null && vr.warnings.length === 0, JSON.stringify([vr.errors, vr.warnings]));
  if (vr.receipt) {
    const html = renderTemplate(vr.receipt.nodes, receiptData(sampleReceiptDto(), { port: 23456, version: "9.9.9", deletedLabel: "(deleted)" }));
    for (const want of ["No. 5A7C0E21", "MODEL-OPUS", "claude-opus-4-7", "sold out", "(deleted)", "46.52M", "711", "v9.9.9", "<ccr-qr size=\"72\">"]) {
      check(`小票范例渲染包含 ${want}`, html.includes(want));
    }
    // 跟随小票页的显示设置: 按订阅分组 / 不显示请求数 / 全写数字 / 关掉缓存
    const { DEFAULT_RECEIPT_OPTIONS } = await import(`${SRC}plugins/receipt/fields.ts`);
    const ctx = { port: 23456, version: "9.9.9", deletedLabel: "(deleted)" };
    const bySub = renderTemplate(vr.receipt.nodes, receiptData(sampleReceiptDto(), ctx, { ...DEFAULT_RECEIPT_OPTIONS, groupMode: "subscription", showRequests: false, compact: false, showCache: false }));
    check("设置: 按订阅分组", bySub.includes("Anthropic Main") && !bySub.includes("MODEL-OPUS"));
    check("设置: 隐藏请求数", !bySub.includes("loaves sold") && !bySub.includes("× 388"));
    check("设置: 全写数字", bySub.includes("46,522,000"));
    check("设置: 关掉缓存行", !bySub.includes("cache write"));
    const totalsOnly = renderTemplate(vr.receipt.nodes, receiptData(sampleReceiptDto(), ctx, { ...DEFAULT_RECEIPT_OPTIONS, groupMode: "totals_only" }));
    check("设置: 只看合计", totalsOnly.includes("BY SUBSCRIPTION") && totalsOnly.includes("BY MODEL") && !totalsOnly.includes(" / </span>"));
    check("默认: 分组按虚拟模型且带分隔", html.includes("Anthropic / Anthropic Main"));
    check("小票范例渲染结果仍过 HTML 白名单", validateTemplateHtml(html).length === 0, JSON.stringify(validateTemplateHtml(html).slice(0, 3)));
  }
  const vrBad = validateReceipt({ receiptApi: 1, template: "t.html", fonts: { "Bad;Name": "f.woff2" } }, ["t.html"], { "t.html": "<div>{{receipt.nope}}</div>" });
  check("小票: 字体名非法 / 字体未登记 → 错误", vrBad.receipt === null && vrBad.errors.some((e) => e.path.startsWith("fonts.")));
  const vrWarn = validateReceipt({ receiptApi: 1, template: "t.html" }, ["t.html"], { "t.html": "<div>{{receipt.nope}}</div>" });
  check("小票: 未知字段只警告", vrWarn.receipt !== null && vrWarn.warnings.some((w) => w.detail === "{{receipt.nope}}"));
  check("清单: 主题插件不能带 html", validateManifest({ ...exManifest, assets: ["a.html"] }, "6.7.0").manifest === null);
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
