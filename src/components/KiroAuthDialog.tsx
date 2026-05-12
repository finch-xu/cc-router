import { useEffect, useRef, useState } from "react";
import { ExternalLink, Loader2, Copy, Check, X, RefreshCw } from "lucide-react";
import { open as openShell } from "@tauri-apps/plugin-shell";
import { Spinner } from "@/components/Spinner";
import { useT } from "@/i18n";
import { api } from "@/api/tauri";
import type {
  KiroAccount,
  KiroDisguise,
  KiroImportResult,
} from "@/types";

interface Props {
  open: boolean;
  onClose: () => void;
  /** 成功时回传给父组件: 凭据来源 (session_id 或 device_code) + 用户最终的伪装字段. */
  onSuccess: (payload: KiroAuthSuccessPayload) => void;
}

export interface KiroAuthSuccessPayload {
  /** 二选一: 方案 A 用 session_id, 方案 B 用 device_code. */
  sessionId?: string;
  deviceCode?: string;
  region: string;
  authMethod: "social" | "idc";
  /** 用户在 dialog 里编辑过的伪装字段, 父组件 create_kiro_subscription 时回传后端. */
  disguise: KiroDisguise;
  /** 用户手填或导入的 profile_arn (可选, 方案 B 一般为空). */
  profileArn?: string;
}

const POLL_INTERVAL_MS = 2500;

/** 与 Rust 后端 KiroDisguise::default 保持一致. */
const DEFAULT_DISGUISE: KiroDisguise = {
  machine_id: "",
  kiro_version: "0.11.107",
  system_version:
    typeof navigator !== "undefined" && /Win/i.test(navigator.platform ?? "")
      ? "win32#10.0.22631"
      : "darwin#24.6.0",
  node_version: "22.22.0",
};

function generateMachineId(): string {
  const a = crypto.randomUUID().replace(/-/g, "");
  const b = crypto.randomUUID().replace(/-/g, "");
  return `${a}${b}`;
}

type Tab = "import" | "device_flow";
type Phase = "tab_select" | "importing" | "device_starting" | "device_waiting" | "error" | "disguise_form";

