import { Github, ExternalLink, Globe, AlertTriangle } from "lucide-react";
import { open as openShell } from "@tauri-apps/plugin-shell";
import { version as VERSION } from "../../package.json";
import logoUrl from "@/assets/logo.png";
import { useT } from "@/i18n";

const REPO_URL = "https://github.com/finch-xu/cc-router";
const DOCS_URL = "https://ccrouter.app/docs/";
const SITE_URL = "https://ccrouter.app";

export function AboutPage() {
  const { t } = useT();
  return (
    <>
      <div className="page-header">
        <h1>{t("about.title")}</h1>
        <div className="subtitle">{t("about.subtitle")}</div>
      </div>

      <div className="card about-card">
        <div className="about-mark">
          <img src={logoUrl} alt="cc-router" />
        </div>
        <div className="about-name">cc-router</div>
        <div className="about-version">v{VERSION}</div>
        <div className="about-desc">{t("about.description")}</div>

        <div style={{ display: "flex", gap: 8, justifyContent: "center", flexWrap: "wrap" }}>
          <button
            className="btn"
            type="button"
            onClick={() => openShell(REPO_URL).catch(() => {})}
          >
            <Github size={13} /> {t("about.repo")}
          </button>
          <button
            className="btn"
            type="button"
            onClick={() => openShell(DOCS_URL).catch(() => {})}
          >
            <ExternalLink size={12} /> {t("about.docs")}
          </button>
          <button
            className="btn"
            type="button"
            onClick={() => openShell(SITE_URL).catch(() => {})}
          >
            <Globe size={13} /> {t("about.site")}
          </button>
        </div>
        <div className="about-meta">
          <span>© 2026 finch-xu</span>
          <span>·</span>
          <span>MIT License</span>
        </div>
      </div>

      <div className="card disclaimer-card">
        <div className="disclaimer-title">
          <AlertTriangle size={13} />
          {t("about.disclaimer.title")}
        </div>
        <p>{t("about.disclaimer.usage")}</p>
        <p>{t("about.disclaimer.tos")}</p>
        <p>{t("about.disclaimer.warranty")}</p>
      </div>
    </>
  );
}
