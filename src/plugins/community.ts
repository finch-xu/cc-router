/**
 * 社区目录 index.json 的宽松解析: 不认识 / 不完整的条目直接跳过, 不让一条坏数据拖垮整个商店。
 * 格式由 cc-router-community 仓库的 publish 流程生成 (见实施文档 §2.6)。零依赖。
 */
import { SUPPORTED_TYPES, compareVersions, isSafePath, type LocalizedText, type PluginType } from "./manifest";
import { safeHttpsUrl } from "./links";

export const COMMUNITY_REPO = "finch-xu/cc-router-community";
export const COMMUNITY_RAW_BASE = `https://raw.githubusercontent.com/${COMMUNITY_REPO}/main/`;
export const COMMUNITY_URL = `https://github.com/${COMMUNITY_REPO}`;

export interface CommunityEntry {
  id: string;
  type: PluginType;
  name: LocalizedText;
  description?: LocalizedText;
  author: string;
  license?: string;
  /** 插件介绍页 (清单 `homepage`, 由目录的发布流程抄进 index.json; 旧目录没有这个字段) */
  homepage?: string;
  /** owner/repo */
  repo: string;
  path?: string;
  tags: string[];
  logo?: string;
  screenshots: string[];
  latest: { version: string; commit: string; sha256: string; minApp?: string };
  revoked?: string;
}

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown, max = 200): string | undefined => (typeof v === "string" && v.length > 0 && v.length <= max ? v : undefined);
const text = (v: unknown): LocalizedText | undefined => {
  if (typeof v === "string") return str(v, 400);
  if (!isObj(v)) return undefined;
  const out: Record<string, string> = {};
  for (const [k, val] of Object.entries(v)) if (str(val, 400)) out[k] = val as string;
  return Object.keys(out).length ? out : undefined;
};
const mediaUrl = (v: unknown): string | undefined => (isSafePath(v) ? COMMUNITY_RAW_BASE + v : undefined);

export function parseCommunityIndex(raw: unknown): CommunityEntry[] {
  if (!isObj(raw) || raw.index !== 1 || !Array.isArray(raw.plugins)) return [];
  const out: CommunityEntry[] = [];
  for (const p of raw.plugins) {
    if (!isObj(p) || !isObj(p.latest)) continue;
    const id = str(p.id, 40);
    const type = str(p.type, 20);
    const name = text(p.name);
    const author = str(p.author, 80);
    const repo = str(p.repo, 140);
    const commit = str(p.latest.commit, 40);
    const sha256 = str(p.latest.sha256, 64);
    const version = str(p.latest.version, 40);
    if (!id || !/^[a-z0-9][a-z0-9-]{1,39}$/.test(id)) continue;
    if (!type || !(SUPPORTED_TYPES as readonly string[]).includes(type)) continue;
    if (!name || !author || !repo || !/^[A-Za-z0-9-]+\/[A-Za-z0-9._-]+$/.test(repo)) continue;
    if (!commit || !/^[0-9a-f]{40}$/.test(commit) || !sha256 || !/^[0-9a-f]{64}$/.test(sha256) || !version) continue;
    const path = p.path === undefined || p.path === null ? undefined : isSafePath(p.path) ? p.path : null;
    if (path === null) continue;
    out.push({
      id,
      type: type as PluginType,
      name,
      description: text(p.description),
      author,
      license: str(p.license, 64),
      homepage: safeHttpsUrl(p.homepage),
      repo,
      path,
      tags: Array.isArray(p.tags) ? p.tags.filter((t): t is string => typeof t === "string" && /^[a-z0-9:-]{1,30}$/.test(t)).slice(0, 5) : [],
      logo: mediaUrl(p.logo),
      screenshots: Array.isArray(p.screenshots) ? p.screenshots.map(mediaUrl).filter((u): u is string => !!u).slice(0, 4) : [],
      latest: { version, commit, sha256, minApp: str(p.latest.minApp, 40) },
      revoked: str(p.revoked, 400),
    });
  }
  return out;
}

/** 给「从链接安装」用的定位串: owner/repo@commit[/path] */
export function entryLink(e: CommunityEntry): string {
  return `${e.repo}@${e.latest.commit}${e.path ? `/${e.path}` : ""}`;
}

/** 已装版本是否比目录里的旧 (同来源的社区插件才提示更新) */
export function hasUpdate(installedVersion: string, entry: CommunityEntry): boolean {
  return compareVersions(installedVersion, entry.latest.version) < 0;
}
