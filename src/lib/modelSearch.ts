import type { ModelInfo } from "@/types";

/** 下拉一次最多渲染的条数; 超出的提示用户继续输入缩小范围 (代替分页 / 虚拟滚动)。 */
export const MODEL_RENDER_LIMIT = 200;

/** 大小写与分隔符都不算数: `glm5` 与 `@cf/zai-org/glm-5.3`、`kimi code` 与 `kimi-k2.7-code` 都能对上。 */
function normalize(s: string): string {
  return s.toLowerCase().replace(/[\s\-_./:@]+/g, "");
}

/**
 * 0 = 不匹配, 越大越靠前。连续命中 (子串) 恒高于按顺序零散命中 (子序列);
 * 子串里越靠前越好, 子序列里跳过的字符越少越好。
 */
export function modelMatchScore(query: string, candidate: string): number {
  const q = normalize(query);
  if (!q) return 1;
  const c = normalize(candidate);
  const at = c.indexOf(q);
  if (at >= 0) return 10_000 - at;
  let gaps = 0;
  let ci = 0;
  for (const ch of q) {
    const found = c.indexOf(ch, ci);
    if (found < 0) return 0;
    gaps += found - ci;
    ci = found + 1;
  }
  return Math.max(1, 1_000 - gaps);
}

/**
 * 按 query 过滤并排序 (同分保持原顺序), 截到 MODEL_RENDER_LIMIT 条。
 * `keep` (当前选中项) 只要在列表里就一定保留: Radix Select 卸载选中项会把焦点从搜索框抢走。
 */
export function searchModels(
  models: ModelInfo[],
  query: string,
  keep: string,
): { shown: ModelInfo[]; total: number } {
  const ranked = models
    .map((m, i) => ({
      m,
      i,
      score: Math.max(modelMatchScore(query, m.id), modelMatchScore(query, m.display_name ?? "")),
    }))
    .filter((r) => r.score > 0 || r.m.id === keep)
    .sort((a, b) => b.score - a.score || a.i - b.i);
  const shown = ranked.slice(0, MODEL_RENDER_LIMIT).map((r) => r.m);
  const kept = ranked.find((r) => r.m.id === keep);
  if (kept && !shown.includes(kept.m)) shown.push(kept.m);
  return { shown, total: ranked.length };
}
