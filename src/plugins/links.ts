/**
 * 插件的「介绍链接」: 作者在清单里写的 `homepage`, 没写时退回插件源码在 GitHub 上的位置。
 * 零依赖 (社区仓库 CI 也可以直接 import)。
 */

/** 与 `manifest.ts` 对 `homepage` 的校验同一条规则; 渲染前再查一次, 不把任意 scheme 交给 openExternal */
export function safeHttpsUrl(v: unknown): string | undefined {
  return typeof v === "string" && v.length <= 300 && /^https:\/\/[^\s"'<>]+$/.test(v) ? v : undefined;
}

/**
 * 插件源码在 GitHub 上的位置。钉了提交号就指向那个提交 (= 用户装的那一版, 路径事后被挪走也不会 404);
 * 没有提交号但有子目录时走默认分支 (`HEAD`)。
 */
export function githubSourceUrl(repo: string, commit?: string | null, path?: string | null): string | undefined {
  if (!/^[A-Za-z0-9-]+\/[A-Za-z0-9._-]+$/.test(repo)) return undefined;
  const base = `https://github.com/${repo}`;
  const sub = path ? `/${path.split("/").map(encodeURIComponent).join("/")}` : "";
  if (commit && /^[0-9a-f]{7,40}$/.test(commit)) return `${base}/tree/${commit}${sub}`;
  return sub ? `${base}/tree/HEAD${sub}` : base;
}

export interface PluginLinks {
  /** 作者写的插件介绍页 (清单 `homepage`) */
  homepage?: string;
  /** 插件源码 (GitHub 上的仓库 / 子目录) */
  source?: string;
}

/** 本地文件夹安装的插件没有源码链接, 只有作者写了 homepage 才有介绍链接 */
export function pluginLinks(opts: {
  homepage?: unknown;
  repo?: string | null;
  commit?: string | null;
  path?: string | null;
}): PluginLinks {
  const homepage = safeHttpsUrl(opts.homepage);
  const source = opts.repo ? githubSourceUrl(opts.repo, opts.commit, opts.path) : undefined;
  return { homepage, source: source !== homepage ? source : undefined };
}