export function KiroAuthDialog({ open, onClose, onSuccess }: Props) {
  const { t } = useT();
  const [tab, setTab] = useState<Tab>("import");
  const [phase, setPhase] = useState<Phase>("tab_select");
  const [errorMsg, setErrorMsg] = useState<string>("");

  const [pasteJson, setPasteJson] = useState<string>("");
  const [importPath, setImportPath] = useState<string>(defaultIdeCachePath());
  const [importResult, setImportResult] = useState<KiroImportResult | null>(null);
  const [importProfileArn, setImportProfileArn] = useState<string>("");

  const [deviceCode, setDeviceCode] = useState<string>("");
  const [userCode, setUserCode] = useState<string>("");
  const [verifyUrl, setVerifyUrl] = useState<string>("");
  const [verifyUrlComplete, setVerifyUrlComplete] = useState<string>("");
  const [region, setRegion] = useState<string>("us-east-1");
  const [authMethod, setAuthMethod] = useState<"social" | "idc">("social");
  const [copied, setCopied] = useState(false);
  const pollTimer = useRef<number | null>(null);

  const [disguise, setDisguise] = useState<KiroDisguise>({
    ...DEFAULT_DISGUISE,
    machine_id: generateMachineId(),
  });

  useEffect(() => {
    if (!open) {
      setPhase("tab_select");
      setTab("import");
      setErrorMsg("");
      setPasteJson("");
      setImportResult(null);
      setImportProfileArn("");
      setDeviceCode("");
      setUserCode("");
      setVerifyUrl("");
      setVerifyUrlComplete("");
      setAuthMethod("social");
      setCopied(false);
      setDisguise({ ...DEFAULT_DISGUISE, machine_id: generateMachineId() });
      if (pollTimer.current !== null) {
        window.clearInterval(pollTimer.current);
        pollTimer.current = null;
      }
    }
  }, [open]);

  useEffect(() => {
    if (phase !== "device_waiting" || !deviceCode) return;

    let stopped = false;
    const poll = async () => {
      try {
        const account: KiroAccount | null = await api.pollKiroDeviceCode(deviceCode);
        if (stopped) return;
        if (account) {
          setRegion(account.region);
          setAuthMethod(account.auth_method);
          setPhase("disguise_form");
        }
      } catch (e) {
        if (stopped) return;
        setErrorMsg(String(e));
        setPhase("error");
      }
    };
    const id = window.setInterval(poll, POLL_INTERVAL_MS);
    pollTimer.current = id;
    return () => {
      stopped = true;
      window.clearInterval(id);
    };
  }, [phase, deviceCode]);

  async function handleImportFromFile() {
    setPhase("importing");
    setErrorMsg("");
    try {
      const res = await api.importKiroCredentialsFromFile(importPath);
      setImportResult(res);
      setRegion(res.preview.region);
      setAuthMethod(res.preview.auth_method);
      setPhase("disguise_form");
    } catch (e) {
      setErrorMsg(String(e));
      setPhase("error");
    }
  }

  async function handleImportFromText() {
    if (!pasteJson.trim()) {
      setErrorMsg(t("oauth.kiro.errPasteEmpty"));
      setPhase("error");
      return;
    }
    setPhase("importing");
    setErrorMsg("");
    try {
      const res = await api.importKiroCredentialsFromText(pasteJson);
      setImportResult(res);
      setRegion(res.preview.region);
      setAuthMethod(res.preview.auth_method);
      setPhase("disguise_form");
    } catch (e) {
      setErrorMsg(String(e));
      setPhase("error");
    }
  }

  async function handleStartDeviceFlow() {
    setPhase("device_starting");
    setErrorMsg("");
    try {
      const start = await api.startKiroDeviceFlow(region);
      setDeviceCode(start.device_code);
      setUserCode(start.user_code);
      setVerifyUrl(start.verification_uri);
      setVerifyUrlComplete(start.verification_uri_complete ?? "");
      setRegion(start.region);
      setAuthMethod("idc"); // device flow 总是 idc
      setPhase("device_waiting");
      const launchUrl = start.verification_uri_complete ?? start.verification_uri;
      openShell(launchUrl).catch(() => {});
    } catch (e) {
      setErrorMsg(String(e));
      setPhase("error");
    }
  }

  function handleConfirmDisguise() {
    onSuccess({
      sessionId: importResult?.session_id,
      deviceCode: importResult ? undefined : deviceCode,
      region,
      authMethod,
      disguise,
      profileArn: importProfileArn || undefined,
    });
  }

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
        style={{ width: 540, maxWidth: "90vw", maxHeight: "90vh", overflow: "auto", padding: 24 }}
        onClick={(e) => e.stopPropagation()}
      >
        <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start", marginBottom: 16 }}>
          <div>
            <h3 style={{ margin: 0, fontSize: 16 }}>{t("oauth.kiro.title")}</h3>
            <div style={{ fontSize: 12, color: "var(--ink-3)", marginTop: 4 }}>
              {t("oauth.kiro.subtitle")}
            </div>
          </div>
          <button type="button" onClick={onClose} className="btn bare sm" style={{ padding: 4 }} aria-label={t("common.close")}>
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
          {t("oauth.kiro.tosWarning")}
        </div>

        {phase === "tab_select" && (
          <>
            <div style={{ display: "flex", gap: 8, marginBottom: 16, borderBottom: "1px solid var(--line)" }}>
              <TabButton active={tab === "import"} onClick={() => setTab("import")}>
                {t("oauth.kiro.tabImport")}
              </TabButton>
              <TabButton active={tab === "device_flow"} onClick={() => setTab("device_flow")}>
                {t("oauth.kiro.tabDeviceFlow")}
              </TabButton>
            </div>

            {tab === "import" && (
              <ImportTabContent
                importPath={importPath}
                setImportPath={setImportPath}
                pasteJson={pasteJson}
                setPasteJson={setPasteJson}
                onImportFromFile={handleImportFromFile}
                onImportFromText={handleImportFromText}
              />
            )}

            {tab === "device_flow" && (
              <DeviceFlowIntroContent
                region={region}
                setRegion={setRegion}
                onStart={handleStartDeviceFlow}
              />
            )}
          </>
        )}

        {phase === "importing" && (
          <div style={{ display: "flex", alignItems: "center", gap: 8, color: "var(--ink-3)" }}>
            <Spinner /> {t("oauth.kiro.importing")}
          </div>
        )}

        {phase === "device_starting" && (
          <div style={{ display: "flex", alignItems: "center", gap: 8, color: "var(--ink-3)" }}>
            <Spinner /> {t("oauth.kiro.deviceStarting")}
          </div>
        )}

        {phase === "device_waiting" && (
          <>
            <div style={{ marginBottom: 16, color: "var(--ink-3)", fontSize: 13 }}>
              {t("oauth.kiro.deviceInstructions")}
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
                {t("oauth.kiro.userCodeLabel")}
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
              onClick={() => openShell(verifyUrlComplete || verifyUrl).catch(() => {})}
            >
              <ExternalLink size={12} /> {t("oauth.kiro.openBrowser")}
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
              {t("oauth.kiro.waitingForAuth")}
            </div>
          </>
        )}

        {phase === "disguise_form" && (
          <>
            <div
              style={{
                padding: "8px 12px",
                marginBottom: 16,
                background: "var(--surface-2)",
                fontSize: 12,
                lineHeight: 1.6,
                color: "var(--ink-2)",
                borderRadius: 4,
              }}
            >
              <strong>{t("oauth.kiro.credentialReady")}</strong>
              <br />
              {t("oauth.kiro.authMethod")}: <code>{authMethod}</code> · region: <code>{region}</code>
              {importResult?.preview.has_profile_arn && (
                <>
                  <br />
                  <span>{t("oauth.kiro.profileArnDetected")}</span>
                </>
              )}
            </div>
            <div style={{ marginBottom: 12, fontSize: 13, fontWeight: 500, color: "var(--ink-2)" }}>
              {t("oauth.kiro.disguiseTitle")}
            </div>
            <div style={{ fontSize: 11.5, color: "var(--ink-3)", marginBottom: 12, lineHeight: 1.55 }}>
              {t("oauth.kiro.disguiseHint")}
            </div>
            <DisguiseInput label="Machine ID" value={disguise.machine_id} onChange={(v) => setDisguise({ ...disguise, machine_id: v })} hint="64 hex" extra={
              <button type="button" className="btn bare sm" onClick={() => setDisguise({ ...disguise, machine_id: generateMachineId() })}>
                <RefreshCw size={11} /> {t("oauth.kiro.regen")}
              </button>
            }/>
            <DisguiseInput label="Kiro Version" value={disguise.kiro_version} onChange={(v) => setDisguise({ ...disguise, kiro_version: v })} />
            <DisguiseInput label="System Version" value={disguise.system_version} onChange={(v) => setDisguise({ ...disguise, system_version: v })} />
            <DisguiseInput label="Node Version" value={disguise.node_version} onChange={(v) => setDisguise({ ...disguise, node_version: v })} />
            {!importResult?.preview.has_profile_arn && (
              <DisguiseInput
                label="Profile ARN (optional)"
                value={importProfileArn}
                onChange={setImportProfileArn}
                hint="arn:aws:codewhisperer:..."
              />
            )}
            <button
              type="button"
              className="btn primary"
              style={{ width: "100%", marginTop: 16 }}
              onClick={handleConfirmDisguise}
            >
              <Check size={12} /> {t("oauth.kiro.confirmAndNext")}
            </button>
          </>
        )}

        {phase === "error" && (
          <>
            <div className="alert err" style={{ marginBottom: 12 }}>
              {errorMsg || t("oauth.kiro.failed")}
            </div>
            <button type="button" className="btn" style={{ width: "100%" }} onClick={() => setPhase("tab_select")}>
              {t("common.retry")}
            </button>
          </>
        )}
      </div>
    </div>
  );
}

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      style={{
        padding: "8px 16px",
        background: "transparent",
        border: "none",
        borderBottom: active ? "2px solid var(--accent)" : "2px solid transparent",
        color: active ? "var(--ink-1)" : "var(--ink-3)",
        cursor: "pointer",
        fontSize: 13,
        fontWeight: active ? 500 : 400,
      }}
    >
      {children}
    </button>
  );
}

