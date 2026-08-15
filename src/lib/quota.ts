/** "5M" / "100m" / "2.5B" / "500k" / "1200000" → 整数 token 数; 非法返回 null; 空串返回 undefined (=不限). */
export function parseTokenShorthand(raw: string): number | null | undefined {
  const s = raw.trim().replace(/[,_\s]/g, "");
  if (s === "") return undefined;
  const m = /^(\d+(?:\.\d+)?)([kKmMbB]?)$/.exec(s);
  if (!m) return null;
  const n = parseFloat(m[1]);
  const mult = { "": 1, k: 1e3, m: 1e6, b: 1e9 }[m[2].toLowerCase() as "" | "k" | "m" | "b"];
  const v = Math.round(n * mult);
  return v > 0 && Number.isSafeInteger(v) ? v : null;
}

/**
 * 5_000_000 → "5M"; 1_500_000 → "1.5M"; 800 → "800"。
 * 无损: 只有当紧凑形式经 parseTokenShorthand 还原回来精确等于 n (小数最多 2 位导致的精度丢失会被
 * 检出) 才使用 k/M/B 简写, 否则退回 String(n) 原样数字, 避免重新保存时限额悄悄变了。
 */
export function formatTokenShorthand(n: number | null | undefined): string {
  if (n == null) return "";
  const units: Array<[number, string]> = [[1e9, "B"], [1e6, "M"], [1e3, "k"]];
  for (const [base, suf] of units) {
    if (n >= base) {
      const v = n / base;
      const candidate = `${Number.isInteger(v) ? v : v.toFixed(2).replace(/\.?0+$/, "")}${suf}`;
      return parseTokenShorthand(candidate) === n ? candidate : String(n);
    }
  }
  return String(n);
}
