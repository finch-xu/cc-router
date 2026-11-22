import { Link } from "react-router";
import { useT } from "@/i18n";
import { fmtNum } from "@/lib/format";
import { equivalentParts, spendParts } from "@/lib/cost";
import { customProviderLabel } from "@/lib/providerLabels";
import { localized, type LocalizedText } from "@/plugins/manifest";
import { useInstalledPlugins } from "@/hooks/usePlugins";
import type { CostSummaryDto } from "@/types";

/**
 * 费用卡片 (计价插件, 设计 §5)。没有启用的计价插件时只是一行引导; 有了才显示金额。
 * 金额按**当前**价格表现算, 两种币种分开、真实花费与包月订阅的「等价按量费用」分开。
 */
export function CostCard({ data, loading, errorText }: { data: CostSummaryDto | undefined; loading?: boolean; errorText?: string | null }) {
  const { t, locale } = useT();
  const installed = useInstalledPlugins();
  if (!data) return null;

  if (!data.has_pricing) {
    return (
      <section className="card sx-cost-hint">
        <div className="card-body">
          <span>{t("stats.cost.noPricing")}</span>
          <Link className="btn sm" to="/plugins">
            {t("stats.cost.getPricing")}
          </Link>
        </div>
      </section>
    );
  }

  const nameOf = (id: string) => {
    const m = installed.data?.find((p) => p.id === id)?.manifest as { name?: LocalizedText } | undefined;
    return localized(m?.name, locale) || id;
  };
  const spend = spendParts(data.totals);
  const equiv = equivalentParts(data.totals);
  const unpricedShown = data.unpriced.slice(0, 5);
  return (
    <section className={"card" + (loading ? " is-loading" : "")}>
      <div className="card-head">
        <div className="sx-head-lead">
          <span className="card-title">{t("stats.cost.title")}</span>
          <span className="card-sub">{t("stats.cost.sub")}</span>
        </div>
      </div>
      <div className="card-body">
        {errorText ? (
          <div className="sx-error">{errorText}</div>
        ) : (
          <>
            <div className="sx-cost-top">
              <div className="sx-kpi">
                <div className="l">{t("stats.cost.spend")}</div>
                <div className="v">{spend.length ? spend.join(" + ") : "—"}</div>
              </div>
              {equiv.length > 0 && (
                <div className="sx-kpi">
                  <div className="l">{t("stats.cost.equivalent")}</div>
                  <div className="v">≈ {equiv.join(" + ")}</div>
                  <div className="s">{t("stats.cost.equivalentHint")}</div>
                </div>
              )}
              <div className="sx-kpi">
                <div className="l">{t("stats.cost.coverage")}</div>
                <div className="v">
                  {fmtNum(data.priced_requests)}
                  <small>/ {fmtNum(data.total_requests)}</small>
                </div>
              </div>
            </div>

            {data.by_subscription.length > 0 && (
              <div className="sx-rows">
                {data.by_subscription.slice(0, 8).map((s) => {
                  const parts = spendParts(s.totals);
                  const eq = equivalentParts(s.totals);
                  return (
                    <div key={s.subscription_id} className="sx-row" style={{ gridTemplateColumns: "minmax(0, 1fr) auto" }}>
                      <span className="n">
                        <span>{s.display_name ?? t("receipts.slip.deletedSub")}</span>
                      </span>
                      <span className="v">
                        {parts.join(" + ")}
                        {eq.length > 0 && <span className="sx-cost-eq">{`${parts.length ? " · " : ""}≈ ${eq.join(" + ")}`}</span>}
                      </span>
                    </div>
                  );
                })}
              </div>
            )}

            {unpricedShown.length > 0 && (
              <div className="sx-cost-unpriced">
                <div className="l">{t("stats.cost.unpriced", { count: data.unpriced.length })}</div>
                {unpricedShown.map((u) => (
                  <div key={`${u.provider_id}/${u.model}`} className="mono">
                    {customProviderLabel(u.provider_id, t) ?? u.provider_id} / {u.model} · {t("stats.cost.requests", { count: fmtNum(u.requests) })}
                  </div>
                ))}
              </div>
            )}

            <div className="sx-cost-source">
              {t("stats.cost.source", { names: data.sources.map((s) => `${nameOf(s.plugin_id)} ${s.version}`).join(", ") })}
            </div>
          </>
        )}
      </div>
    </section>
  );
}