function ImportTabContent({
  importPath,
  setImportPath,
  pasteJson,
  setPasteJson,
  onImportFromFile,
  onImportFromText,
}: {
  importPath: string;
  setImportPath: (v: string) => void;
  pasteJson: string;
  setPasteJson: (v: string) => void;
  onImportFromFile: () => void;
  onImportFromText: () => void;
}) {
  const { t } = useT();
  return (
    <>
      <div style={{ fontSize: 12.5, color: "var(--ink-3)", lineHeight: 1.6, marginBottom: 12 }}>
        {t("oauth.kiro.importDescription")}
      </div>
      <div style={{ marginBottom: 16 }}>
        <label style={{ display: "block", fontSize: 12, color: "var(--ink-3)", marginBottom: 4 }}>
          {t("oauth.kiro.localFilePath")}
        </label>
        <div style={{ display: "flex", gap: 8 }}>
          <input
            type="text"
            value={importPath}
            onChange={(e) => setImportPath(e.target.value)}
            className="input"
            style={{ flex: 1 }}
            placeholder="~/.aws/sso/cache/kiro-auth-token.json"
          />
          <button type="button" className="btn" onClick={onImportFromFile} disabled={!importPath}>
            {t("oauth.kiro.readFile")}
          </button>
        </div>
      </div>
      <div style={{ fontSize: 11, color: "var(--ink-3)", margin: "12px 0 8px" }}>{t("common.orPasteBelow")}</div>
      <textarea
        value={pasteJson}
        onChange={(e) => setPasteJson(e.target.value)}
        className="input"
        rows={6}
        style={{ width: "100%", fontFamily: "var(--mono)", fontSize: 11 }}
        placeholder='{ "refreshToken": "...", "region": "us-east-1", ... }'
      />
      <button type="button" className="btn primary" style={{ width: "100%", marginTop: 12 }} onClick={onImportFromText} disabled={!pasteJson.trim()}>
        {t("oauth.kiro.importPasted")}
      </button>
    </>
  );
}

