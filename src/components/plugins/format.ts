import type { TFunction as T } from "@/i18n";
import type { PluginSource } from "@/types";

export function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

/** 「社区 · owner/repo @ 9f3c2a1」/「本地 · /path」 */
export function sourceText(t: T, s: PluginSource): string {
  const kind = t(`plugins.source.${s.kind}`);
  if (s.kind === "local") return `${kind} · ${s.location}`;
  const where = s.path ? `${s.location}/${s.path}` : s.location;
  return `${kind} · ${where}${s.commit ? ` @ ${s.commit.slice(0, 7)}` : ""}`;
}
