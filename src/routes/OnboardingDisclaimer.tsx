import { useNavigate } from "react-router-dom";
import { AlertTriangle } from "lucide-react";
import logoUrl from "@/assets/logo.png";
import { useT } from "@/i18n";

const DISCLAIMER_FLAG_KEY = "cc-router.disclaimer-accepted";

export function OnboardingDisclaimerPage() {
  const { t } = useT();
  const navigate = useNavigate();

  function handleAccept() {
    try {
      localStorage.setItem(DISCLAIMER_FLAG_KEY, "1");
    } catch {
      // localStorage 写失败时仍允许继续, 下次启动会再被拦一次, 可接受
    }
    navigate("/subscriptions/new?onboarding=1", { replace: true });
  }

  return (
    <div className="onboarding-disclaimer-shell">
      <div className="card onboarding-disclaimer">
        <div className="onboarding-disclaimer-mark">
          <img src={logoUrl} alt="cc-router" />
        </div>
        <h1 className="onboarding-disclaimer-title">
          <AlertTriangle size={18} />
          {t("onboarding.disclaimer.title")}
        </h1>
        <div className="onboarding-disclaimer-subtitle">
          {t("onboarding.disclaimer.subtitle")}
        </div>
        <div className="onboarding-disclaimer-body">
          <p>{t("about.disclaimer.usage")}</p>
          <p>{t("about.disclaimer.tos")}</p>
          <p>{t("about.disclaimer.warranty")}</p>
        </div>
        <button
          className="btn primary onboarding-disclaimer-accept"
          type="button"
          onClick={handleAccept}
        >
          {t("onboarding.disclaimer.accept")}
        </button>
      </div>
    </div>
  );
}
