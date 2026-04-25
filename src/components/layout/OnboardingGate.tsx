import { ReactNode } from "react";
import { Navigate, useLocation } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "@/api/tauri";

// onboarding 期间只允许访问订阅添加页(承担引导职责)。
// 收紧到精确前缀, 避免用户从 /subscriptions 列表逃出去触发"来回跳"。
const ALLOWED_DURING_ONBOARDING = ["/subscriptions/new"];

export function OnboardingGate({ children }: { children: ReactNode }) {
  const location = useLocation();
  const { data, isLoading } = useQuery({
    queryKey: ["onboarding-state"],
    queryFn: () => api.getOnboardingState(),
    staleTime: Infinity,
  });

  if (isLoading) {
    return (
      <div className="flex h-screen items-center justify-center text-sm text-muted-foreground">
        加载中…
      </div>
    );
  }

  const allowed = ALLOWED_DURING_ONBOARDING.some((prefix) =>
    location.pathname.startsWith(prefix),
  );

  if (!data?.completed && !allowed) {
    return <Navigate to="/subscriptions/new?onboarding=1" replace />;
  }

  return <>{children}</>;
}
