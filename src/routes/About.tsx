import {
  Github,
  ExternalLink,
  RefreshCw,
  Download,
  RotateCw,
  AlertCircle,
  CheckCircle2,
} from "lucide-react";
import { open as openShell } from "@tauri-apps/plugin-shell";
import { version as VERSION } from "../../package.json";
import logoUrl from "@/assets/logo.png";
import { useUpdater } from "@/hooks/useUpdater";
import { openReleasePage } from "@/lib/updater";
import { fmtBytes } from "@/lib/format";

const REPO_URL = "https://github.com/finch-xu/cc-router";
const DOCS_URL = "https://github.com/finch-xu/cc-router#readme";

export function AboutPage() {
  return (
    <>
      <div className="page-header">
        <h1>关于</h1>
        <div className="subtitle">项目信息与版权</div>
      </div>

      <div className="card about-card">
        <div className="about-mark">
          <img src={logoUrl} alt="cc-router" />
        </div>
        <div className="about-name">cc-router</div>
        <div className="about-version">v{VERSION}</div>
        <div className="about-desc">
          本地 HTTP 代理,将多家大模型订阅聚合为单一 Anthropic Messages API 端点,供 Claude Code 透明切换。
        </div>

        <UpdaterBlock />

        <div style={{ display: "flex", gap: 8, justifyContent: "center", flexWrap: "wrap" }}>
          <button
            className="btn"
            type="button"
            onClick={() => openShell(REPO_URL).catch(() => {})}
          >
            <Github size={13} /> GitHub 仓库
          </button>
          <button
            className="btn"
            type="button"
            onClick={() => openShell(DOCS_URL).catch(() => {})}
          >
            <ExternalLink size={12} /> 文档
          </button>
        </div>
        <div className="about-meta">
          <span>© 2026 finch-xu</span>
          <span>·</span>
          <span>MIT License</span>
          <span>·</span>
          <span>Tauri 2 · React 19</span>
        </div>
      </div>
    </>
  );
}

const ROW_STYLE: React.CSSProperties = {
  margin: "12px auto 16px",
  maxWidth: 480,
  padding: "8px 12px",
  borderRadius: 8,
  fontSize: 13,
  color: "var(--ink-3)",
  display: "flex",
  alignItems: "center",
  justifyContent: "space-between",
  gap: 12,
};

const ALERT_STYLE: React.CSSProperties = {
  margin: "12px auto 16px",
  maxWidth: 480,
  flexDirection: "column",
  textAlign: "left",
};

const NOTES_STYLE: React.CSSProperties = {
  margin: "8px 0",
  padding: "8px 10px",
  borderRadius: 6,
  background: "var(--surface-3)",
  fontSize: 12,
  whiteSpace: "pre-wrap",
  maxHeight: 160,
  overflow: "auto",
};

function UpdaterBlock() {
  const { status, detected, progress, errorMessage, check, install, restart } = useUpdater();

  if (status === "idle" || status === "checking" || status === "up_to_date") {
    return (
      <div style={ROW_STYLE}>
        <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
          {status === "checking" ? (
            <>
              <RefreshCw size={13} className="spin" /> 检查更新中…
            </>
          ) : status === "up_to_date" ? (
            <>
              <CheckCircle2 size={13} /> 已是最新版本
            </>
          ) : (
            <>检查应用更新</>
          )}
        </span>
        <button
          className="btn"
          type="button"
          disabled={status === "checking"}
          onClick={() => void check()}
        >
          <RefreshCw size={12} /> {status === "up_to_date" ? "重新检查" : "检查更新"}
        </button>
      </div>
    );
  }

  if (status === "available" && detected) {
    const isManual = detected.kind === "manual";
    return (
      <div className="alert" style={ALERT_STYLE}>
        <div style={{ fontWeight: 600 }}>发现新版本 v{detected.version}</div>
        <div style={{ color: "var(--ink-3)", fontSize: 12, marginTop: 2 }}>
          当前 v{VERSION}
          {isManual ? " · deb 安装包不支持原地升级,需手动重装" : ""}
        </div>
        {detected.body && <div style={NOTES_STYLE}>{detected.body}</div>}
        <div style={{ display: "flex", gap: 8, justifyContent: "flex-end", marginTop: 6 }}>
          {isManual ? (
            <button className="btn btn-primary" type="button" onClick={() => void openReleasePage()}>
              <ExternalLink size={12} /> 前往下载页
            </button>
          ) : (
            <button className="btn btn-primary" type="button" onClick={() => void install()}>
              <Download size={12} /> 立即更新
            </button>
          )}
        </div>
      </div>
    );
  }

  if (status === "downloading" && detected) {
    const total = progress?.total ?? null;
    const downloaded = progress?.downloaded ?? 0;
    const percent = total ? Math.min(100, Math.round((downloaded / total) * 100)) : null;
    return (
      <div className="alert" style={ALERT_STYLE}>
        <div style={{ marginBottom: 8 }}>
          正在下载 v{detected.version}…
          {percent !== null ? ` ${percent}%` : ` ${fmtBytes(downloaded)}`}
        </div>
        <div style={{ height: 6, borderRadius: 3, background: "var(--surface-3)", overflow: "hidden" }}>
          <div
            style={{
              height: "100%",
              width: percent !== null ? `${percent}%` : "30%",
              background: "var(--accent)",
              transition: "width 200ms ease",
            }}
          />
        </div>
      </div>
    );
  }

  if (status === "ready") {
    return (
      <div className="alert warn" style={ALERT_STYLE}>
        <div style={{ fontWeight: 600 }}>更新已就绪</div>
        <div style={{ fontSize: 12, marginTop: 2 }}>
          重启 app 后生效。重启会暂时中断 Claude Code 的当前会话,可在合适的时机再操作。
        </div>
        <div style={{ display: "flex", gap: 8, justifyContent: "flex-end", marginTop: 8 }}>
          <button className="btn btn-primary" type="button" onClick={() => void restart()}>
            <RotateCw size={12} /> 立即重启
          </button>
        </div>
      </div>
    );
  }

  if (status === "error") {
    return (
      <div className="alert err" style={ALERT_STYLE}>
        <div style={{ display: "flex", alignItems: "center", gap: 6, fontWeight: 600 }}>
          <AlertCircle size={14} /> 更新检查失败
        </div>
        {errorMessage && (
          <div style={{ fontSize: 12, marginTop: 2, wordBreak: "break-all" }}>{errorMessage}</div>
        )}
        <div style={{ display: "flex", gap: 8, justifyContent: "flex-end", marginTop: 8 }}>
          <button className="btn" type="button" onClick={() => void check()}>
            <RefreshCw size={12} /> 重试
          </button>
        </div>
      </div>
    );
  }

  return null;
}
