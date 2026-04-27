import { useState } from "react";
import { Link } from "react-router-dom";
import { Construction } from "lucide-react";
import { CopyableBlock } from "@/components/CopyableBlock";
import { EmptyState } from "@/components/EmptyState";
import { useEnvSnippet, useProxyStatus, useSettings } from "@/hooks/useSettings";
import { useT } from "@/i18n";
import { cn } from "@/lib/utils";

type Tab = "claude-code" | "cc-switch" | "other";

export function GuidePage() {
  const { t } = useT();
  const [tab, setTab] = useState<Tab>("claude-code");

  return (
    <>
      <div className="page-header">
        <h1>{t("guide.title")}</h1>
        <div className="subtitle">{t("guide.subtitle")}</div>
      </div>

      <div className="tabs">
        <button
          className={cn("tab", tab === "claude-code" && "active")}
          onClick={() => setTab("claude-code")}
          type="button"
        >
          Claude Code
        </button>
        <button
          className={cn("tab", tab === "cc-switch" && "active")}
          onClick={() => setTab("cc-switch")}
          type="button"
        >
          cc-switch
        </button>
        <button
          className={cn("tab", tab === "other" && "active")}
          onClick={() => setTab("other")}
          type="button"
        >
          {t("guide.tab.other")}
        </button>
      </div>

      {tab === "claude-code" && <ClaudeCodeTab />}
      {tab === "cc-switch" && (
        <EmptyState
          icon={Construction}
          message={t("guide.tab.ccSwitchEmpty")}
        />
      )}
      {tab === "other" && (
        <EmptyState
          icon={Construction}
          message={t("guide.tab.otherEmpty")}
        />
      )}
    </>
  );
}

function ClaudeCodeTab() {
  const { t } = useT();
  const proxy = useProxyStatus();
  const settings = useSettings();
  const env = useEnvSnippet();

  const port = proxy.data?.port ?? 23456;
  const token = settings.data?.auth_token ?? "";
  const running = proxy.data?.running ?? false;

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
          <CopyableBlock text={claudeJson} highlight={false} />
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
            <CopyableBlock text={env.data} />
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
