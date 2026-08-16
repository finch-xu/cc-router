import { useMemo } from "react";
import { useNavigate } from "react-router-dom";
import { ProviderLogo } from "@/components/ProviderLogo";
import { useProviders } from "@/hooks/useProviders";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useT } from "@/i18n";
import { fmtCompact, fmtNum } from "@/lib/format";
import type { BreakdownDto, ProviderInfo, SubscriptionDto } from "@/types";
import { StatsCard } from "./StatsCard";

/** 订阅排行表: 行数可能 > 7 家, 表格比图更合适; 请求数格内加占比细条给一眼的量级感。 */
export function SubscriptionTable({
  items,
  loading,
  errorText,
}: {
  items: BreakdownDto[];
  loading?: boolean;
  errorText?: string | null;
}) {
  const { t } = useT();
  const navigate = useNavigate();
  const subs = useSubscriptions();
  const providers = useProviders();

  // 一次构建 id → 详情索引, 避免行渲染时 N×M 的 array.find
  const subById = useMemo(
    () => new Map<string, SubscriptionDto>((subs.data ?? []).map((s) => [s.id, s])),
    [subs.data],
  );
  const providerById = useMemo(
    () => new Map<string, ProviderInfo>((providers.data ?? []).map((p) => [p.id, p])),
    [providers.data],
  );
  const maxCount = Math.max(...items.map((it) => it.request_count), 1);

  return (
    <StatsCard
      title={t("stats.bySub.title")}
      subtitle={t("stats.bySub.subtitle")}
      isEmpty={items.length === 0}
      emptyText={t("stats.bySub.empty")}
      loading={loading}
      errorText={errorText}
    >
      <table className="table stats-sub-table">
        <thead>
          <tr>
            <th>{t("stats.bySub.col.subscription")}</th>
            <th style={{ width: 170, textAlign: "right" }}>{t("stats.bySub.col.requests")}</th>
            <th style={{ width: 100, textAlign: "right" }}>{t("stats.bySub.col.successRate")}</th>
            <th style={{ width: 100, textAlign: "right" }}>{t("stats.bySub.col.avgDuration")}</th>
            <th style={{ width: 140, textAlign: "right" }}>{t("stats.bySub.col.tokensTotal")}</th>
          </tr>
        </thead>
        <tbody>
          {items.map((it) => {
            const sub = subById.get(it.key);
            const provider = sub ? providerById.get(sub.provider_id) : undefined;
            const successPct = it.request_count > 0 ? (it.success_count / it.request_count) * 100 : 0;
            const share = (it.request_count / maxCount) * 100;
            return (
              <tr
                key={it.key}
                onClick={() => navigate(`/request-logs?subscription_id=${it.key}`)}
                style={{ cursor: "pointer" }}
              >
                <td>
                  <div className="cell-with-icon">
                    <ProviderLogo iconId={provider?.icon} size={18} iconSize={12} />
                    <span className="cell-with-icon-label">{it.label}</span>
                  </div>
                </td>
                <td style={{ textAlign: "right" }}>
                  <div className="share-cell">
                    <span className="share-bar" aria-hidden="true">
                      <span className="share-bar-fill" style={{ width: `${share}%` }} />
                    </span>
                    <span className="mono tnum strong">{fmtNum(it.request_count)}</span>
                  </div>
                </td>
                <td className="mono tnum" style={{ textAlign: "right" }}>
                  {successPct.toFixed(1)}%
                </td>
                <td className="mono tnum" style={{ textAlign: "right" }}>
                  {it.avg_duration_ms != null ? `${(it.avg_duration_ms / 1000).toFixed(2)}s` : "-"}
                </td>
                <td className="mono tnum" style={{ textAlign: "right" }}>
                  {fmtCompact(it.total_input_tokens + it.total_output_tokens)}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </StatsCard>
  );
}
