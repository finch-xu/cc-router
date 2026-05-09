import { useEffect, useRef, useState } from "react";
import { ExternalLink, Loader2, Copy, Check, X } from "lucide-react";
import { open as openShell } from "@tauri-apps/plugin-shell";
import { Spinner } from "@/components/Spinner";
import { useT } from "@/i18n";
import { api } from "@/api/tauri";
import type { ChatGptAccount } from "@/types";

interface Props {
  open: boolean;
  onClose: () => void;
  /** 授权成功时回传 device_code + account, 父组件保留 device_code 在「保存订阅」时用. */
  onSuccess: (deviceCode: string, account: ChatGptAccount) => void;
}

const POLL_INTERVAL_MS = 2500;

export function ChatGptOAuthDialog({ open, onClose, onSuccess }: Props) {
  const { t } = useT();
  const [phase, setPhase] = useState<"idle" | "starting" | "waiting" | "completed" | "error">("idle");
  const [deviceCode, setDeviceCode] = useState<string>("");
  const [userCode, setUserCode] = useState<string>("");
  const [verifyUrl, setVerifyUrl] = useState<string>("");
  const [errorMsg, setErrorMsg] = useState<string>("");
  const [copied, setCopied] = useState(false);
  const pollTimer = useRef<number | null>(null);

  // 启动 Device Code 流程: open=true 时跑一次, 关闭时清状态.
  useEffect(() => {
    if (!open) {
      setPhase("idle");
      setDeviceCode("");
      setUserCode("");
      setVerifyUrl("");
      setErrorMsg("");
      setCopied(false);
      if (pollTimer.current !== null) {
        window.clearInterval(pollTimer.current);
        pollTimer.current = null;
      }
      return;
    }

    setPhase("starting");
    api
      .startChatGptDeviceFlow()
      .then((res) => {
        setDeviceCode(res.device_code);
        setUserCode(res.user_code);
        setVerifyUrl(res.verification_uri);
        setPhase("waiting");
        // 启动后立即开浏览器
        openShell(res.verification_uri).catch(() => {});
      })
      .catch((e) => {
        setErrorMsg(String(e));
        setPhase("error");
      });
  }, [open]);

  // 轮询: phase=waiting 时启动 setInterval
  useEffect(() => {
    if (phase !== "waiting" || !deviceCode) return;

    let stopped = false;
    const poll = async () => {
      try {
        const account = await api.pollChatGptDeviceCode(deviceCode);
        if (stopped) return;
        if (account) {
          setPhase("completed");
          onSuccess(deviceCode, account);
        }
      } catch (e) {
        if (stopped) return;
        setErrorMsg(String(e));
        setPhase("error");
      }
    };
    // 2.5s 后开始第一次, 之后每 2.5s 一次. 立即跑一次会过快, 还没人去授权.
    const id = window.setInterval(poll, POLL_INTERVAL_MS);
    pollTimer.current = id;

    return () => {
      stopped = true;
      window.clearInterval(id);
    };
  }, [phase, deviceCode, onSuccess]);

  if (!open) return null;

  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        background: "rgba(0,0,0,0.4)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        zIndex: 1000,
      }}
      onClick={onClose}
    >
      <div
        className="card"
        style={{ width: 460, maxWidth: "90vw", padding: 24 }}
        onClick={(e) => e.stopPropagation()}
      >
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start", marginBottom: 16 }}>
          <div>
            <h3 style={{ margin: 0, fontSize: 16 }}>{t("oauth.chatgpt.title")}</h3>
            <div style={{ fontSize: 12, color: "var(--ink-3)", marginTop: 4 }}>
              {t("oauth.chatgpt.subtitle")}
            </div>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="btn bare sm"
            style={{ padding: 4 }}
            aria-label={t("common.close")}
          >
            <X size={14} />
          </button>
        </div>

        <div
          style={{
            padding: "10px 12px",
            marginBottom: 16,
            borderLeft: "3px solid var(--warn, #d97706)",
            background: "var(--warn-bg, rgba(217, 119, 6, 0.08))",
            fontSize: 11.5,
            lineHeight: 1.55,
            color: "var(--ink-2)",
            borderRadius: "0 4px 4px 0",
          }}
        >
          {t("oauth.chatgpt.tosWarning")}
        </div>

        {phase === "starting" && (
          <div style={{ display: "flex", alignItems: "center", gap: 8, color: "var(--ink-3)" }}>
            <Spinner /> {t("oauth.chatgpt.starting")}
          </div>
        )}

        {phase === "waiting" && (
          <>
            <div style={{ marginBottom: 16, color: "var(--ink-3)", fontSize: 13 }}>
              {t("oauth.chatgpt.instructions")}
            </div>

            <div
              style={{
                background: "var(--surface-2)",
                border: "1px solid var(--line)",
                borderRadius: 6,
                padding: 16,
                textAlign: "center",
                marginBottom: 16,
              }}
            >
              <div style={{ fontSize: 11, color: "var(--ink-3)", marginBottom: 6 }}>
                {t("oauth.chatgpt.userCodeLabel")}
              </div>
              <div
                style={{
                  fontFamily: "var(--mono)",
                  fontSize: 24,
                  letterSpacing: 4,
                  fontWeight: 600,
                  color: "var(--ink-1)",
                  userSelect: "all",
                }}
              >
                {userCode}
              </div>
              <button
                type="button"
                className="btn bare sm"
                style={{ marginTop: 10 }}
                onClick={async () => {
                  try {
                    await navigator.clipboard.writeText(userCode);
                    setCopied(true);
                    window.setTimeout(() => setCopied(false), 1500);
                  } catch {}
                }}
              >
                {copied ? <Check size={11} /> : <Copy size={11} />}
                {copied ? t("common.copied") : t("common.copy")}
              </button>
            </div>

            <button
              type="button"
              className="btn"
              style={{ width: "100%", marginBottom: 12 }}
              onClick={() => openShell(verifyUrl).catch(() => {})}
            >
              <ExternalLink size={12} /> {t("oauth.chatgpt.openBrowser")}
            </button>

            <div
              style={{
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                gap: 8,
                fontSize: 12,
                color: "var(--ink-3)",
              }}
            >
              <Loader2 size={12} className="spin" />
              {t("oauth.chatgpt.waitingForAuth")}
            </div>
          </>
        )}

        {phase === "completed" && (
          <div style={{ display: "flex", alignItems: "center", gap: 8, color: "var(--ok)" }}>
            <Check size={14} /> {t("oauth.chatgpt.connected")}
          </div>
        )}

        {phase === "error" && (
          <div className="alert err" style={{ marginBottom: 12 }}>
            {errorMsg || t("oauth.chatgpt.failed")}
          </div>
        )}
      </div>
    </div>
  );
}
