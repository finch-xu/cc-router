import { useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Spinner } from "@/components/Spinner";
import { PluginSlip, pluginReceiptCss } from "@/components/receipts/PluginSlip";
import { api } from "@/api/tauri";
import { useT } from "@/i18n";
import { useTheme } from "@/hooks/useTheme";
import { errorText } from "@/lib/errorText";
import { fmtBytes, sourceText } from "./format";
import { IssueList } from "./IssueList";
import { checkThemePackage, PluginThemeError, type CheckedTheme } from "@/plugins/theme/apply";
import { checkReceiptPackage, type CheckedReceipt } from "@/plugins/receipt/apply";
import { sampleReceiptDto } from "@/plugins/receipt/fields";
import { themeCss } from "@/plugins/theme/css";
import { isSystemType, localized, validateManifest, type Issue, type Manifest } from "@/plugins/manifest";
import { CapabilitySummary } from "./CapabilitySummary";
import { pluginLinks } from "@/plugins/links";
import { runtime } from "@/runtime";
import { version as APP_VERSION } from "../../../package.json";
import type { InstalledPlugin, PluginCheckResult, StagedPlugin } from "@/types";

const BIG_PACKAGE = 8 * 1024 * 1024;

/** 系统插件 (厂商等): 清单在前端校验, 内容由 Rust 校验 (暂存结果里的 `system`) */
interface CheckedSystem {
  manifest: Manifest;
  warnings: Issue[];
  system: PluginCheckResult;
}

type Checked =
  | { kind: "theme"; ok: CheckedTheme }
  | { kind: "receipt"; ok: CheckedReceipt }
  | { kind: "system"; ok: CheckedSystem }
  | { kind: "error"; issues: Issue[]; message: string };

function check(staged: StagedPlugin): Checked {
  try {
    const type = (staged.manifest as { type?: unknown } | null)?.type;
    if (typeof type === "string" && isSystemType(type)) {
      const { manifest, errors, warnings } = validateManifest(staged.manifest, APP_VERSION);
      if (!manifest) return { kind: "error", issues: errors, message: "" };
      if (!staged.system) return { kind: "error", issues: [], message: "missing Rust check result" };
      if (!staged.system.ok) return { kind: "error", issues: staged.system.issues, message: "" };
      return { kind: "system", ok: { manifest, warnings, system: staged.system } };
    }
    if (type === "receipt") return { kind: "receipt", ok: checkReceiptPackage(staged.manifest, staged.entry, staged.texts) };
    return { kind: "theme", ok: checkThemePackage(staged.manifest, staged.entry) };
  } catch (e) {
    return { kind: "error", issues: e instanceof PluginThemeError ? e.issues : [], message: errorText(e) };
  }
}

/**
 * 安装确认框: 暂存好的包 → 前端校验 → 显示来源 / 大小 / 警告 → 试穿 (主题) 或样例预览 (小票) → 安装。
 * 关闭 (未安装) 时丢弃暂存; 试穿只在框开着时生效。
 */
