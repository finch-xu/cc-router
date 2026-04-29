import { useState, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { Bot, Boxes } from "lucide-react";
import ClaudeCode from "@lobehub/icons/es/ClaudeCode";
import Cline from "@lobehub/icons/es/Cline";
import HermesAgent from "@lobehub/icons/es/HermesAgent";
import KiloCode from "@lobehub/icons/es/KiloCode";
import OpenClaw from "@lobehub/icons/es/OpenClaw";
import OpenCode from "@lobehub/icons/es/OpenCode";
import Qwen from "@lobehub/icons/es/Qwen";
import RooCode from "@lobehub/icons/es/RooCode";
import { CopyableBlock } from "@/components/CopyableBlock";
import { useEnvSnippet, useProxyEndpoint } from "@/hooks/useSettings";
import { useT } from "@/i18n";
import { cn } from "@/lib/utils";

type Tab = "claude-code" | "cc-switch" | "openclaw" | "hermes" | "opencode" | "others";

const ICON_SIZE = 14;
const COMING_SOON_SIZE = 28;

const COMING_SOON: { name: string; icon: ReactNode }[] = [
  { name: "Kilo Code", icon: <KiloCode.Avatar size={COMING_SOON_SIZE} /> },
  { name: "Cline", icon: <Cline.Avatar size={COMING_SOON_SIZE} /> },
  { name: "Qwen Code", icon: <Qwen.Avatar size={COMING_SOON_SIZE} /> },
  { name: "Roo Code", icon: <RooCode.Avatar size={COMING_SOON_SIZE} /> },
];

export function GuidePage() {
  const { t } = useT();
  const [tab, setTab] = useState<Tab>("claude-code");

  const TABS: { id: Tab; label: string; icon: ReactNode }[] = [
    { id: "claude-code", label: "Claude Code", icon: <ClaudeCode.Color size={ICON_SIZE} /> },
    { id: "cc-switch", label: "cc-switch", icon: <Bot size={ICON_SIZE} /> },
    { id: "openclaw", label: "OpenClaw", icon: <OpenClaw.Color size={ICON_SIZE} /> },
    { id: "hermes", label: "Hermes Agent", icon: <HermesAgent size={ICON_SIZE} /> },
    { id: "opencode", label: "OpenCode", icon: <OpenCode size={ICON_SIZE} /> },
    { id: "others", label: t("guide.tab.others"), icon: <Boxes size={ICON_SIZE} /> },
  ];

  return (
    <>
      <div className="page-header">
        <h1>{t("guide.title")}</h1>
        <div className="subtitle">{t("guide.subtitle")}</div>
      </div>

      <div className="tabs">
        {TABS.map(({ id, label, icon }) => (
          <button
            key={id}
            className={cn("tab", tab === id && "active")}
            onClick={() => setTab(id)}
            type="button"
          >
            {icon}
            {label}
          </button>
        ))}
      </div>

      {tab === "claude-code" && <ClaudeCodeTab />}
      {tab === "cc-switch" && <CcSwitchTab />}
      {tab === "openclaw" && <OpenClawTab />}
      {tab === "hermes" && <HermesAgentTab />}
      {tab === "opencode" && <OpenCodeTab />}
      {tab === "others" && <OthersTab />}
    </>
  );
}

function OthersTab() {
  const { t } = useT();
  return (
    <div className="card section">
      <div className="card-head">
        <div className="card-title">{t("guide.others.title")}</div>
      </div>
      <div className="card-body">
        <div className="brand-grid">
          {COMING_SOON.map(({ name, icon }) => (
            <div key={name} className="brand-card">
              {icon}
              <span>{name}</span>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

function ClaudeCodeTab() {
  const { t } = useT();
  const { port, token, running } = useProxyEndpoint();
  const env = useEnvSnippet();

  const claudeJson = JSON.stringify(
    {
      env: {
        ANTHROPIC_BASE_URL: `http://127.0.0.1:${port}`,
        ANTHROPIC_AUTH_TOKEN: token,
        API_TIMEOUT_MS: "3000000",
        ANTHROPIC_MODEL: "model-opus",
        ANTHROPIC_DEFAULT_OPUS_MODEL: "model-opus",
        ANTHROPIC_DEFAULT_SONNET_MODEL: "model-sonnet",
        ANTHROPIC_DEFAULT_HAIKU_MODEL: "model-haiku",
        CLAUDE_CODE_SUBAGENT_MODEL: "model-opus",
        CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
        CLAUDE_CODE_DISABLE_NONSTREAMING_FALLBACK: "1",
        CLAUDE_CODE_EFFORT_LEVEL: "max"
      },
    },
    null,
    2,
  );

  return (
    <>
      {/* 代理监听地址 */}
      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("guide.proxy.title")}</div>
          <span className="card-sub mono">
            127.0.0.1:{port} ·{" "}
            {running ? t("settings.proxy.statusRunning") : t("guide.proxy.statusNotStarted")}
          </span>
        </div>
        <div className="card-body">
          <div className="field-hint">
            {t("guide.proxy.intro1")}
            <Link
              to="/settings"
              style={{
                color: "var(--accent-ink)",
                textDecoration: "none",
                margin: "0 4px",
              }}
            >
              {t("guide.proxy.settingsLink")}
            </Link>
            {t("guide.proxy.intro2")}
          </div>
        </div>
      </div>

      {/* 方式 1: settings.json (推荐) */}
      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("guide.method1.title")}</div>
          <span className="card-sub">{t("guide.method1.sub")}</span>
        </div>
        <div className="card-body">
          <div className="field-hint" style={{ marginBottom: 10 }}>
            {t("guide.method1.desc")}
          </div>
          <table
            className="table"
            style={{
              marginBottom: 14,
              fontSize: 12,
              tableLayout: "fixed",
            }}
          >
            <tbody>
              <tr>
                <td style={{ width: 90, color: "var(--ink-3)" }}>macOS</td>
                <td className="mono">~/.claude/settings.json</td>
              </tr>
              <tr>
                <td style={{ color: "var(--ink-3)" }}>Linux</td>
                <td className="mono">~/.claude/settings.json</td>
              </tr>
              <tr>
                <td style={{ color: "var(--ink-3)" }}>Windows</td>
                <td className="mono">%USERPROFILE%\.claude\settings.json</td>
              </tr>
            </tbody>
          </table>
          <CopyableBlock text={claudeJson} />
          <div className="field-hint" style={{ marginTop: 10 }}>
            <span className="mono">ANTHROPIC_AUTH_TOKEN</span>
            {t("guide.method1.note1")}
            <Link
              to="/settings"
              style={{
                color: "var(--accent-ink)",
                textDecoration: "none",
                margin: "0 4px",
              }}
            >
              {t("guide.proxy.settingsLink")}
            </Link>
            {t("guide.method1.note2")}
          </div>
        </div>
      </div>

      {/* 方式 2: env */}
      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("guide.method2.title")}</div>
          <span className="card-sub">{t("guide.method2.sub")}</span>
        </div>
        <div className="card-body">
          {env.data ? (
            <CopyableBlock text={env.data} highlight />
          ) : (
            <div className="field-hint">{t("common.loading")}</div>
          )}
          <div className="field-hint" style={{ marginTop: 10 }}>
            {t("guide.method2.note1")}
            <span className="mono">~/.zshrc</span>
            {t("guide.method2.note2")}
            <span className="mono">~/.bashrc</span>
            {t("guide.method2.note3")}
          </div>
        </div>
      </div>
    </>
  );
}

function CcSwitchTab() {
  const { t } = useT();
  const { port, token } = useProxyEndpoint();
  const baseUrl = `http://127.0.0.1:${port}`;

  const ccSwitchJson = JSON.stringify(
    {
      env: {
        ANTHROPIC_BASE_URL: baseUrl,
        ANTHROPIC_AUTH_TOKEN: token,
        API_TIMEOUT_MS: "3000000",
        ANTHROPIC_MODEL: "model-opus",
        ANTHROPIC_DEFAULT_OPUS_MODEL: "model-opus",
        ANTHROPIC_DEFAULT_SONNET_MODEL: "model-sonnet",
        ANTHROPIC_DEFAULT_HAIKU_MODEL: "model-haiku",
        CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
      },
    },
    null,
    2,
  );

  return (
    <>
      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("guide.ccswitch.entry.title")}</div>
        </div>
        <div className="card-body">
          <div className="field-hint">{t("guide.ccswitch.entry.body")}</div>
        </div>
      </div>

      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("guide.ccswitch.method1.title")}</div>
          <span className="card-sub">{t("guide.ccswitch.method1.sub")}</span>
        </div>
        <div className="card-body">
          <div className="field-hint" style={{ marginBottom: 10 }}>
            {t("guide.ccswitch.method1.desc")}
          </div>
          <table
            className="table"
            style={{ marginBottom: 14, fontSize: 12, tableLayout: "fixed" }}
          >
            <tbody>
              <tr>
                <td style={{ width: 130, color: "var(--ink-3)" }}>
                  {t("guide.ccswitch.method1.fieldUrl")}
                </td>
                <td className="mono">{baseUrl}</td>
              </tr>
              <tr>
                <td style={{ color: "var(--ink-3)" }}>
                  {t("guide.ccswitch.method1.fieldKey")}
                </td>
                <td className="mono">{token}</td>
              </tr>
              <tr>
                <td style={{ color: "var(--ink-3)" }}>
                  {t("guide.ccswitch.method1.fieldFormat")}
                </td>
                <td>Anthropic Messages</td>
              </tr>
              <tr>
                <td style={{ color: "var(--ink-3)" }}>
                  {t("guide.ccswitch.method1.fieldAuthField")}
                </td>
                <td className="mono">ANTHROPIC_AUTH_TOKEN</td>
              </tr>
            </tbody>
          </table>
          <div className="field-hint" style={{ marginBottom: 8 }}>
            {t("guide.ccswitch.method1.modelMapTitle")}
          </div>
          <table
            className="table"
            style={{ fontSize: 12, tableLayout: "fixed" }}
          >
            <tbody>
              <tr>
                <td style={{ width: 200, color: "var(--ink-3)" }}>
                  {t("guide.ccswitch.method1.modelOpus")}
                </td>
                <td className="mono">model-opus</td>
              </tr>
              <tr>
                <td style={{ color: "var(--ink-3)" }}>
                  {t("guide.ccswitch.method1.modelSonnet")}
                </td>
                <td className="mono">model-sonnet</td>
              </tr>
              <tr>
                <td style={{ color: "var(--ink-3)" }}>
                  {t("guide.ccswitch.method1.modelHaiku")}
                </td>
                <td className="mono">model-haiku</td>
              </tr>
            </tbody>
          </table>
          <div className="field-hint" style={{ marginTop: 10 }}>
            {t("guide.ccswitch.method1.note")}
          </div>
        </div>
      </div>

      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("guide.ccswitch.method2.title")}</div>
          <span className="card-sub">{t("guide.ccswitch.method2.sub")}</span>
        </div>
        <div className="card-body">
          <div className="field-hint" style={{ marginBottom: 10 }}>
            {t("guide.ccswitch.method2.desc")}
          </div>
          <CopyableBlock text={ccSwitchJson} />
        </div>
      </div>
    </>
  );
}

