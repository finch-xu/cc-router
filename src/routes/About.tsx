import { Github, ExternalLink } from "lucide-react";
import { open as openShell } from "@tauri-apps/plugin-shell";
import { version as VERSION } from "../../package.json";
import logoUrl from "@/assets/logo.png";

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