export function InstallDialog({
  staged,
  communitySha256,
  onClose,
  onConfigureRules,
}: {
  staged: StagedPlugin | null;
  /** 从社区目录安装时传目录里的包哈希, 后端核对 */
  communitySha256?: string;
  onClose: () => void;
  /** 安装完规则插件后点「去配置」: 切到已安装页并打开参数框 */
  onConfigureRules?: (pluginId: string) => void;
}) {
  const { t, locale } = useT();
  const navigate = useNavigate();
  const { preview, setPreview, setThemeId, themeId } = useTheme();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [installed, setInstalled] = useState<InstalledPlugin | null>(null);

  const checked = useMemo(() => (staged ? check(staged) : null), [staged]);
  const ok = checked && checked.kind !== "error" ? checked.ok : null;

  useEffect(() => {
    setError(null);
    setInstalled(null);
    setBusy(false);
  }, [staged]);

  const pluginId = ok?.manifest.id ?? null;
  const tryingOn = !!pluginId && preview?.pluginId === pluginId;

  // 框关掉 / 换了包: 结束试穿
  useEffect(() => {
    return () => setPreview(null);
  }, [staged, setPreview]);

  // 小票样例预览 (自带字体安装前拿不到, 预览用回退字体)
  const sample = useMemo(() => {
    if (checked?.kind !== "receipt") return null;
    const id = checked.ok.manifest.id;
    return {
      id,
      receipt: checked.ok.receipt,
      css: pluginReceiptCss(id, checked.ok.receipt, () => null),
      dto: sampleReceiptDto(),
      ctx: { port: 23456, version: APP_VERSION, deletedLabel: t("receipts.slip.deletedSub") },
    };
  }, [checked, t]);

  function close() {
    if (busy) return;
    setPreview(null);
    if (staged && !installed) api.discardStagedPlugin(staged.staging_id).catch(() => {});
    onClose();
  }

  function toggleTryOn() {
    if (checked?.kind !== "theme" || !pluginId) return;
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

  function startUsing(rec: InstalledPlugin) {
    if (rec.plugin_type === "provider") navigate(`/subscriptions/new?provider=${encodeURIComponent(`plugin:${rec.id}`)}`);
    else if (rec.plugin_type === "pricing") navigate("/statistics");
    else if (rec.plugin_type === "rules" || rec.plugin_type === "notifier") onConfigureRules?.(rec.id);
    else if (rec.plugin_type === "receipt") navigate(`/receipts?theme=plugin:${rec.id}`);
    else setThemeId(`plugin:${rec.id}`);
    close();
  }

  const m = ok?.manifest;
  const links = staged
    ? pluginLinks({
        homepage: m?.homepage,
        repo: staged.source.kind === "local" ? null : staged.source.location,
        commit: staged.source.commit,
        path: staged.source.path,
      })
    : {};
  const open = (url: string) => () => void runtime.openExternal(url).catch(() => {});
  const name = m ? localized(m.name, locale) : staged ? String((staged.manifest as { id?: unknown })?.id ?? "") : "";
  const warnings = ok ? (checked as { ok: { warnings: Issue[] } }).ok.warnings : [];
  const alreadyInUse = installed && installed.plugin_type === "theme" && themeId === `plugin:${installed.id}`;

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
                <dt>{t("plugins.install.type")}</dt>
                <dd>{t(`plugins.type.${m.type}`)}</dd>
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
                {links.homepage && (
                  <>
                    <dt>{t("plugins.install.homepage")}</dt>
                    <dd>
                      <button className="pg-link" type="button" onClick={open(links.homepage)}>
                        {links.homepage}
                      </button>
                    </dd>
                  </>
                )}
              </>
            )}
            <dt>{t("plugins.install.source")}</dt>
            <dd className="mono">
              {links.source ? (
                <button className="pg-link" type="button" title={links.source} onClick={open(links.source)}>
                  {sourceText(t, communitySha256 ? { ...staged.source, kind: "community" } : staged.source)}
                </button>
              ) : (
                sourceText(t, communitySha256 ? { ...staged.source, kind: "community" } : staged.source)
              )}
            </dd>
            <dt>{t("plugins.install.size")}</dt>
            <dd>{fmtBytes(staged.total_size)}</dd>
          </dl>
        )}

        {checked?.kind === "system" && checked.ok.system.capabilities && !installed && (
          <CapabilitySummary capabilities={checked.ok.system.capabilities} diff={checked.ok.system.diff} />
        )}

        {sample && !installed && (
          <div className="pg-receipt-preview">
            <div className="pg-receipt-preview-label">{t("plugins.install.receiptPreview")}</div>
            <PluginSlip pluginId={sample.id} receipt={sample.receipt} css={sample.css} dto={sample.dto} ctx={sample.ctx} />
          </div>
        )}

        {installed ? (
          <div className="pg-alert info">
            {t("plugins.install.done", { name })}
            {installed.plugin_type === "rules" && (
              <>
                {" "}
                {t("plugins.install.rulesDisabledHint")}
              </>
            )}
            {installed.plugin_type === "notifier" && (
              <>
                {" "}
                {t("plugins.install.notifierDisabledHint")}
              </>
            )}
          </div>
        ) : (
          <>
            {checked?.kind === "error" && (
              <div className="pg-alert err">
                {t("plugins.install.invalid", { count: Math.max(checked.issues.length, 1) })}
                {checked.issues.length > 0 ? <IssueList issues={checked.issues} /> : <div>{checked.message}</div>}
              </div>
            )}
            {staged && ok && staged.source.kind === "github" && !communitySha256 && (
              <div className="pg-alert warn">
                {checked?.kind !== "system"
                  ? t("plugins.install.unreviewed")
                  : checked.ok.manifest.type === "pricing"
                    ? t("plugins.install.unreviewedPricing")
                    : checked.ok.manifest.type === "rules"
                      ? t("plugins.install.unreviewedRules")
                      : checked.ok.manifest.type === "notifier"
                        ? t("plugins.install.unreviewedNotifier")
                        : t("plugins.install.unreviewedProvider")}
              </div>
            )}
            {staged?.installed && ok && (
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
              {!alreadyInUse && (
                <button className="btn primary" type="button" onClick={() => startUsing(installed)}>
                  {installed.plugin_type === "provider"
                    ? t("plugins.btn.addSubscription")
                    : installed.plugin_type === "pricing"
                      ? t("plugins.btn.viewCosts")
                      : installed.plugin_type === "rules" || installed.plugin_type === "notifier"
                        ? t("plugins.btn.configure")
                        : t("plugins.btn.useNow")}
                </button>
              )}
            </>
          ) : (
            <>
              <button className="btn" type="button" onClick={close} disabled={busy}>
                {t("common.cancel")}
              </button>
              {checked?.kind === "theme" && (
                <button className="btn" type="button" onClick={toggleTryOn} disabled={busy}>
                  {tryingOn ? t("plugins.btn.stopTryOn") : t("plugins.btn.tryOn")}
                </button>
              )}
              <button className="btn primary" type="button" onClick={install} disabled={busy || !ok}>
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
