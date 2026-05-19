export function fmtNum(n?: number | null): string {
  if (n == null) return "-";
  return n.toLocaleString("zh-CN");
}

export function fmtKilo(n: number): string {
  if (n >= 1000) return (n / 1000).toFixed(0) + "K";
  return String(n);
}

/**
 * 把 token 数字压缩成 K/M/B/T 紧凑形式, 保留 2 位小数。
 *   null / undefined → "-"; < 1e3 原样; < 1e6 → "X.XXK"; < 1e9 → "X.XXM"; < 1e12 → "X.XXB"; ≥ 1e12 → "X.XXT"
 */
export function fmtCompact(n?: number | null): string {
  if (n == null) return "-";
  if (n < 1000) return String(n);
  if (n < 1_000_000) return (n / 1000).toFixed(2) + "K";
  if (n < 1_000_000_000) return (n / 1_000_000).toFixed(2) + "M";
  if (n < 1_000_000_000_000) return (n / 1_000_000_000).toFixed(2) + "B";
  return (n / 1_000_000_000_000).toFixed(2) + "T";
}

export function fmtTime(ms: number): string {
  return new Date(ms).toLocaleString("zh-CN", { hour12: false });
}

export function fmtTimeShort(ms?: number): string {
  if (!ms) return "—";
  return new Date(ms).toLocaleTimeString("zh-CN", {
    hour12: false,
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function fmtLatencyMs(ms?: number | null): string {
  if (ms == null) return "-";
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(2)}s`;
}

export function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(2)} MB`;
}

/** "刚刚 / N 分钟前 / N 小时前 / N 天前" with i18n. `t` resolves keys under `common.relativeTime.*`. */
export function fmtRelativeTime(ms: number, t: (k: string) => string): string {
  const diff = Date.now() - ms;
  if (diff < 60_000) return t("common.relativeTime.justNow");
  if (diff < 3_600_000)
    return `${Math.floor(diff / 60_000)}${t("common.relativeTime.minutesAgo")}`;
  if (diff < 86_400_000)
    return `${Math.floor(diff / 3_600_000)}${t("common.relativeTime.hoursAgo")}`;
  return `${Math.floor(diff / 86_400_000)}${t("common.relativeTime.daysAgo")}`;
}
