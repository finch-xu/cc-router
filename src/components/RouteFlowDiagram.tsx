import { Fragment, useMemo } from "react";
// 直接导入 Color leaf 组件,绕过默认 barrel(否则会拖入 Avatar/Mono/Combine/Text 共 ~20KB)
import ClaudeCodeColor from "@lobehub/icons/es/ClaudeCode/components/Color";
import { ProviderLogo } from "@/components/ProviderLogo";
import { useProxyStatus } from "@/hooks/useSettings";
import { useVirtualModels } from "@/hooks/useVirtualModels";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useProviders } from "@/hooks/useProviders";
import { MODE_LABEL_KEY, VM_ORDER } from "@/lib/virtualModels";
import { useT } from "@/i18n";
import logoUrl from "@/assets/logo.png";
import type { SubscriptionDto, VirtualModelDto } from "@/types";

export function RouteFlowDiagram() {
  const { t } = useT();
  const proxy = useProxyStatus();
  const vms = useVirtualModels();
  const subs = useSubscriptions();
  const providers = useProviders();

  const subsMap = useMemo(() => {
    const m = new Map<string, SubscriptionDto>();
    subs.data?.forEach((s) => m.set(s.id, s));
    return m;
  }, [subs.data]);

  const orderedVms = useMemo<VirtualModelDto[]>(
    () =>
      VM_ORDER.map((name) => vms.data?.find((v) => v.name === name)).filter(
        (v): v is VirtualModelDto => v !== undefined,
      ),
    [vms.data],
  );

  const providersInUse = useMemo(() => {
    const result: { id: string; healthy: boolean }[] = [];
    const seen = new Set<string>();
    for (const vm of orderedVms) {
      for (const sid of vm.subscription_ids) {
        const sub = subsMap.get(sid);
        if (!sub || seen.has(sub.provider_id)) continue;
        seen.add(sub.provider_id);
        result.push({ id: sub.provider_id, healthy: sub.state === "healthy" });
      }
    }
    return result;
  }, [orderedVms, subsMap]);

  const providerOf = (id: string) => providers.data?.find((p) => p.id === id);

  const running = proxy.data?.running ?? false;
  const port = proxy.data?.port ?? "?";

  return (
    <div className="card diagram section">
      <div className="diagram-inner">
        <div className="card-head" style={{ padding: "0 0 18px" }}>
          <div className="card-title">
            <span>{t("routeFlow.title")}</span>
            <span className="pill accent">
              <span className="dot" />
              {running ? t("routeFlow.live") : t("routeFlow.offline")}
            </span>
          </div>
          <div className="card-sub mono">
            v1/messages → {orderedVms.length} slots → {providersInUse.length} vendors
          </div>
        </div>

        <div className="routemap">
          <div className="node">
            <div className="node-label">CLIENT</div>
            <div className="node-title">
              <ClaudeCodeColor size={20} />
              Claude Code
            </div>
            <div className="node-meta">POST /v1/messages</div>
            <div className="node-list">
              <div className="row mono">
                <span className="dot" style={{ background: "var(--ink-4)" }} />
                opus / sonnet / haiku
              </div>
              <div className="row mono">
                <span className="dot" style={{ background: "var(--ink-4)" }} />
                X-API-Key: ••••
              </div>
            </div>
          </div>

          <div className="routemap-mid">
            <div className="routemap-mid-head">
              <span className={"live" + (running ? "" : " off")} />
              <img src={logoUrl} alt="" />
              <span style={{ color: "var(--ink)", fontWeight: 600 }}>cc-router</span>
              <span>127.0.0.1:{port}</span>
              <span style={{ marginLeft: "auto" }}>
                {orderedVms.length} routes · {running ? "running" : "stopped"}
              </span>
            </div>
            {orderedVms.length === 0 ? (
              <div className="route-row">
                <div className="chain-empty">{t("common.loading")}</div>
              </div>
            ) : (
              orderedVms.map((vm) => (
                <div className="route-row" key={vm.name}>
                  <div className="route-slot">{vm.name}</div>
                  <div className="route-mode">{t(MODE_LABEL_KEY[vm.mode])}</div>
                  <div className="route-chain">
                    {vm.subscription_ids.length === 0 ? (
                      <span className="chain-empty">{t("routeFlow.notBound")}</span>
                    ) : (
                      vm.subscription_ids.map((sid, i) => {
                        const sub = subsMap.get(sid);
                        const dotColor = i === 0 ? "var(--accent)" : "var(--ink-4)";
                        return (
                          <Fragment key={sid}>
                            <span className="chain-chip">
                              <span className="dot" style={{ background: dotColor }} />
                              {sub?.display_name ?? "?"}
                            </span>
                            {i < vm.subscription_ids.length - 1 && (
                              <span className="chain-arrow">
                                {vm.mode === "round_robin" ? "↻" : "→"}
                              </span>
                            )}
                          </Fragment>
                        );
                      })
                    )}
                  </div>
                </div>
              ))
            )}
          </div>

          <div className="node">
            <div className="node-label">UPSTREAMS</div>
            <div className="node-title">{t("routeFlow.providersTitle")}</div>
            <div className="node-list">
              {providersInUse.length === 0 ? (
                <div className="chain-empty">{t("routeFlow.providersEmpty")}</div>
              ) : (
                providersInUse.map((p) => {
                  const info = providerOf(p.id);
                  return (
                    <div className="row" key={p.id}>
                      <span
                        className="dot"
                        style={{ background: p.healthy ? "var(--ok)" : "var(--err)" }}
                      />
                      <ProviderLogo iconId={info?.icon} size={18} iconSize={12} />
                      <span style={{ fontSize: 12 }}>{info?.display_name ?? p.id}</span>
                    </div>
                  );
                })
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
