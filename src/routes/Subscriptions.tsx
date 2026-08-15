import { Link } from "react-router-dom";
import { Plus, Key } from "lucide-react";
import { StatusBadge } from "@/components/StatusBadge";
import { ProviderLogo } from "@/components/ProviderLogo";
import { EmptyState } from "@/components/EmptyState";
import { BalanceBadge } from "@/components/SubscriptionBalanceCard";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useT } from "@/i18n";
import { fmtCompact, fmtTimeShort } from "@/lib/format";
import { formatTokenShorthand } from "@/lib/quota";
import { QUOTA_SEGMENTS } from "@/components/SubscriptionQuotaCard";
import type { SubscriptionDto } from "@/types";

export function SubscriptionsPage() {
  const { t } = useT();
  const subs = useSubscriptions();

  return (
    <>
      <div className="page-actions">
        <div className="page-header" style={{ margin: 0 }}>
          <h1>{t("subscriptions.title")}</h1>
          <div className="subtitle">{t("subscriptions.subtitle")}</div>
        </div>
        <Link className="btn primary" to="/subscriptions/new">
          <Plus size={12} /> {t("subscriptions.add")}
        </Link>
      </div>

      {subs.isLoading && <div className="field-hint">{t("common.loading")}</div>}

      {subs.data && subs.data.length === 0 && (
        <EmptyState
          icon={Key}
          message={t("subscriptions.empty.message")}
          action={
            <Link className="btn primary sm" to="/subscriptions/new">
              <Plus size={12} /> {t("subscriptions.empty.action")}
            </Link>
          }
        />
      )}

      {subs.data && subs.data.length > 0 && (
        <div className="card">
          <table className="table">
            <thead>
              <tr>
                <th style={{ width: 100 }}>{t("subscriptions.col.status")}</th>
                <th>{t("subscriptions.col.provider")}</th>
                <th style={{ width: 220 }}>{t("subscriptions.col.quota")}</th>
                <th style={{ width: 90 }}>{t("subscriptions.col.referenced")}</th>
                <th style={{ width: 100 }}>{t("subscriptions.col.updatedAt")}</th>
                <th style={{ width: 80 }}></th>
              </tr>
            </thead>
            <tbody>
              {subs.data.map((sub) => {
                return (
                  <tr key={sub.id}>
                    <td>
                      <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                        <StatusBadge state={sub.state} />
                        {sub.quota_usage.some((q) => q.exceeded) && (
                          <span className="pill warn">{t("quota.exceeded")}</span>
                        )}
                      </div>
                    </td>
                    <td>
                      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
                        <ProviderLogo iconId={sub.provider_icon} size={24} />
                        <div style={{ display: "flex", flexDirection: "column", gap: 2, minWidth: 0 }}>
                          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                            <span style={{ fontWeight: 500, color: "var(--ink)" }}>
                              {sub.provider_display_name}
                            </span>
                            {sub.is_user_defined && (
                              <span
                                style={{
                                  fontSize: 10,
                                  padding: "1px 6px",
                                  borderRadius: 4,
                                  background: "var(--bg-muted, #f0f0f0)",
                                  color: "var(--ink-3)",
                                }}
                              >
                                🔧 {t("subscriptions.custom")}
                              </span>
                            )}
                            <BalanceBadge subscription={sub} />
                          </div>
                          <span style={{ fontSize: 12, color: "var(--ink-3)" }}>{sub.display_name}</span>
                        </div>
                      </div>
                    </td>
                    <td>
                      <QuotaCell subscription={sub} />
                    </td>
                    <td>
                      {sub.referenced_by.length > 0 ? (
                        <span className="pill tag mono">used: {sub.referenced_by.length}</span>
                      ) : (
                        <span className="field-hint" style={{ marginTop: 0, fontSize: 11.5 }}>
                          {t("subscriptions.notUsed")}
                        </span>
                      )}
                    </td>
                    <td className="mono" style={{ color: "var(--ink-3)", fontSize: 12 }}>
                      {fmtTimeShort(sub.updated_at)}
                    </td>
                    <td>
                      <Link className="btn sm" to={`/subscriptions/${sub.id}`}>
                        {t("subscriptions.view")}
                      </Link>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </>
  );
}

/** 列表用的紧凑限额展示: 每条已设限额一行「周期 · 已用/上限」+ 细四段条; 未设限额显示灰字。 */
function QuotaCell({ subscription }: { subscription: SubscriptionDto }) {
  const { t } = useT();
  const rows = subscription.quota_usage.filter((q) => q.limit != null);
  if (rows.length === 0) {
    return (
      <span className="field-hint" style={{ marginTop: 0, fontSize: 11.5 }}>
        {t("subscriptions.quotaNone")}
      </span>
    );
  }
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
      {rows.map((q) => {
        const limit = q.limit!;
        const used = q.input + q.output + q.cache_creation + q.cache_read;
        return (
          <div key={q.period} style={{ display: "flex", flexDirection: "column", gap: 2 }}>
            <div
              className="mono"
              style={{
                display: "flex",
                justifyContent: "space-between",
                gap: 8,
                fontSize: 11.5,
                color: q.exceeded ? "var(--destructive, #dc2626)" : "var(--ink-3)",
              }}
            >
              <span>{t(`quota.period.${q.period}`)}</span>
              <span>
                {fmtCompact(used)} / {formatTokenShorthand(limit)}
              </span>
            </div>
            <div
              style={{
                display: "flex",
                height: 4,
                width: "100%",
                overflow: "hidden",
                borderRadius: 2,
                background: "var(--bg-muted, #f0f0f0)",
              }}
            >
              {QUOTA_SEGMENTS.map((seg) => (
                <div
                  key={seg.key}
                  style={{
                    height: "100%",
                    flexShrink: 0,
                    width: `${(Math.min(q[seg.key], limit) / limit) * 100}%`,
                    background: seg.color,
                  }}
                />
              ))}
            </div>
          </div>
        );
      })}
    </div>
  );
}