function DeviceFlowIntroContent({
  region,
  setRegion,
  onStart,
}: {
  region: string;
  setRegion: (v: string) => void;
  onStart: () => void;
}) {
  const { t } = useT();
  return (
    <>
      <div style={{ fontSize: 12.5, color: "var(--ink-3)", lineHeight: 1.6, marginBottom: 16 }}>
        {t("oauth.kiro.deviceFlowDescription")}
      </div>
      <div style={{ marginBottom: 16 }}>
        <label style={{ display: "block", fontSize: 12, color: "var(--ink-3)", marginBottom: 4 }}>
          AWS Region
        </label>
        <input
          type="text"
          value={region}
          onChange={(e) => setRegion(e.target.value)}
          className="input"
          style={{ width: "100%" }}
          placeholder="us-east-1"
        />
      </div>
      <button type="button" className="btn primary" style={{ width: "100%" }} onClick={onStart}>
        <ExternalLink size={12} /> {t("oauth.kiro.startBuilderIdLogin")}
      </button>
    </>
  );
}

function DisguiseInput({
  label,
  value,
  onChange,
  hint,
  extra,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  hint?: string;
  extra?: React.ReactNode;
}) {
  return (
    <div style={{ marginBottom: 10 }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 4 }}>
        <label style={{ fontSize: 11.5, color: "var(--ink-3)" }}>{label}</label>
        {extra}
      </div>
      <input
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="input"
        style={{ width: "100%", fontFamily: "var(--mono)", fontSize: 11.5 }}
        placeholder={hint}
      />
    </div>
  );
}

/** mac/linux 默认路径; Windows 用户需手填 `%USERPROFILE%\.aws\sso\cache\...`. */
function defaultIdeCachePath(): string {
  return "~/.aws/sso/cache/kiro-auth-token.json";
}
