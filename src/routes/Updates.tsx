import {
  AlertCircle,
  CheckCircle2,
  Download,
  ExternalLink,
  RefreshCw,
  RotateCw,
} from "lucide-react";
import { version as VERSION } from "../../package.json";
import logoUrl from "@/assets/logo.png";
import { useUpdater } from "@/hooks/useUpdater";
import { useSettings, useUpdateSettings } from "@/hooks/useSettings";
import { useT } from "@/i18n";
import { openReleasePage } from "@/lib/updater";
import { fmtBytes } from "@/lib/format";
import type { UpdateSource } from "@/types";

export function UpdatesPage() {
  const { t } = useT();
  const { status, check } = useUpdater();
  const settings = useSettings();
  const updateMut = useUpdateSettings();

  // 更新源与设置页共用同一份 settings.update_source。
  // useUpdateSettings 的 onSuccess 已经 invalidate(['settings']), 两边 UI 自动同步 ——
  // 这里绝不能另起本地 state 做副本, 否则切回设置页会看到旧值。
  const source: UpdateSource = settings.data?.update_source ?? "china";

  async function changeSource(next: UpdateSource) {
    if (next === source) return;
    try {
      await updateMut.mutateAsync({ update_source: next });
      // check 的依赖里有 update_source, 新值会带出新的 manifestUrlForSource()
      void check();
    } catch (e) {
      console.warn("[updates] change source failed", e);
    }
  }

  const checking = status === "checking";

  return (
    // updates-flow: 让「有更新」时的日志区吃满剩余视口高度 (见 styles.css)。
    // 其余状态下所有 section 都是 flex-shrink:0, 排版与改造前一致。
    <div className="page-flow updates-flow">
      <div className="flush-section">
        <div className="updates-head">
          <div className="updates-mark">
            <img src={logoUrl} alt="cc-router" />
          </div>
          <div>
            <div style={{ fontSize: 15, fontWeight: 700, letterSpacing: "-0.01em" }}>
              cc-router
            </div>
            <div className="mono" style={{ fontSize: 11.5, color: "var(--ink-3)", marginTop: 2 }}>
              {t("updates.current", { version: VERSION })}
            </div>
          </div>

          <div
            style={{ marginLeft: "auto", display: "flex", alignItems: "center", gap: 10 }}
          >
            <span style={{ fontSize: 11.5, color: "var(--ink-3)" }}>
              {t("updates.source")}
            </span>
            <div className="radio-group">
              {/* 文案复用设置页的 key: 同一份配置在两页显示同样的字, 避免看成两个开关 */}
              <button
                className={source === "international" ? "on" : ""}
                type="button"
                onClick={() => void changeSource("international")}
              >
                {t("settings.update.source.international")}
              </button>
              <button
                className={source === "china" ? "on" : ""}
                type="button"
                onClick={() => void changeSource("china")}
              >
                {t("settings.update.source.china")}
              </button>
            </div>
            <button
              className="btn"
              type="button"
              disabled={checking}
              onClick={() => void check()}
            >
              <RefreshCw size={12} className={checking ? "spin" : undefined} />
              {status === "up_to_date" ? t("about.updater.recheck") : t("about.updater.check")}
            </button>
          </div>
        </div>
        <div style={{ marginTop: 10, fontSize: 11, color: "var(--ink-4)" }}>
          {t("updates.sourceHint")}
        </div>
      </div>

      <UpdaterStatusSection />
    </div>
  );
}

