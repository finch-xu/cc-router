import { Navigate, Route, Routes } from "react-router";
import { AgentMonitorPage } from "@/routes/AgentMonitor";
import { AppShell } from "@/components/layout/AppShell";
import { VirtualModelsPage } from "@/routes/VirtualModels";
import { LiveRoutingPage } from "@/routes/LiveRouting";
import { UpdatesPage } from "@/routes/Updates";
import { SubscriptionsPage } from "@/routes/Subscriptions";
import { SubscriptionNewPage } from "@/routes/SubscriptionNew";
import { SubscriptionEditPage } from "@/routes/SubscriptionEdit";
import { SettingsPage } from "@/routes/Settings";
import { LogsPage } from "@/routes/Logs";
import { RequestDetailPage } from "@/routes/RequestDetail";
import { StatisticsPage } from "@/routes/Statistics";
import { ReceiptsPage } from "@/routes/Receipts";
import { PluginsPage } from "@/routes/Plugins";
import { AboutPage } from "@/routes/About";
import { GuidePage } from "@/routes/Guide";
import { useSubscriptionEventBridge } from "@/hooks/useSubscriptions";
import { useRouteFlashListener } from "@/hooks/useRouteFlash";
import { useSettingsEventBridge, useUpdateSourceAutoInit } from "@/hooks/useSettings";
import { useTrayNavigation } from "@/hooks/useTrayNavigation";
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
  useSettingsEventBridge();
  useTrayNavigation();
  useRouteFlashListener();
  useUpdateSourceAutoInit();
  useUpdaterAutoCheck();
  return (
    <Routes>
      {/* 兼容旧链接: 引导改成主界面上的窗口 (OnboardingHost), 旧的全屏页路由一律回首页 */}
      <Route path="/onboarding/*" element={<Navigate to="/" replace />} />
      <Route element={<AppShell />}>
        <Route index element={<Navigate to="/virtual-models" replace />} />
        <Route path="/live-routing" element={<LiveRoutingPage />} />
        <Route path="/agent-monitor" element={<AgentMonitorPage />} />
        <Route path="/virtual-models" element={<VirtualModelsPage />} />
        <Route path="/subscriptions" element={<SubscriptionsPage />} />
        <Route path="/subscriptions/new" element={<SubscriptionNewPage />} />
        <Route path="/subscriptions/:id" element={<SubscriptionEditPage />} />
        <Route path="/request-logs" element={<LogsPage />} />
        <Route path="/request-logs/:id" element={<RequestDetailPage />} />
        <Route path="/statistics" element={<StatisticsPage />} />
        <Route path="/receipts" element={<ReceiptsPage />} />
        <Route path="/plugins" element={<PluginsPage />} />
        <Route path="/guide" element={<GuidePage />} />
        <Route path="/updates" element={<UpdatesPage />} />
        <Route path="/settings" element={<SettingsPage />} />
        <Route path="/about" element={<AboutPage />} />
      </Route>
      <Route path="*" element={<Navigate to="/" replace />} />
    </Routes>
  );
}
