#!/usr/bin/env node
// 将版本号同步写入 package.json / src-tauri/tauri.conf.json / src-tauri/Cargo.toml / src-tauri/Cargo.lock。
// 用法：pnpm version:set 0.2.0

import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const version = process.argv[2];
if (!version || !/^\d+\.\d+\.\d+(-[\w.-]+)?$/.test(version)) {
  console.error("用法: pnpm version:set <X.Y.Z>   (例如 0.2.0)");
  process.exit(1);
}

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");

function updateJson(relPath) {
  const full = resolve(root, relPath);
  const original = readFileSync(full, "utf8");
  const obj = JSON.parse(original);
  obj.version = version;
  // 保留尾部换行
  const trailing = original.endsWith("\n") ? "\n" : "";
  writeFileSync(full, JSON.stringify(obj, null, 2) + trailing);
  console.log(`  ${relPath}  →  ${version}`);
}

function updateCargoToml(relPath) {
  const full = resolve(root, relPath);
  const original = readFileSync(full, "utf8");
  // 只改 [package] 块里第一条 `version = "..."`，避免误伤依赖声明
  let replaced = false;
  const updated = original.replace(
    /^(version\s*=\s*")([^"]*)(")/m,
    (_, p, _old, q) => {
      replaced = true;
      return `${p}${version}${q}`;
    },
  );
  if (!replaced) {
    throw new Error(`未在 ${relPath} 找到顶层 version 字段`);
  }
  writeFileSync(full, updated);
  console.log(`  ${relPath}  →  ${version}`);
}

// Cargo.lock 里本工作区包 cc-router 的 version 也要跟着改, 否则 lock 与 Cargo.toml
// 不一致, CI 用 --frozen/--locked 会失败。只改 name = "cc-router" 紧跟的那行 version,
// 不动任何依赖条目 (本地包无 checksum, 这一行就是 cargo 自己会写的内容)。
function updateCargoLock(relPath) {
  const full = resolve(root, relPath);
  if (!existsSync(full)) {
    console.log(`  ${relPath}  (不存在, 跳过 — 首次 cargo build 会生成)`);
    return;
  }
  const original = readFileSync(full, "utf8");
  let replaced = false;
  const updated = original.replace(
    /(name = "cc-router"\r?\nversion = ")([^"]*)(")/,
    (_, p, _old, q) => {
      replaced = true;
      return `${p}${version}${q}`;
    },
  );
  if (!replaced) {
    throw new Error(`未在 ${relPath} 找到 cc-router 包的 version 字段`);
  }
  writeFileSync(full, updated);
  console.log(`  ${relPath}  →  ${version}`);
}

console.log(`同步版本号到 ${version}：`);
updateJson("package.json");
updateJson("src-tauri/tauri.conf.json");
updateCargoToml("src-tauri/Cargo.toml");
updateCargoLock("src-tauri/Cargo.lock");
console.log("完成。建议接下来：");
console.log(`  git add -u`);
console.log(`  git commit -m "Bump version to ${version}"`);
console.log(`  git tag v${version}`);
console.log(`  git push && git push --tags`);
