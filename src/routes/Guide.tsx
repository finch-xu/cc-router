import { useState } from "react";
import { Link } from "react-router-dom";
import { Construction } from "lucide-react";
import { CopyableBlock } from "@/components/CopyableBlock";
import { EmptyState } from "@/components/EmptyState";
import { useEnvSnippet, useProxyStatus, useSettings } from "@/hooks/useSettings";
import { cn } from "@/lib/utils";

type Tab = "claude-code" | "cc-switch" | "other";

export function GuidePage() {
  const [tab, setTab] = useState<Tab>("claude-code");

  return (
    <>
      <div className="page-header">
        <h1>接入指南</h1>
        <div className="subtitle">
          把 cc-router 作为 Anthropic Messages 兼容端点接入各类客户端。
        </div>
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
          其他 AI Agent 工具
        </button>
      </div>

      {tab === "claude-code" && <ClaudeCodeTab />}
      {tab === "cc-switch" && (
        <EmptyState
          icon={Construction}
          message="cc-switch 接入指引正在完善中。"
        />
      )}
      {tab === "other" && (
        <EmptyState
          icon={Construction}
          message="其他 AI Agent 工具的接入指引正在完善中。"
        />
      )}
    </>
  );
}

function ClaudeCodeTab() {
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
        CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
        ANTHROPIC_MODEL: "model-opus",
        ANTHROPIC_DEFAULT_SONNET_MODEL: "model-sonnet",
        ANTHROPIC_DEFAULT_OPUS_MODEL: "model-opus",
        ANTHROPIC_DEFAULT_HAIKU_MODEL: "model-haiku",
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
          <div className="card-title">代理监听地址</div>
          <span className="card-sub mono">
            127.0.0.1:{port} · {running ? "运行中" : "未启动"}
          </span>
        </div>
        <div className="card-body">
          <div className="field-hint">
            cc-router 在本地启动 HTTP 代理,把多家订阅聚合成单一 Anthropic Messages 端点。
            端口、监听地址、鉴权 token 可在
            <Link
              to="/settings"
              style={{
                color: "var(--accent-ink)",
                textDecoration: "none",
                margin: "0 4px",
              }}
            >
              设置
            </Link>
            页修改(改端口/监听地址需重启 app 生效)。
          </div>
        </div>
      </div>

      {/* 方式 1: settings.json (推荐) */}
      <div className="card section">
        <div className="card-head">
          <div className="card-title">方式 1 · 直接写入 settings.json(推荐)</div>
          <span className="card-sub">最稳,不依赖 shell 环境</span>
        </div>
        <div className="card-body">
          <div className="field-hint" style={{ marginBottom: 10 }}>
            把下面的 JSON 合并写入 Claude Code 配置文件。如果文件不存在,新建即可。
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
            <span className="mono">ANTHROPIC_AUTH_TOKEN</span> 已填入 cc-router 当前的真实 token,
            可直接使用。token 可在
            <Link
              to="/settings"
              style={{
                color: "var(--accent-ink)",
                textDecoration: "none",
                margin: "0 4px",
              }}
            >
              设置
            </Link>
            页重新生成。
          </div>
        </div>
      </div>

      {/* 方式 2: env */}
      <div className="card section">
        <div className="card-head">
          <div className="card-title">方式 2 · 环境变量(备选)</div>
          <span className="card-sub">GUI 启动的客户端可能读不到 shell env,优先用方式 1</span>
        </div>
        <div className="card-body">
          {env.data ? (
            <CopyableBlock text={env.data} />
          ) : (
            <div className="field-hint">加载中…</div>
          )}
          <div className="field-hint" style={{ marginTop: 10 }}>
            把上面这几行加到 <span className="mono">~/.zshrc</span>、
            <span className="mono"> ~/.bashrc</span> 或 PowerShell profile,
            然后重启 terminal 后启动 Claude Code。
          </div>
        </div>
      </div>
    </>
  );
}
