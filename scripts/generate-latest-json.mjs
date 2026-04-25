#!/usr/bin/env node
// 用 release 里上传的 .sig 文件 + tag 拼出 Tauri updater 要求的 latest.json。
// 用法: node scripts/generate-latest-json.mjs <tag> <sig-dir> <out-file>
// 环境变量:
//   GH_TOKEN  - gh CLI 鉴权 (workflow 里通常是 secrets.GITHUB_TOKEN)
//   GH_REPO   - owner/repo (用来拼下载 URL)

import { readFileSync, writeFileSync, readdirSync } from "node:fs";
import { resolve } from "node:path";
import { execFileSync } from "node:child_process";

const [, , tag, sigDir, outFile] = process.argv;
if (!tag || !sigDir || !outFile) {
  console.error("用法: generate-latest-json.mjs <tag> <sig-dir> <out-file>");
  process.exit(1);
}

const repo = process.env.GH_REPO;
if (!repo) {
  console.error("GH_REPO 环境变量未设置");
  process.exit(1);
}

const version = tag.startsWith("v") ? tag.slice(1) : tag;

let notes = "";
try {
  const out = execFileSync(
    "gh",
    ["release", "view", tag, "--repo", repo, "--json", "body", "--jq", ".body"],
    { encoding: "utf8" },
  );
  notes = out.trim();
} catch (e) {
  console.warn("无法获取 release body, 使用空 notes:", e.message);
}

const files = readdirSync(sigDir);
console.log("发现 sig 文件:", files);

const platforms = {};

for (const sigName of files) {
  if (!sigName.endsWith(".sig")) continue;
  const baseName = sigName.slice(0, -4);
  const sigContent = readFileSync(resolve(sigDir, sigName), "utf8");
  const url = `https://github.com/${repo}/releases/download/${tag}/${baseName}`;

  let key = null;
  if (baseName.endsWith(".app.tar.gz")) {
    key = "darwin-aarch64";
  } else if (baseName.endsWith(".exe")) {
    // Tauri 2 直接对 NSIS .exe installer 签名,不再生成 v1 的 .nsis.zip
    key = "windows-x86_64";
  } else if (baseName.endsWith(".AppImage")) {
    key = "linux-x86_64";
  }
  if (!key) {
    console.warn("跳过未识别平台的 sig:", baseName);
    continue;
  }
  if (platforms[key]) {
    console.warn(`平台 ${key} 已有 entry, 覆盖为 ${baseName}`);
  }
  platforms[key] = { signature: sigContent, url };
}

const required = ["darwin-aarch64", "windows-x86_64", "linux-x86_64"];
for (const k of required) {
  if (!platforms[k]) {
    throw new Error(
      `缺少平台 ${k} 的 .sig - 检查 build job 是否启用了 TAURI_SIGNING_PRIVATE_KEY 和 release 资产是否完整`,
    );
  }
}

const manifest = {
  version,
  notes,
  pub_date: new Date().toISOString(),
  platforms,
};

writeFileSync(outFile, JSON.stringify(manifest, null, 2));
console.log(`写入 ${outFile}:`);
console.log(JSON.stringify(manifest, null, 2));
