import { Link } from "react-router-dom";
import { Plus, Key } from "lucide-react";
import { StatusBadge } from "@/components/StatusBadge";
import { ProviderLogo } from "@/components/ProviderLogo";
import { EmptyState } from "@/components/EmptyState";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useT } from "@/i18n";
import { fmtTimeShort } from "@/lib/format";

// SubscriptionDto 不带 api_key 预览(CLAUDE.md), 用固定遮罩占位
const MASKED_KEY = "•••••••••••••••";

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
                <th>{t("subscriptions.col.note")}</th>
                <th style={{ width: 160 }}>API Key</th>
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
                      <StatusBadge state={sub.state} />
                    </td>
                    <td>
                      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
                        <ProviderLogo iconId={sub.provider_icon} size={24} />
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
                      </div>
                    </td>
                    <td>{sub.display_name}</td>
                    <td className="mono" style={{ color: "var(--ink-3)", fontSize: 12 }}>
                      {MASKED_KEY}
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
