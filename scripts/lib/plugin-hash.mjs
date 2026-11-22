// 插件包哈希 (与 Rust `plugins::files::package_hash` 同算法, 测试向量 src/plugins/hash-vectors.json 锁住两边):
// 按路径字典序 (UTF-8 字节序) 遍历文件, 依次喂入 路径字节 + 0x00 + 内容长度 (u64 大端) + 内容, 整体 SHA-256, 小写 hex。
// 社区仓库 CI 也用这个文件。
import { createHash } from "node:crypto";

/** @param {Record<string, Uint8Array | string>} files 路径 → 内容 (字符串按 UTF-8) */
export function packageHash(files) {
  const enc = new TextEncoder();
  const entries = Object.entries(files).map(([p, c]) => [enc.encode(p), typeof c === "string" ? enc.encode(c) : c]);
  entries.sort(([a], [b]) => Buffer.compare(Buffer.from(a), Buffer.from(b)));
  const h = createHash("sha256");
  for (const [path, content] of entries) {
    h.update(path);
    h.update(Uint8Array.of(0));
    const len = Buffer.alloc(8);
    len.writeBigUInt64BE(BigInt(content.length));
    h.update(len);
    h.update(content);
  }
  return h.digest("hex");
}
