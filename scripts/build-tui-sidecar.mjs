#!/usr/bin/env node
// 编译 cc-router-tui 并放到 src-tauri/binaries/cc-router-tui-<target triple>[.exe],
// 供 tauri.conf.json::bundle.externalBin 打进安装包。
// 由 beforeDevCommand / beforeBuildCommand 调用; 也可以手动跑: node scripts/build-tui-sidecar.mjs
//
// Tauri 给每个钩子命令设置 TAURI_ENV_TARGET_TRIPLE 与 TAURI_ENV_DEBUG; 手动跑时回退到本机 triple + release。

import { execFileSync } from "node:child_process";
import { chmodSync, copyFileSync, mkdirSync, rmSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const srcTauri = join(root, "src-tauri");

const host = execFileSync("rustc", ["--print", "host-tuple"], { encoding: "utf8" }).trim();
const triple = process.env.TAURI_ENV_TARGET_TRIPLE || host;
const debug = process.env.TAURI_ENV_DEBUG === "true";
const cross = triple !== host;

// 只有交叉编译才传 --target: 本机编译时产物落在 target/<profile>/, 与主 crate 共用依赖缓存。
const args = ["build", "-p", "cc-router-tui", "--bin", "cc-router-tui-bin"];
if (!debug) args.push("--release");
if (cross) args.push("--target", triple);
execFileSync("cargo", args, { cwd: srcTauri, stdio: "inherit" });

const ext = triple.includes("windows") ? ".exe" : "";
const built = join(srcTauri, "target", ...(cross ? [triple] : []), debug ? "debug" : "release", `cc-router-tui-bin${ext}`);
const dest = join(srcTauri, "binaries", `cc-router-tui-${triple}${ext}`);

mkdirSync(dirname(dest), { recursive: true });
// 必须先删: 覆盖已存在的文件会沿用它的权限, 而 build.rs 放的占位是 0644 —— 不删的话 sidecar 没有可执行位。
rmSync(dest, { force: true });
copyFileSync(built, dest);
chmodSync(dest, 0o755);

console.log(`  cc-router-tui (${debug ? "debug" : "release"}, ${triple})  →  ${dest}  ${(statSync(dest).size / 1048576).toFixed(2)} MB`);
