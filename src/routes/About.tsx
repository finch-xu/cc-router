import {
  Github,
  ExternalLink,
  RefreshCw,
  Download,
  RotateCw,
  AlertCircle,
  AlertTriangle,
  CheckCircle2,
} from "lucide-react";
import { open as openShell } from "@tauri-apps/plugin-shell";
import { version as VERSION } from "../../package.json";
import logoUrl from "@/assets/logo.png";
import { useUpdater } from "@/hooks/useUpdater";
import { useSettings } from "@/hooks/useSettings";
import { useT } from "@/i18n";
import { openReleasePage } from "@/lib/updater";
import { fmtBytes } from "@/lib/format";

const REPO_URL = "https://github.com/finch-xu/cc-router";
const DOCS_URL = "https://github.com/finch-xu/cc-router#readme";

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

        <UpdaterBlock />

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
  const { t } = useT();
  const { status, detected, progress, errorMessage, check, install, restart } = useUpdater();
  const { data: settings } = useSettings();

  if (status === "idle" || status === "checking" || status === "up_to_date") {
    return (
      <div style={ROW_STYLE}>
        <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
          {status === "checking" ? (
            <>
              <RefreshCw size={13} className="spin" /> {t("about.updater.checking")}
            </>
          ) : status === "up_to_date" ? (
            <>
              <CheckCircle2 size={13} /> {t("about.updater.upToDate")}
            </>
          ) : (
            <>{t("about.updater.idle")}</>
          )}
        </span>
        <button
          className="btn"
          type="button"
          disabled={status === "checking"}
          onClick={() => void check()}
        >
          <RefreshCw size={12} />{" "}
          {status === "up_to_date" ? t("about.updater.recheck") : t("about.updater.check")}
        </button>
      </div>
    );
  }

  if (status === "available" && detected) {
    const isManual = detected.kind === "manual";
    return (
      <div className="alert" style={ALERT_STYLE}>
        <div style={{ fontWeight: 600 }}>
          {t("about.updater.foundNewPrefix")}{detected.version}
        </div>
        <div style={{ color: "var(--ink-3)", fontSize: 12, marginTop: 2 }}>
          {t("about.updater.currentPrefix")}{VERSION}
          {isManual ? t("about.updater.debManual") : ""}
        </div>
        {detected.body && <div style={NOTES_STYLE}>{detected.body}</div>}
        <div style={{ display: "flex", gap: 8, justifyContent: "flex-end", marginTop: 6 }}>
          {isManual ? (
            <button
              className="btn btn-primary"
              type="button"
              onClick={() => void openReleasePage(settings?.update_source ?? null)}
            >
              <ExternalLink size={12} /> {t("about.updater.openDownload")}
            </button>
          ) : (
            <button className="btn btn-primary" type="button" onClick={() => void install()}>
              <Download size={12} /> {t("about.updater.installNow")}
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
          {t("about.updater.downloadingPrefix")}{detected.version}…
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
        <div style={{ fontWeight: 600 }}>{t("about.updater.ready.title")}</div>
        <div style={{ fontSize: 12, marginTop: 2 }}>{t("about.updater.ready.desc")}</div>
        <div style={{ display: "flex", gap: 8, justifyContent: "flex-end", marginTop: 8 }}>
          <button className="btn btn-primary" type="button" onClick={() => void restart()}>
            <RotateCw size={12} /> {t("about.updater.restart")}
          </button>
        </div>
      </div>
    );
  }

  if (status === "error") {
    return (
      <div className="alert err" style={ALERT_STYLE}>
        <div style={{ display: "flex", alignItems: "center", gap: 6, fontWeight: 600 }}>
          <AlertCircle size={14} /> {t("about.updater.error")}
        </div>
        {errorMessage && (
          <div style={{ fontSize: 12, marginTop: 2, wordBreak: "break-all" }}>{errorMessage}</div>
        )}
        <div style={{ display: "flex", gap: 8, justifyContent: "flex-end", marginTop: 8 }}>
          <button className="btn" type="button" onClick={() => void check()}>
            <RefreshCw size={12} /> {t("about.updater.retry")}
          </button>
        </div>
      </div>
    );
  }

  return null;
}
