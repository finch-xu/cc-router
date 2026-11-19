import { LiveSessionSection } from "@/components/LiveSessionSection";
import { ClientAccessSection } from "@/components/ClientAccessSection";
import { RouteFlowDiagram } from "@/components/RouteFlowDiagram";
import { RouteFlowDiagramClassic } from "@/components/RouteFlowDiagramClassic";
import { useTheme } from "@/hooks/useTheme";
import { useT } from "@/i18n";
import { isPlainBased } from "@/themes";

export function LiveRoutingPage() {
  const { art } = useTheme();
  const { t } = useT();
  if (isPlainBased(art)) {
    return (
      <div className="page-flow">
        <RouteFlowDiagramClassic />
        <LiveSessionSection variant="classic" />
        <ClientAccessSection variant="classic" />
      </div>
    );
  }

  return (
    <div className="lr-page">
      <div className="page-actions">
        <div className="page-header" style={{ marginBottom: 0 }}>
          <h1>{t("liveRouting.title")}</h1>
          <div className="subtitle">{t("liveRouting.subtitle")}</div>
        </div>
        {/* 手写批注: 指向下方路由图里流动的连线 */}
        <div className="hand-note" aria-hidden="true">
          <span>{t("liveRouting.handNote")}</span>
          <svg viewBox="0 0 40 40" width="32" height="32">
            <path d="M6 6 C 22 6, 32 14, 30 32 M23 26 L30 33 L36 25" />
          </svg>
        </div>
      </div>
      <RouteFlowDiagram />
      <LiveSessionSection variant="sketch" />
      <ClientAccessSection variant="sketch" />
    </div>
  );
}
