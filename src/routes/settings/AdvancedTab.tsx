import { useState } from "react";
import { TriangleAlert } from "lucide-react";
import { Toggle } from "@/components/Toggle";
import { Spinner } from "@/components/Spinner";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { api } from "@/api/tauri";
import { useT } from "@/i18n";
import { runtime } from "@/runtime";
import { useStorageStats } from "@/hooks/useStorageStats";
import { fmtBytes, fmtNum } from "@/lib/format";
import type { SettingsForm } from "./useSettingsForm";

/** 高级: 数据存储 / 调试 / 危险区域 (恢复出厂). 危险区固定放最底部, 保持不容易顺手点到. */
export function AdvancedTab({ form }: { form: SettingsForm }) {
  const { t } = useT();
  const storage = useStorageStats();
  const [clearDumpsDialog, setClearDumpsDialog] = useState(false);
  const [clearingDumps, setClearingDumps] = useState(false);
  const [resetDialog, setResetDialog] = useState(false);
  const [resetting, setResetting] = useState(false);

  async function openDumps() {
    try {
      await api.openDebugDumpDir();
    } catch (e) {
      alert(`${t("settings.debug.open.alertFailed")}: ${e}`);
    }
  }

  async function confirmClearDumps() {
    setClearingDumps(true);
    try {
      await api.clearDebugDumps();
      setClearDumpsDialog(false);
    } catch (e) {
      alert(`${t("settings.debug.clear.alertFailed")}: ${e}`);
    } finally {
      setClearingDumps(false);
    }
  }

  async function confirmReset() {
    setResetting(true);
    try {
      await api.factoryReset();
      // app 会自动重启;这里不会真正 resolve
    } catch (e) {
      setResetting(false);
      setResetDialog(false);
      alert(`${t("settings.danger.alertFailed")}: ${e}`);
    }
  }

  return (
    <>
      {/* 数据存储 */}
      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("settings.section.storage")}</div>
        </div>
        <div className="card-body">
          <div className="setting-row">
            <div className="label-col">{t("settings.storage.retention.label")}</div>
            <Select
              value={String(form.retentionDays >= 36500 ? 0 : form.retentionDays)}
              onValueChange={(v) => void form.changeRetentionDays(Number(v))}
            >
              <SelectTrigger style={{ maxWidth: 200 }}>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="7">{t("settings.storage.retention.7d")}</SelectItem>
                <SelectItem value="30">{t("settings.storage.retention.30d")}</SelectItem>
                <SelectItem value="90">{t("settings.storage.retention.90d")}</SelectItem>
                <SelectItem value="0">{t("settings.storage.retention.forever")}</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="setting-row">
            <div className="label-col">{t("settings.storage.dbLimit.label")}</div>
            <Select
              value={String(form.dbLimitMb)}
              onValueChange={(v) => void form.changeDbLimit(Number(v))}
            >
              <SelectTrigger style={{ maxWidth: 200 }}>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="100">100 MB</SelectItem>
                <SelectItem value="500">500 MB</SelectItem>
                <SelectItem value="1024">1 GB</SelectItem>
                <SelectItem value="10240">{t("settings.storage.dbLimit.unlimited")}</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="field-hint" style={{ marginTop: 4 }}>
            {storage.data
              ? t("settings.storage.usage", {
                  size: fmtBytes(storage.data.db_bytes),
                  requests: fmtNum(storage.data.requests_rows),
                  events: fmtNum(storage.data.events_rows),
                })
              : t("settings.storage.usageLoading")}
          </div>
        </div>
      </div>

      {/* 调试 */}
      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("settings.section.debug")}</div>
        </div>
        <div className="card-body">
          <div className="setting-row">
            <div className="label-col">
              {t("settings.debug.mode.label")}
              <div className="desc">{t("settings.debug.mode.desc")}</div>
            </div>
            <Toggle
              checked={form.debugMode}
              onChange={(v) => void form.changeDebugMode(v)}
              aria-label={t("settings.debug.mode.label")}
            />
          </div>
          <div className="setting-row">
            <div className="label-col">
              {t("settings.debug.dumps.label")}
              <div className="desc">{t("settings.debug.dumps.desc")}</div>
            </div>
            <div style={{ display: "flex", gap: 8 }}>
              {runtime.kind === "desktop" ? (
                <button className="btn" type="button" onClick={openDumps}>
                  {t("settings.debug.open.button")}
                </button>
              ) : (
                <span className="desc">{t("settings.debug.open.webHint")}</span>
              )}
              <button className="btn" type="button" onClick={() => setClearDumpsDialog(true)}>
                {t("settings.debug.clear.button")}
              </button>
            </div>
          </div>
        </div>
      </div>

      {/* 危险区域 */}
      <div className="danger-card section">
        <div>
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 8,
              fontSize: 13,
              fontWeight: 600,
              color: "oklch(0.42 0.16 28)",
              marginBottom: 6,
            }}
          >
            <TriangleAlert size={14} /> {t("settings.section.danger")}
          </div>
          <div style={{ fontSize: 12, color: "var(--ink-3)", lineHeight: 1.6 }}>
            {t("settings.danger.desc")}
          </div>
        </div>
        <button className="btn danger" type="button" onClick={() => setResetDialog(true)}>
          {t("settings.danger.button")}
        </button>
      </div>

      {/* 清空 dump 确认弹窗 */}
      <Dialog
        open={clearDumpsDialog}
        onOpenChange={(v) => !clearingDumps && setClearDumpsDialog(v)}
      >
        <DialogContent className="cc-dialog">
          <DialogHeader>
            <DialogTitle>{t("settings.debug.clear.dialog.title")}</DialogTitle>
            <DialogDescription>{t("settings.debug.clear.dialog.desc")}</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <button
              className="btn"
              onClick={() => setClearDumpsDialog(false)}
              disabled={clearingDumps}
              type="button"
            >
              {t("common.cancel")}
            </button>
            <button
              className="btn danger"
              onClick={confirmClearDumps}
              disabled={clearingDumps}
              type="button"
            >
              {clearingDumps && <Spinner />}
              {t("settings.debug.clear.dialog.confirm")}
            </button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* 恢复出厂确认弹窗 */}
      <Dialog open={resetDialog} onOpenChange={(v) => !resetting && setResetDialog(v)}>
        <DialogContent className="cc-dialog">
          <DialogHeader>
            <DialogTitle>
              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <TriangleAlert size={16} style={{ color: "var(--err)" }} />
                {t("settings.danger.dialog.title")}
              </div>
            </DialogTitle>
            <DialogDescription asChild>
              <div>
                {t("settings.danger.dialog.intro")}
                <ul style={{ marginTop: 8, paddingLeft: 20, fontSize: 13, lineHeight: 1.7 }}>
                  <li>{t("settings.danger.dialog.item.subscriptions")}</li>
                  <li>{t("settings.danger.dialog.item.virtualModels")}</li>
                  <li>{t("settings.danger.dialog.item.logs")}</li>
                  <li>{t("settings.danger.dialog.item.settings")}</li>
                </ul>
                <p style={{ marginTop: 12 }}>{t("settings.danger.dialog.outro")}</p>
              </div>
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <button
              className="btn"
              onClick={() => setResetDialog(false)}
              disabled={resetting}
              type="button"
            >
              {t("common.cancel")}
            </button>
            <button
              className="btn danger"
              onClick={confirmReset}
              disabled={resetting}
              type="button"
            >
              {resetting && <Spinner />}
              {t("settings.danger.dialog.confirm")}
            </button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
