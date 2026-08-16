import { useT } from "@/i18n";
import { fmtCompact, fmtNum } from "@/lib/format";
import type { OverallStatsDto } from "@/types";

/**
 * 5 个 KPI 卡。口径 (与后端 / 年历一致):
 * - 总 Token = input + output; 缓存读/写只在副行单独展示, 不相加 (OpenAI 系 input 已含 cached, 相加会双计)
 * - 缓存读取占比 = cache_read / (input + cache_read + cache_creation)
 */
export function KpiRow({ stats, loading }: { stats?: OverallStatsDto; loading?: boolean }) {
  const { t } = useT();
  const o = stats;
  const total = o?.total_requests ?? 0;
  const failCount = (o?.error_count ?? 0) + (o?.timeout_count ?? 0);
  const input = o?.total_input_tokens ?? 0;
  const output = o?.total_output_tokens ?? 0;
  const cacheRead = o?.total_cache_read_tokens ?? 0;
  const cacheWrite = o?.total_cache_creation_tokens ?? 0;
  const cacheDenom = input + cacheRead + cacheWrite;
  const cacheShare = cacheDenom > 0 ? (cacheRead / cacheDenom) * 100 : null;

  return (
    <div className={"stats-kpi" + (loading ? " is-loading" : "")}>
      <div className="stat">
        <div className="stat-label">{t("stats.kpi.totalRequests")}</div>
        <div className="stat-val tnum">{fmtNum(total)}</div>
        <div className="stat-delta">
          {fmtNum(o?.success_count ?? 0)} ✓ · {fmtNum(failCount)} ✕
        </div>
      </div>
      <div className="stat">
        <div className="stat-label">{t("stats.kpi.successRate")}</div>
        <div className="stat-val tnum">
          {(o?.success_rate_pct ?? 0).toFixed(1)}
          <span className="stat-unit">%</span>
        </div>
        <div className={"stat-delta" + (failCount > 0 ? " down" : "")}>
          {t("stats.kpi.failedFormat", { failed: fmtNum(failCount), total: fmtNum(total) })}
        </div>
      </div>
      <div className="stat">
        <div className="stat-label">{t("stats.kpi.avgDuration")}</div>
        <div className="stat-val tnum">
          {o?.avg_duration_ms != null ? (o.avg_duration_ms / 1000).toFixed(2) : "-"}
          <span className="stat-unit">s</span>
        </div>
        <div className="stat-delta">
          {o?.p95_duration_ms != null
            ? `${t("stats.kpi.p95Prefix")}${(o.p95_duration_ms / 1000).toFixed(2)}${t("stats.kpi.p95Suffix")}`
            : " "}
        </div>
      </div>
      <div className="stat">
        <div className="stat-label">{t("stats.kpi.totalTokens")}</div>
        <div className="stat-val tnum">{fmtCompact(input + output)}</div>
        <div className="stat-delta">
          {t("stats.kpi.totalTokensSub", { input: fmtCompact(input), output: fmtCompact(output) })}
        </div>
      </div>
      <div className="stat" title={t("stats.kpi.cacheReadHint")}>
        <div className="stat-label">{t("stats.kpi.cacheReadShare")}</div>
        <div className="stat-val tnum">
          {cacheShare != null ? cacheShare.toFixed(1) : "-"}
          <span className="stat-unit">%</span>
        </div>
        <div className="stat-delta">
          {t("stats.kpi.cacheReadSub", { read: fmtCompact(cacheRead), write: fmtCompact(cacheWrite) })}
        </div>
      </div>
    </div>
  );
}
