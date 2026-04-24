import { ReactNode } from "react";
import { Navigate, useLocation } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "@/api/tauri";

// 这些路径在 onboarding 过程中仍允许访问（第一步需要跳过去添加订阅）。
const ALLOWED_DURING_ONBOARDING = ["/subscriptions"];

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
    return <Navigate to="/onboarding" replace />;
  }

  return <>{children}</>;
}
