import type { DailySeriesPointDto, StatsRange } from "@/types";
import { eachLocalDay, localDayKey, localDaysAgo, localToday, parseLocalDay } from "@/lib/localDay";

/** 一个补零后的时间桶: 后端只返回有数据的桶, 这里按范围铺满, 让柱图无缺口、X 轴刻度均匀。 */
export interface SeriesBucket {
  /** 唯一 key: 日 `YYYY-MM-DD` 或 小时 `YYYY-MM-DD#H` */
  key: string;
  /** X 轴刻度: 日 → `M/D`, 小时 → `HH:00` */
  label: string;
  /** tooltip 标题: 完整本地日期 (+ 时刻) */
  title: string;
  point: DailySeriesPointDto | null;
}

/** range 对应「包含今天共 N 天」的回推天数; all_time 无固定起点 (取数据最早一天)。 */
export function rangeDaysBack(range: StatsRange): number | null {
  switch (range) {
    case "today":
      return 0;
    case "last7_days":
      return 6;
    case "last30_days":
      return 29;
    case "last90_days":
      return 89;
    case "all_time":
      return null;
  }
}

export function isHourlyRange(range: StatsRange): boolean {
  return range === "today";
}

export function buildSeriesBuckets(points: DailySeriesPointDto[], range: StatsRange): SeriesBucket[] {
  if (isHourlyRange(range)) {
    const byHour = new Map<number, DailySeriesPointDto>();
    for (const p of points) if (p.hour != null) byHour.set(p.hour, p);
    const today = localToday();
    const dateStr = today.toLocaleDateString();
    const lastHour = new Date().getHours();
    const out: SeriesBucket[] = [];
    for (let h = 0; h <= lastHour; h++) {
      const hh = String(h).padStart(2, "0");
      out.push({
        key: `${localDayKey(today)}#${h}`,
        label: `${hh}:00`,
        title: `${dateStr} ${hh}:00`,
        point: byHour.get(h) ?? null,
      });
    }
    return out;
  }

  const byDay = new Map<string, DailySeriesPointDto>();
  for (const p of points) byDay.set(p.day, p);
  const daysBack = rangeDaysBack(range);
  const today = localToday();
  let from: Date;
  if (daysBack != null) {
    from = localDaysAgo(daysBack);
  } else {
    // all_time: 从数据最早一天铺到今天; 无数据则空数组
    const firstKey = points.map((p) => p.day).sort()[0];
    if (!firstKey) return [];
    from = parseLocalDay(firstKey);
  }
  return eachLocalDay(from, today).map((d) => {
    const key = localDayKey(d);
    return {
      key,
      label: d.toLocaleDateString(undefined, { month: "numeric", day: "numeric" }),
      title: d.toLocaleDateString(),
      point: byDay.get(key) ?? null,
    };
  });
}
