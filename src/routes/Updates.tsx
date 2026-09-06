import {
  CircleAlert,
  CircleCheck,
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

/** 检查更新页: 与设置页同一套「页头 + 卡片 + setting-row」排版. */
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
    <>
      <div className="page-header">
        <h1>{t("updates.title")}</h1>
        <div className="subtitle">{t("updates.subtitle")}</div>
      </div>

      {/* 版本与更新源 */}
      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("updates.section.version")}</div>
        </div>
        <div className="card-body">
          <div className="setting-row">
            <div className="label-col">{t("updates.row.current")}</div>
            <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
              <div className="app-mark small">
                <img src={logoUrl} alt="cc-router" />
              </div>
              <span className="mono" style={{ fontSize: 12.5, color: "var(--ink-2)" }}>
                cc-router v{VERSION}
              </span>
            </div>
          </div>
          <div className="setting-row">
            <div className="label-col">
              {t("updates.source")}
              <div className="desc">{t("updates.sourceHint")}</div>
            </div>
            {/* 套一层 div: setting-row 的网格会把直接子元素拉满整列 */}
            <div>
              <div className="radio-group" style={{ display: "inline-flex" }}>
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
            </div>
          </div>
          <div className="setting-row">
            <div className="label-col">{t("updates.row.check")}</div>
            <div>
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
        </div>
      </div>

      {/* 更新状态 */}
      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("updates.section.status")}</div>
          <StatusPill />
        </div>
        <div className="card-body">
          <UpdaterStatusSection />
        </div>
      </div>
    </>
  );
}

/** card-head 右侧的状态胶囊, 五种状态各一种颜色. */
function StatusPill() {
  const { t } = useT();
  const { status } = useUpdater();
  switch (status) {
    case "checking":
      return (
        <span className="pill">
          <RefreshCw size={11} className="spin" /> {t("about.updater.checking")}
        </span>
      );
    case "up_to_date":
      return (
        <span className="pill ok">
          <CircleCheck size={11} /> {t("about.updater.upToDate")}
        </span>
      );
    case "available":
    case "downloading":
    case "ready":
      return (
        <span className="pill warn">
          <span className="dot" />
          {status === "ready" ? t("about.updater.ready.title") : t("updates.available")}
        </span>
      );
    case "error":
      return (
        <span className="pill err">
          <span className="dot" />
          {t("about.updater.error")}
        </span>
      );
    default:
      return (
        <span className="pill">
          <CircleAlert size={11} /> {t("about.updater.idle")}
        </span>
      );
  }
}

/** 状态机排版层. 状态本身已由 card-head 的 StatusPill 表达, 这里只放各状态的正文与动作. */
function UpdaterStatusSection() {
  const { t } = useT();
  const { status, detected, progress, errorMessage, check, install, restart } = useUpdater();
  const { data: settings } = useSettings();

  if (status === "available" && detected) {
    const isManual = detected.kind === "manual";
    return (
      <>
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 10,
            marginBottom: 12,
            flexWrap: "wrap",
          }}
        >
          <span style={{ fontSize: 13, fontWeight: 600 }}>
            {t("about.updater.foundNewPrefix")}
            {detected.version}
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
      </>
    );
  }

  if (status === "downloading" && detected) {
    const total = progress?.total ?? null;
    const downloaded = progress?.downloaded ?? 0;
    const percent = total ? Math.min(100, Math.round((downloaded / total) * 100)) : null;
    return (
      <>
        <div className="mono" style={{ fontSize: 11.5, color: "var(--ink-2)", marginBottom: 8 }}>
          {t("about.updater.downloadingPrefix")}
          {detected.version}
          {percent !== null ? ` · ${percent}%` : ""} · {fmtBytes(downloaded)}
          {total ? ` / ${fmtBytes(total)}` : ""}
        </div>
        <div className="update-progress">
          <i style={{ width: percent !== null ? `${percent}%` : "30%" }} />
        </div>
      </>
    );
  }

  if (status === "ready") {
    return (
      <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
        <span style={{ fontSize: 12, color: "var(--ink-3)", flex: 1, minWidth: 240 }}>
          {t("about.updater.ready.desc")}
        </span>
        <button className="btn primary" type="button" onClick={() => void restart()}>
          <RotateCw size={12} /> {t("about.updater.restart")}
        </button>
      </div>
    );
  }

  if (status === "error") {
    return (
      <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
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
          title={errorMessage ?? undefined}
        >
          {errorMessage ?? t("about.updater.error")}
        </span>
        <button className="btn" type="button" onClick={() => void check()}>
          <RefreshCw size={12} /> {t("about.updater.retry")}
        </button>
      </div>
    );
  }

  // idle / checking / up_to_date: 状态已在 pill 里, 正文给一句说明即可
  return (
    <div style={{ fontSize: 12, color: "var(--ink-3)" }}>
      {status === "up_to_date"
        ? t("updates.status.upToDateDesc", { version: VERSION })
        : status === "checking"
          ? t("about.updater.checking")
          : t("updates.status.idleDesc")}
    </div>
  );
}
