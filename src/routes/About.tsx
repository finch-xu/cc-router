import { ExternalLink, Globe, TriangleAlert } from "lucide-react";
import Github from "@lobehub/icons/es/Github";
import { runtime } from "@/runtime";
import { version as VERSION } from "../../package.json";
import logoUrl from "@/assets/logo.png";
import { useT } from "@/i18n";

const REPO_URL = "https://github.com/finch-xu/cc-router";
const DOCS_URL = "https://ccrouter.app/docs/";
const SITE_URL = "https://ccrouter.app";

/** 关于页: 与设置页同一套「页头 + 全宽卡片」排版, 不再居中压窄. */
export function AboutPage() {
  const { t } = useT();
  return (
    <>
      <div className="page-header">
        <h1>{t("about.title")}</h1>
        <div className="subtitle">{t("about.subtitle")}</div>
      </div>

      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("about.section.app")}</div>
        </div>
        <div className="card-body">
          <div className="about-hero">
            <div className="app-mark">
              <img src={logoUrl} alt="cc-router" />
            </div>
            <div style={{ minWidth: 0 }}>
              <div className="about-name">cc-router</div>
              <div className="about-meta">
                <span>v{VERSION}</span>
                <span>·</span>
                <span>MIT License</span>
                <span>·</span>
                <span>© 2026 finch-xu</span>
              </div>
              <div className="about-desc">{t("about.description")}</div>
              <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                <button
                  className="btn"
                  type="button"
                  onClick={() => runtime.openExternal(REPO_URL).catch(() => {})}
                >
                  <Github size={13} /> {t("about.repo")}
                </button>
                <button
                  className="btn"
                  type="button"
                  onClick={() => runtime.openExternal(DOCS_URL).catch(() => {})}
                >
                  <ExternalLink size={12} /> {t("about.docs")}
                </button>
                <button
                  className="btn"
                  type="button"
                  onClick={() => runtime.openExternal(SITE_URL).catch(() => {})}
                >
                  <Globe size={13} /> {t("about.site")}
                </button>
              </div>
            </div>
          </div>
        </div>
      </div>

      <div className="card section">
        <div className="card-head">
          <div className="card-title" style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <TriangleAlert size={13} style={{ color: "var(--warn)" }} />
            {t("about.disclaimer.title")}
          </div>
        </div>
        <div className="card-body about-disclaimer">
          <p>{t("about.disclaimer.usage")}</p>
          <p>{t("about.disclaimer.tos")}</p>
          <p>{t("about.disclaimer.warranty")}</p>
        </div>
      </div>
    </>
  );
}