function OpenClawTab() {
  const { t } = useT();
  const { port, token } = useProxyEndpoint();

  const configJson = JSON.stringify(
    {
      env: {
        ANTHROPIC_API_KEY: token,
        ANTHROPIC_BASE_URL: `http://127.0.0.1:${port}`,
      },
      agents: {
        defaults: {
          model: { primary: "anthropic/model-opus" },
          models: {
            "anthropic/model-opus": {
              params: {
                cacheRetention: "long",
                thinking: "adaptive",
              },
            },
          },
        },
      },
    },
    null,
    2,
  );

  return (
    <>
      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("guide.openclaw.path.title")}</div>
          <span className="card-sub mono">~/.openclaw/config.json</span>
        </div>
        <div className="card-body">
          <div className="field-hint">{t("guide.openclaw.path.note")}</div>
        </div>
      </div>

      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("guide.openclaw.config.title")}</div>
        </div>
        <div className="card-body">
          <div className="field-hint" style={{ marginBottom: 10 }}>
            {t("guide.openclaw.config.desc")}
          </div>
          <CopyableBlock text={configJson} />
          <div className="field-hint" style={{ marginTop: 10 }}>
            {t("guide.openclaw.note.modelRef")}
          </div>
          <div className="field-hint">
            {t("guide.openclaw.note.tokenHint")}
          </div>
        </div>
      </div>
    </>
  );
}

