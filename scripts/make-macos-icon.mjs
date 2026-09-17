#!/usr/bin/env node
// 生成符合 Apple 图标网格的 macOS 专用图标 (src-tauri/icons/icon.icns)。
// 用法：pnpm icon:macos        (只能在 macOS 上跑, 依赖系统自带的 sips)
//
// 为什么 macOS 要单独一份: Apple 的图标模板是 1024 画布里主体只占 824×824 (约 80.5%),
// 四周留 100px 透明边 —— 系统 app 全都如此, 而 Dock 不会替第三方图标缩放。原稿 assets/icon.png
// 是满铺的, 直接拿去生成 icns 会在 Dock / Cmd+Tab 里比别的 app 大一圈。
// Windows 的 .ico 和 Linux 的 png 惯例就是满铺, 所以**不能**改原稿后整套重新生成,
// 只替换 icon.icns 这一个文件; 其余尺寸仍由 `pnpm tauri icon assets/icon.png` 负责。

import { execFileSync } from "node:child_process";
import { copyFileSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

if (process.platform !== "darwin") {
  console.error("make-macos-icon 依赖 macOS 自带的 sips, 请在 macOS 上运行。");
  process.exit(1);
}

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const SOURCE = join(root, "assets/icon.png"); // 满铺原稿, 1024×1024
const PADDED = join(root, "assets/icon-macos.png"); // 留白后的 macOS 母版, 入库方便 review
const ICNS = join(root, "src-tauri/icons/icon.icns");

const CANVAS = 1024;
const BODY = 824; // Apple macOS 图标网格: 主体边长 (= 系统 app 实测的 80.5%)

const run = (cmd, args) => execFileSync(cmd, args, { cwd: root, stdio: ["ignore", "ignore", "inherit"] });

const tmp = mkdtempSync(join(tmpdir(), "ccr-icon-"));
try {
  // 1. 缩到主体尺寸  2. 居中补透明边到整画布 (sips -p 对带 alpha 的 png 补的是全透明, 已实测)
  const body = join(tmp, "body.png");
  run("sips", ["-z", String(BODY), String(BODY), SOURCE, "--out", body]);
  run("sips", ["-p", String(CANVAS), String(CANVAS), body, "--out", PADDED]);

  // 3. 借 tauri icon 生成 icns (各档尺寸 + 打包格式都交给它), 只取 icns 一个文件
  const out = join(tmp, "out");
  run("pnpm", ["tauri", "icon", PADDED, "-o", out]);
  copyFileSync(join(out, "icon.icns"), ICNS);

  console.log(`  assets/icon-macos.png        主体 ${BODY}/${CANVAS}`);
  console.log("  src-tauri/icons/icon.icns    已更新 (其余平台图标未动)");
} finally {
  rmSync(tmp, { recursive: true, force: true });
}
