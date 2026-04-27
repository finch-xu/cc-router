import { Navigate, Route, Routes } from "react-router-dom";
import { AppShell } from "@/components/layout/AppShell";
import { VirtualModelsPage } from "@/routes/VirtualModels";
import { SubscriptionsPage } from "@/routes/Subscriptions";
import { SubscriptionNewPage } from "@/routes/SubscriptionNew";
import { SubscriptionEditPage } from "@/routes/SubscriptionEdit";
import { SettingsPage } from "@/routes/Settings";
import { RequestLogsPage } from "@/routes/RequestLogs";
import { AboutPage } from "@/routes/About";
import { GuidePage } from "@/routes/Guide";
import { OnboardingDisclaimerPage } from "@/routes/OnboardingDisclaimer";
import { OnboardingGate } from "@/components/layout/OnboardingGate";
import { useSubscriptionEventBridge } from "@/hooks/useSubscriptions";
import { useRouteFlashListener } from "@/hooks/useRouteFlash";
import { UpdaterProvider, useUpdaterAutoCheck } from "@/hooks/useUpdater";

export default function App() {
  return (
    <UpdaterProvider>
      <AppInner />
    </UpdaterProvider>
  );
}

function AppInner() {
  useSubscriptionEventBridge();
  useRouteFlashListener();
  useUpdaterAutoCheck();
  return (
    <Routes>
      {/* 免责声明门禁: 全屏顶层路由, 不进 OnboardingGate / AppShell */}
      <Route path="/onboarding/disclaimer" element={<OnboardingDisclaimerPage />} />
      {/* 兼容旧链接: 引导壳已删, 旧 /onboarding 转到订阅向导 */}
      <Route
        path="/onboarding"
        element={<Navigate to="/subscriptions/new?onboarding=1" replace />}
      />
      <Route
        element={
          <OnboardingGate>
            <AppShell />
          </OnboardingGate>
        }
      >
        <Route index element={<Navigate to="/virtual-models" replace />} />
        <Route path="/virtual-models" element={<VirtualModelsPage />} />
        <Route path="/subscriptions" element={<SubscriptionsPage />} />
        <Route path="/subscriptions/new" element={<SubscriptionNewPage />} />
        <Route path="/subscriptions/:id" element={<SubscriptionEditPage />} />
        <Route path="/request-logs" element={<RequestLogsPage />} />
        <Route path="/guide" element={<GuidePage />} />
        <Route path="/settings" element={<SettingsPage />} />
        <Route path="/about" element={<AboutPage />} />
      </Route>
      <Route path="*" element={<Navigate to="/" replace />} />
    </Routes>
  );
}
