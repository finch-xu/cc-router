import { useEffect, useRef, useState } from "react";
import { ExternalLink, LoaderCircle, Copy, Check, X } from "lucide-react";
import { runtime } from "@/runtime";
import { Spinner } from "@/components/Spinner";
import { useT } from "@/i18n";
import { api } from "@/api/tauri";
import type { ChatGptAccount } from "@/types";
import { errorText } from "@/lib/errorText";

interface Props {
  open: boolean;
  onClose: () => void;
  /** 授权成功时回传 authKey (PKCE flow_id 或 device_code) + account, 父组件在「保存订阅」时回传后端消费. */
  onSuccess: (deviceCode: string, account: ChatGptAccount) => void;
}

type AuthMethod = "browser" | "device";
type Phase = "idle" | "starting" | "waiting" | "completed" | "error";

const BROWSER_POLL_INTERVAL_MS = 1500;
const DEVICE_POLL_INTERVAL_MS = 2500;

export function ChatGptOAuthDialog({ open, onClose, onSuccess }: Props) {
  const { t } = useT();
  const [method, setMethod] = useState<AuthMethod>("browser");
  const [phase, setPhase] = useState<Phase>("idle");
  const [authKey, setAuthKey] = useState<string>("");
  const [userCode, setUserCode] = useState<string>("");
  const [openUrl, setOpenUrl] = useState<string>("");
  const [errorMsg, setErrorMsg] = useState<string>("");
  const [copied, setCopied] = useState(false);
  const [attemptSeq, setAttemptSeq] = useState(0);

  // 防止快速切换 Tab / 重试时旧请求回写覆盖新状态
  const runIdRef = useRef(0);
  // 记录当前活跃的浏览器 PKCE flow_id, 关闭弹窗或切到设备码时释放本地 1455 端口
  const activeBrowserFlowRef = useRef<string | null>(null);
  const completedRef = useRef(false);

  // 关闭弹窗时重置状态并释放 1455 监听
  useEffect(() => {
    if (!open) {
      runIdRef.current += 1;
      if (activeBrowserFlowRef.current && !completedRef.current) {
        api.cancelChatGptBrowserFlow(activeBrowserFlowRef.current).catch(() => {});
      }
      activeBrowserFlowRef.current = null;
      completedRef.current = false;
      setMethod("browser");
      setPhase("idle");
      setAuthKey("");
      setUserCode("");
      setOpenUrl("");
      setErrorMsg("");
      setCopied(false);
    }
  }, [open]);

  // 组件卸载时兜底释放 1455 端口
  useEffect(() => {
    return () => {
      if (activeBrowserFlowRef.current && !completedRef.current) {
        api.cancelChatGptBrowserFlow(activeBrowserFlowRef.current).catch(() => {});
      }
    };
  }, []);

  // 启动对应登录流程 (open=true, 或切换 method / 点击重试时触发)
  useEffect(() => {
    if (!open) return;

    const myRunId = ++runIdRef.current;
    completedRef.current = false;
    setPhase("starting");
    setAuthKey("");
    setUserCode("");
    setOpenUrl("");
    setErrorMsg("");
    setCopied(false);

    if (method === "browser") {
      api
        .startChatGptBrowserFlow()
        .then((res) => {
          if (runIdRef.current !== myRunId) {
            api.cancelChatGptBrowserFlow(res.flow_id).catch(() => {});
            return;
          }
          activeBrowserFlowRef.current = res.flow_id;
          setAuthKey(res.flow_id);
          setOpenUrl(res.authorize_url);
          setPhase("waiting");
          runtime.openExternal(res.authorize_url).catch(() => {});
        })
        .catch((e) => {
          if (runIdRef.current !== myRunId) return;
          setErrorMsg(errorText(e));
          setPhase("error");
        });
    } else {
      // 切到设备码前先释放可能存在的 1455 监听
      if (activeBrowserFlowRef.current) {
        api.cancelChatGptBrowserFlow(activeBrowserFlowRef.current).catch(() => {});
        activeBrowserFlowRef.current = null;
      }
      api
        .startChatGptDeviceFlow()
        .then((res) => {
          if (runIdRef.current !== myRunId) return;
          setAuthKey(res.device_code);
          setUserCode(res.user_code);
          setOpenUrl(res.verification_uri);
          setPhase("waiting");
          runtime.openExternal(res.verification_uri).catch(() => {});
        })
        .catch((e) => {
          if (runIdRef.current !== myRunId) return;
          setErrorMsg(errorText(e));
          setPhase("error");
        });
    }
  }, [open, method, attemptSeq]);

  // 轮询授权结果
  useEffect(() => {
    if (!open || phase !== "waiting" || !authKey) return;

    let stopped = false;
    const intervalMs = method === "browser" ? BROWSER_POLL_INTERVAL_MS : DEVICE_POLL_INTERVAL_MS;
    const poll = async () => {
      try {
        const account =
          method === "browser"
            ? await api.pollChatGptBrowserFlow(authKey)
            : await api.pollChatGptDeviceCode(authKey);
        if (stopped) return;
        if (account) {
          completedRef.current = true;
          activeBrowserFlowRef.current = null;
          setPhase("completed");
          onSuccess(authKey, account);
        }
      } catch (e) {
        if (stopped) return;
        if (method === "browser") {
          activeBrowserFlowRef.current = null;
        }
        setErrorMsg(errorText(e));
        setPhase("error");
      }
    };

    const id = window.setInterval(poll, intervalMs);
    return () => {
      stopped = true;
      window.clearInterval(id);
    };
  }, [open, phase, authKey, method, onSuccess]);

  if (!open) return null;

  function switchMethod(next: AuthMethod) {
    if (next === method) return;
    setMethod(next);
  }

  return (
    <div
      className="oauth-overlay"
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
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start", marginBottom: 14 }}>
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
            display: "flex",
            gap: 6,
            padding: 3,
            background: "var(--surface-2)",
            border: "1px solid var(--line)",
            borderRadius: 6,
            marginBottom: 14,
          }}
        >
          <button
            type="button"
            className={`btn sm ${method === "browser" ? "primary" : "bare"}`}
            style={{ flex: 1, justifyContent: "center" }}
            onClick={() => switchMethod("browser")}
          >
            {t("oauth.chatgpt.tabBrowser")}
          </button>
          <button
            type="button"
            className={`btn sm ${method === "device" ? "primary" : "bare"}`}
            style={{ flex: 1, justifyContent: "center" }}
            onClick={() => switchMethod("device")}
          >
            {t("oauth.chatgpt.tabDevice")}
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

        {phase === "waiting" && method === "browser" && (
          <>
            <div style={{ marginBottom: 16, color: "var(--ink-3)", fontSize: 13, lineHeight: 1.6 }}>
              {t("oauth.chatgpt.browserInstructions")}
            </div>

            <button
              type="button"
              className="btn"
              style={{ width: "100%", marginBottom: 12 }}
              onClick={() => runtime.openExternal(openUrl).catch(() => {})}
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
                marginBottom: 10,
              }}
            >
              <LoaderCircle size={12} className="spin" />
              {t("oauth.chatgpt.browserWaiting")}
            </div>

            <div style={{ textAlign: "center" }}>
              <button
                type="button"
                className="btn bare sm"
                style={{ fontSize: 11.5, color: "var(--ink-3)" }}
                onClick={() => switchMethod("device")}
              >
                {t("oauth.chatgpt.switchToDevice")}
              </button>
            </div>
          </>
        )}

        {phase === "waiting" && method === "device" && (
          <>
            <div style={{ marginBottom: 12, color: "var(--ink-3)", fontSize: 13, lineHeight: 1.55 }}>
              {t("oauth.chatgpt.instructions")}
            </div>
            <div style={{ marginBottom: 14, color: "var(--ink-4)", fontSize: 11.5, lineHeight: 1.5 }}>
              {t("oauth.chatgpt.deviceHint")}
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
                  fontFamily: "var(--font-mono)",
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
              onClick={() => runtime.openExternal(openUrl).catch(() => {})}
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
              <LoaderCircle size={12} className="spin" />
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
          <>
            <div className="alert err" style={{ marginBottom: 12 }}>
              {errorMsg || t("oauth.chatgpt.failed")}
            </div>
            <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
              {method === "browser" && (
                <button
                  type="button"
                  className="btn sm"
                  onClick={() => switchMethod("device")}
                >
                  {t("oauth.chatgpt.switchToDevice")}
                </button>
              )}
              <button
                type="button"
                className="btn primary sm"
                onClick={() => setAttemptSeq((n) => n + 1)}
              >
                {t("oauth.chatgpt.retry")}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