/** 状态机排版层。逻辑与原 About.tsx::UpdaterBlock 一致, 只换成通栏排版。 */
function UpdaterStatusSection() {
  const { t } = useT();
  const { status, detected, progress, errorMessage, check, install, restart } = useUpdater();
  const { data: settings } = useSettings();

  if (status === "available" && detected) {
    const isManual = detected.kind === "manual";
    return (
      // updates-notes-section 是唯一允许纵向 grow 的 section: 它内部再开一层
      // flex column, 把剩余高度整块让给 .update-notes。
      <div className="flush-section updates-notes-section">
        <div
          className="updates-notes-head"
          style={{
            display: "flex",
            alignItems: "center",
            gap: 10,
            marginBottom: 10,
            flexWrap: "wrap",
          }}
        >
          <span style={{ fontSize: 13, fontWeight: 600 }}>
            {t("about.updater.foundNewPrefix")}
            {detected.version}
          </span>
          <span className="pill warn">
            <span className="dot" />
            {t("updates.available")}
          </span>
          {isManual && (
            <span className="mono" style={{ fontSize: 11.5, color: "var(--ink-3)" }}>
              {t("about.updater.debManual")}
            </span>
          )}
          <div style={{ marginLeft: "auto" }}>
            {isManual ? (
              <button
                className="btn primary"
                type="button"
                onClick={() => void openReleasePage(settings?.update_source ?? null)}
              >
                <ExternalLink size={12} /> {t("about.updater.openDownload")}
              </button>
            ) : (
              <button className="btn primary" type="button" onClick={() => void install()}>
                <Download size={12} /> {t("about.updater.installNow")}
              </button>
            )}
          </div>
        </div>
        {detected.body && <div className="update-notes">{detected.body}</div>}
      </div>
    );
  }

  if (status === "downloading" && detected) {
    const total = progress?.total ?? null;
    const downloaded = progress?.downloaded ?? 0;
    const percent = total ? Math.min(100, Math.round((downloaded / total) * 100)) : null;
    return (
      <div className="flush-section">
        <div
          className="mono"
          style={{ fontSize: 11.5, color: "var(--ink-2)", marginBottom: 8 }}
        >
          {t("about.updater.downloadingPrefix")}
          {detected.version}
          {percent !== null ? ` · ${percent}%` : ""} ·{" "}
          {fmtBytes(downloaded)}
          {total ? ` / ${fmtBytes(total)}` : ""}
        </div>
        <div className="update-progress">
          <i style={{ width: percent !== null ? `${percent}%` : "30%" }} />
        </div>
      </div>
    );
  }

  if (status === "ready") {
    return (
      <div className="flush-section">
        <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
          <span className="pill warn">
            <span className="dot" />
            {t("about.updater.ready.title")}
          </span>
          <span style={{ fontSize: 12, color: "var(--ink-3)", flex: 1, minWidth: 240 }}>
            {t("about.updater.ready.desc")}
          </span>
          <button className="btn primary" type="button" onClick={() => void restart()}>
            <RotateCw size={12} /> {t("about.updater.restart")}
          </button>
        </div>
      </div>
    );
  }

  if (status === "error") {
    return (
      <div className="flush-section">
        <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
          <span className="pill err">
            <span className="dot" />
            {t("about.updater.error")}
          </span>
          {errorMessage && (
            <span
              className="mono"
              style={{
                fontSize: 11,
                color: "var(--ink-3)",
                flex: 1,
                minWidth: 200,
                overflow: "hidden",
                textOverflow: "ellipsis",
                whiteSpace: "nowrap",
              }}
              title={errorMessage}
            >
              {errorMessage}
            </span>
          )}
          <button className="btn" type="button" onClick={() => void check()}>
            <RefreshCw size={12} /> {t("about.updater.retry")}
          </button>
        </div>
      </div>
    );
  }

  // idle / checking / up_to_date
  return (
    <div className="flush-section">
      {status === "checking" ? (
        <span className="pill">
          <RefreshCw size={11} className="spin" /> {t("about.updater.checking")}
        </span>
      ) : status === "up_to_date" ? (
        <span className="pill ok">
          <CheckCircle2 size={11} /> {t("about.updater.upToDate")} v{VERSION}
        </span>
      ) : (
        <span className="pill">
          <AlertCircle size={11} /> {t("about.updater.idle")}
        </span>
      )}
    </div>
  );
}
