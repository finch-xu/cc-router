export function fmtNum(n?: number | null): string {
  if (n == null) return "-";
  return n.toLocaleString("zh-CN");
}

export function fmtKilo(n: number): string {
  if (n >= 1000) return (n / 1000).toFixed(0) + "K";
  return String(n);
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
