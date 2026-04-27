import { ReactNode } from "react";
import { Navigate, useLocation } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "@/api/tauri";
import { useT } from "@/i18n";

// onboarding 期间只允许访问订阅添加页(承担引导职责)。
// 收紧到精确前缀, 避免用户从 /subscriptions 列表逃出去触发"来回跳"。
const ALLOWED_DURING_ONBOARDING = ["/subscriptions/new"];

export function OnboardingGate({ children }: { children: ReactNode }) {
  const { t } = useT();
  const location = useLocation();
  const { data, isLoading } = useQuery({
    queryKey: ["onboarding-state"],
    queryFn: () => api.getOnboardingState(),
    staleTime: Infinity,
  });

  if (isLoading) {
    return (
      <div className="flex h-screen items-center justify-center text-sm text-muted-foreground">
        {t("common.loading")}
      </div>
    );
  }

  const allowed = ALLOWED_DURING_ONBOARDING.some((prefix) =>
    location.pathname.startsWith(prefix),
  );

  if (!data?.completed && !allowed) {
    const accepted =
      typeof window !== "undefined" &&
      window.localStorage.getItem("cc-router.disclaimer-accepted") === "1";
    return (
      <Navigate
        to={accepted ? "/subscriptions/new?onboarding=1" : "/onboarding/disclaimer"}
        replace
      />
    );
  }

  return <>{children}</>;
}