function HermesAgentTab() {
  const { t } = useT();
  const { port, token } = useProxyEndpoint();

  const configYaml = `model:
  provider: anthropic
  base_url: http://127.0.0.1:${port}
  api_mode: anthropic_messages
  default: model-sonnet
  context_length: 200000`;

  const authCmd = `hermes auth add anthropic --type api-key --api-key ${token || "<your-cc-router-token>"}`;

  return (
    <>
      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("guide.hermes.path.title")}</div>
          <span className="card-sub mono">~/.hermes/config.yaml</span>
        </div>
        <div className="card-body">
          <div className="field-hint">{t("guide.hermes.path.note")}</div>
        </div>
      </div>

      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("guide.hermes.config.title")}</div>
        </div>
        <div className="card-body">
          <div className="field-hint" style={{ marginBottom: 10 }}>
            {t("guide.hermes.config.desc")}
          </div>
          <CopyableBlock text={configYaml} />
          <div className="field-hint" style={{ marginTop: 10 }}>
            {t("guide.hermes.note.apiMode")}
          </div>
        </div>
      </div>

      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("guide.hermes.auth.title")}</div>
        </div>
        <div className="card-body">
          <div className="field-hint" style={{ marginBottom: 10 }}>
            {t("guide.hermes.auth.desc")}
          </div>
          <CopyableBlock text={authCmd} />
        </div>
      </div>
    </>
  );
}

