import { Navigate, Route, Routes } from "react-router-dom";
import { AppShell } from "@/components/layout/AppShell";
import { VirtualModelsPage } from "@/routes/VirtualModels";
import { SubscriptionsPage } from "@/routes/Subscriptions";
import { SubscriptionNewPage } from "@/routes/SubscriptionNew";
import { SubscriptionEditPage } from "@/routes/SubscriptionEdit";
import { SettingsPage } from "@/routes/Settings";
import { RequestLogsPage } from "@/routes/RequestLogs";
import { AboutPage } from "@/routes/About";
import { OnboardingPage } from "@/routes/Onboarding";
import { OnboardingGate } from "@/components/layout/OnboardingGate";

export default function App() {
  return (
    <Routes>
      <Route path="/onboarding" element={<OnboardingPage />} />
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
        <Route path="/settings" element={<SettingsPage />} />
        <Route path="/about" element={<AboutPage />} />
      </Route>
      <Route path="*" element={<Navigate to="/" replace />} />
    </Routes>
  );
}
