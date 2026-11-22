import { useEffect, useMemo, useState } from "react";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Spinner } from "@/components/Spinner";
import { api } from "@/api/tauri";
import { useT } from "@/i18n";
import { useTheme } from "@/hooks/useTheme";
import { errorText } from "@/lib/errorText";
import { fmtBytes, sourceText } from "./format";
import { IssueList } from "./IssueList";
import { checkThemePackage, PluginThemeError } from "@/plugins/theme/apply";
import { themeCss } from "@/plugins/theme/css";
import { localized, type Issue } from "@/plugins/manifest";
import type { InstalledPlugin, StagedPlugin } from "@/types";

const BIG_PACKAGE = 8 * 1024 * 1024;

/**
 * 安装确认框: 暂存好的包 → 前端校验 → 显示来源 / 大小 / 警告 → 试穿 / 安装 / 取消。
 * 关闭 (未安装) 时丢弃暂存; 试穿只在框开着时生效。
 */
export function InstallDialog({
  staged,
  communitySha256,
  onClose,
}: {
  staged: StagedPlugin | null;
  /** 从社区目录安装时传目录里的包哈希, 后端核对 */
  communitySha256?: string;
  onClose: () => void;
}) {
  const { t, locale } = useT();
  const { preview, setPreview, setThemeId, themeId } = useTheme();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [installed, setInstalled] = useState<InstalledPlugin | null>(null);

  const checked = useMemo(() => {
    if (!staged) return null;
    try {
      return { ok: checkThemePackage(staged.manifest, staged.entry), issues: [] as Issue[] };
    } catch (e) {
      return { ok: null, issues: e instanceof PluginThemeError ? e.issues : [], message: errorText(e) };
    }
  }, [staged]);

  useEffect(() => {
    setError(null);
    setInstalled(null);
    setBusy(false);
  }, [staged]);

  const pluginId = checked?.ok?.manifest.id ?? null;
  const tryingOn = !!pluginId && preview?.pluginId === pluginId;

  // 框关掉 / 换了包: 结束试穿
  useEffect(() => {
    return () => setPreview(null);
  }, [staged, setPreview]);

  function close() {
    if (busy) return;
    setPreview(null);
    if (staged && !installed) api.discardStagedPlugin(staged.staging_id).catch(() => {});
    onClose();
  }

  function toggleTryOn() {
    if (!checked?.ok || !pluginId) return;
    if (tryingOn) setPreview(null);
    else setPreview({ pluginId, css: themeCss(pluginId, checked.ok.theme, () => null).full });
  }

  async function install() {
    if (!staged) return;
    setBusy(true);
    setError(null);
    try {
      const rec = await api.installStagedPlugin(staged.staging_id, communitySha256);
      setPreview(null);
      setInstalled(rec);
    } catch (e) {
      setError(t("plugins.install.failed", { error: errorText(e) }));
    } finally {
      setBusy(false);
    }
  }

  const m = checked?.ok?.manifest;
  const name = m ? localized(m.name, locale) : (staged ? String((staged.manifest as { id?: unknown })?.id ?? "") : "");
  const warnings = checked?.ok?.warnings ?? [];

  return (
    <Dialog open={!!staged} onOpenChange={(v) => !v && close()}>
      <DialogContent className="cc-dialog">
        <DialogHeader>
          <DialogTitle>{t("plugins.install.title", { name })}</DialogTitle>
          {m?.description && <DialogDescription>{localized(m.description, locale)}</DialogDescription>}
        </DialogHeader>

        {staged && (
          <dl className="pg-facts">
            {m && (
              <>
                <dt>{t("plugins.install.version")}</dt>
                <dd>{m.version}</dd>
                <dt>{t("plugins.install.author")}</dt>
                <dd>{m.author}</dd>
                {m.license && (
                  <>
                    <dt>{t("plugins.install.license")}</dt>
                    <dd>{m.license}</dd>
                  </>
                )}
              </>
            )}
            <dt>{t("plugins.install.source")}</dt>
            <dd className="mono">{sourceText(t, communitySha256 ? { ...staged.source, kind: "community" } : staged.source)}</dd>
            <dt>{t("plugins.install.size")}</dt>
            <dd>{fmtBytes(staged.total_size)}</dd>
          </dl>
        )}

        {installed ? (
          <div className="pg-alert info">{t("plugins.install.done", { name })}</div>
        ) : (
          <>
            {checked && !checked.ok && (
              <div className="pg-alert err">
                {t("plugins.install.invalid", { count: Math.max(checked.issues.length, 1) })}
                {checked.issues.length > 0 ? <IssueList issues={checked.issues} /> : <div>{checked.message}</div>}
              </div>
            )}
            {staged && checked?.ok && staged.source.kind === "github" && !communitySha256 && (
              <div className="pg-alert warn">{t("plugins.install.unreviewed")}</div>
            )}
            {staged?.installed && checked?.ok && (
              <div className="pg-alert info">
                {t("plugins.install.replaces", { version: staged.installed.version, source: sourceText(t, staged.installed.source) })}
              </div>
            )}
            {staged && staged.total_size > BIG_PACKAGE && <div className="pg-alert warn">{t("plugins.install.big")}</div>}
            {tryingOn && staged && staged.assets.length > 0 && <div className="pg-alert info">{t("plugins.install.assetsAfter")}</div>}
            {warnings.length > 0 && (
              <div className="pg-alert info">
                {t("plugins.install.warnings")}
                <IssueList issues={warnings} />
              </div>
            )}
          </>
        )}
        {error && <div className="pg-alert err">{error}</div>}

        <DialogFooter>
          {installed ? (
            <>
              <button className="btn" type="button" onClick={close}>
                {t("plugins.btn.done")}
              </button>
              {themeId !== `plugin:${installed.id}` && (
                <button
                  className="btn primary"
                  type="button"
                  onClick={() => {
                    setThemeId(`plugin:${installed.id}`);
                    close();
                  }}
                >
                  {t("plugins.btn.useNow")}
                </button>
              )}
            </>
          ) : (
            <>
              <button className="btn" type="button" onClick={close} disabled={busy}>
                {t("common.cancel")}
              </button>
              {checked?.ok && (
                <button className="btn" type="button" onClick={toggleTryOn} disabled={busy}>
                  {tryingOn ? t("plugins.btn.stopTryOn") : t("plugins.btn.tryOn")}
                </button>
              )}
              <button className="btn primary" type="button" onClick={install} disabled={busy || !checked?.ok}>
                {busy && <Spinner />}
                {t("plugins.btn.install")}
              </button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