function OpenCodeTab() {
  const { t } = useT();
  const { port, token } = useProxyEndpoint();

  const configJson = JSON.stringify(
    {
      $schema: "https://opencode.ai/config.json",
      provider: {
        "cc-router": {
          npm: "@ai-sdk/anthropic",
          options: {
            baseURL: `http://127.0.0.1:${port}`,
            apiKey: token,
          },
          models: {
            "model-opus": { name: "model-opus" },
            "model-sonnet": { name: "model-sonnet" },
            "model-haiku": { name: "model-haiku" },
          },
        },
      },
    },
    null,
    2,
  );

  return (
    <>
      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("guide.opencode.path.title")}</div>
        </div>
        <div className="card-body">
          <table
            className="table"
            style={{ fontSize: 12, tableLayout: "fixed" }}
          >
            <tbody>
              <tr>
                <td style={{ width: 90, color: "var(--ink-3)" }}>
                  {t("guide.opencode.path.project")}
                </td>
                <td className="mono">opencode.json</td>
              </tr>
              <tr>
                <td style={{ color: "var(--ink-3)" }}>
                  {t("guide.opencode.path.global")}
                </td>
                <td className="mono">~/.config/opencode/opencode.json</td>
              </tr>
            </tbody>
          </table>
          <div className="field-hint" style={{ marginTop: 10 }}>
            {t("guide.opencode.path.note")}
          </div>
        </div>
      </div>

      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("guide.opencode.config.title")}</div>
        </div>
        <div className="card-body">
          <div className="field-hint" style={{ marginBottom: 10 }}>
            {t("guide.opencode.config.desc")}
          </div>
          <CopyableBlock text={configJson} />
          <div className="field-hint" style={{ marginTop: 10 }}>
            {t("guide.opencode.note.baseUrlTrap")}
          </div>
          <div className="field-hint">
            {t("guide.opencode.note.usage")}
          </div>
        </div>
      </div>
    </>
  );
}
