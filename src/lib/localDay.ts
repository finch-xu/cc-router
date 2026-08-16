/**
 * 本地日历日工具。统计聚合表的 key 是本地日历日字符串 `YYYY-MM-DD`
 * (后端 chrono::Local / SQLite date(...,'localtime')), 前端必须用本地 getter 拼 key,
 * **绝不能用 `toISOString()`** (那是 UTC 日期, 东八区晚上 8 点后就会差一天)。
 */

export function localDayKey(d: Date): string {
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

/** `YYYY-MM-DD` → 本地 0 点的 Date (`new Date(y, m-1, d)`, 不走 UTC 解析)。 */
export function parseLocalDay(key: string): Date {
  const [y, m, d] = key.split("-").map(Number);
  return new Date(y, m - 1, d);
}

/** 本地今天 0 点。 */
export function localToday(): Date {
  const now = new Date();
  return new Date(now.getFullYear(), now.getMonth(), now.getDate());
}

/** 从 `from` 起 (含) 到 `to` 止 (含) 逐日迭代, 用 setDate 递增, DST 日不会偏移。 */
export function eachLocalDay(from: Date, to: Date): Date[] {
  const out: Date[] = [];
  const cur = new Date(from.getFullYear(), from.getMonth(), from.getDate());
  const end = new Date(to.getFullYear(), to.getMonth(), to.getDate()).getTime();
  while (cur.getTime() <= end) {
    out.push(new Date(cur));
    cur.setDate(cur.getDate() + 1);
  }
  return out;
}

/** 今天往回 `n` 天 (n=0 即今天) 的本地 0 点。 */
export function localDaysAgo(n: number): Date {
  const t = localToday();
  t.setDate(t.getDate() - n);
  return t;
}
