import { useEffect, useState, type FormEvent, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { AUTH_REQUIRED_EVENT, runtime, webLogin, webSession } from "@/runtime";
import { useT } from "@/i18n";
import logoUrl from "@/assets/logo.png";

type Phase = "checking" | "login" | "ready";

/**
 * 仅 web 模式挂载. 启动查 /ui/api/session: 已登录 (或免登录模式) 直接进主界面,
 * 否则显示登录页; 任何 API 返回 401 时 web runtime 派发 AUTH_REQUIRED_EVENT, 这里切回登录页.
 */
export function WebAuthGate({ children }: { children: ReactNode }) {
  const [phase, setPhase] = useState<Phase>(() =>
    runtime.kind === "web" ? "checking" : "ready",
  );
  const queryClient = useQueryClient();

  useEffect(() => {
    if (runtime.kind !== "web") {
      setPhase("ready");
      return;
    }
    let cancelled = false;
    void webSession()
      .then((s) => {
        if (!cancelled) setPhase(s.authenticated ? "ready" : "login");
      })
      .catch(() => {
        if (!cancelled) setPhase("login");
      });
    const onAuthRequired = () => {
      queryClient.clear();
      setPhase("login");
    };
    window.addEventListener(AUTH_REQUIRED_EVENT, onAuthRequired);
    return () => {
      cancelled = true;
      window.removeEventListener(AUTH_REQUIRED_EVENT, onAuthRequired);
    };
  }, [queryClient]);

  if (phase === "ready") return <>{children}</>;
  if (phase === "checking") return <CenteredNote />;
  return <LoginPage onSuccess={() => setPhase("ready")} />;
}

function CenteredNote() {
  const { t } = useT();
  return <div className="web-login-shell">{t("webAuth.checking")}</div>;
}

function LoginPage({ onSuccess }: { onSuccess: () => void }) {
  const { t } = useT();
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!token.trim() || busy) return;
    setBusy(true);
    setError(null);
    let res: Awaited<ReturnType<typeof webLogin>>;
    try {
      res = await webLogin(token.trim());
    } catch {
      res = { ok: false };
    }
    setBusy(false);
    if (res.ok) {
      onSuccess();
      return;
    }
    setError(res.message?.includes("too many") ? t("webAuth.locked") : t("webAuth.failed"));
  }

  return (
    <div className="web-login-shell">
      <form className="web-login-card" onSubmit={submit}>
        <img src={logoUrl} alt="cc-router" className="web-login-logo" />
        <h1>{t("webAuth.title")}</h1>
        <p className="desc">{t("webAuth.desc")}</p>
        <input
          className="input mono"
          type="password"
          autoComplete="current-password"
          placeholder={t("webAuth.tokenPlaceholder")}
          value={token}
          onChange={(e) => setToken(e.target.value)}
          autoFocus
        />
        {error && <div className="web-login-error">{error}</div>}
        <button className="btn primary" type="submit" disabled={busy || !token.trim()}>
          {busy ? t("webAuth.submitting") : t("webAuth.submit")}
        </button>
      </form>
    </div>
  );
}
