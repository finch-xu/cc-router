import { useMemo, useState } from "react";
import { ExternalLink, FolderOpen, Link2, Palette, Puzzle, Receipt } from "lucide-react";
import { useNavigate } from "react-router";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { EmptyState } from "@/components/EmptyState";
import { Spinner } from "@/components/Spinner";
import { InstallDialog } from "@/components/plugins/InstallDialog";
import { LinkDialog } from "@/components/plugins/LinkDialog";
import { fmtBytes, sourceText } from "@/components/plugins/format";
import "@/components/plugins/plugins.css";
import { api } from "@/api/tauri";
import { useT } from "@/i18n";
import { useTheme } from "@/hooks/useTheme";
import { useCommunityIndex, useInstalledPlugins } from "@/hooks/usePlugins";
import { errorText } from "@/lib/errorText";
import { runtime } from "@/runtime";
import { COMMUNITY_URL, entryLink, hasUpdate, type CommunityEntry } from "@/plugins/community";
import { localized, type LocalizedText } from "@/plugins/manifest";
import type { InstalledPlugin, StagedPlugin } from "@/types";

type Tab = "community" | "installed";
type Kind = "all" | "theme" | "receipt";

/** 清单原文里的名字 / 介绍 (已安装列表用; 内容在安装时已校验过) */
function manifestText(m: unknown, key: "name" | "description"): LocalizedText | undefined {
  const v = (m as Record<string, unknown> | null)?.[key];
  return typeof v === "string" || (typeof v === "object" && v !== null) ? (v as LocalizedText) : undefined;
}

export function PluginsPage() {
  const { t, locale } = useT();
  const isDesktop = runtime.kind === "desktop";
  const installed = useInstalledPlugins();
  const community = useCommunityIndex(isDesktop);
  const { themeId, setThemeId } = useTheme();

  const [tab, setTab] = useState<Tab>(isDesktop ? "community" : "installed");
  const [query, setQuery] = useState("");
  const [tag, setTag] = useState<string | null>(null);
  const [kind, setKind] = useState<Kind>("all");
  const navigate = useNavigate();
  const [linkOpen, setLinkOpen] = useState(false);
  const [staged, setStaged] = useState<{ s: StagedPlugin; sha?: string } | null>(null);
  const [detail, setDetail] = useState<CommunityEntry | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const installedById = useMemo(() => new Map((installed.data ?? []).map((p) => [p.id, p])), [installed.data]);
  const communityById = useMemo(() => new Map((community.data ?? []).map((e) => [e.id, e])), [community.data]);

  const entries = useMemo(() => {
    const q = query.trim().toLowerCase();
    return (community.data ?? []).filter((e) => {
      if (e.revoked) return false;
      if (kind !== "all" && e.type !== kind) return false;
      if (tag && !e.tags.includes(tag)) return false;
      if (!q) return true;
      const hay = [localized(e.name, locale), localized(e.description, locale), e.author, e.id, ...e.tags].join(" ").toLowerCase();
      return hay.includes(q);
    });
  }, [community.data, query, tag, locale, kind]);
  const installedList = (installed.data ?? []).filter((p) => kind === "all" || p.plugin_type === kind);
  const allTags = useMemo(() => [...new Set((community.data ?? []).flatMap((e) => e.tags))].sort(), [community.data]);

  async function stage(run: () => Promise<StagedPlugin>, key: string, sha?: string) {
    setBusyId(key);
    setError(null);
    try {
      const s = await run();
      setDetail(null);
      setStaged({ s, sha });
    } catch (e) {
      setError(t("plugins.fetchFailed", { error: errorText(e) }));
    } finally {
      setBusyId(null);
    }
  }

  const installFromCommunity = (e: CommunityEntry) => stage(() => api.stagePluginFromGithub(entryLink(e)), e.id, e.latest.sha256);

  async function fromFolder() {
    const path = await runtime.pickFolder().catch(() => null);
    if (path) await stage(() => api.stagePluginFromDir(path), "__folder__");
  }

  async function uninstall(p: InstalledPlugin) {
    const name = localized(manifestText(p.manifest, "name"), locale) || p.id;
    if (!window.confirm(t("plugins.uninstall.confirm", { name }))) return;
    setBusyId(p.id);
    setError(null);
    try {
      if (themeId === `plugin:${p.id}`) setThemeId("quiet");
      await api.uninstallPlugin(p.id);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusyId(null);
    }
  }

  const badgeFor = (id: string, e?: CommunityEntry) => {
    const ins = installedById.get(id);
    if (!ins) return null;
    if (themeId === `plugin:${id}`) return <span className="pg-badge acc">{t("plugins.badge.current")}</span>;
    if (e && ins.source.kind === "community" && hasUpdate(ins.version, e)) return <span className="pg-badge acc">{t("plugins.badge.update")}</span>;
    return <span className="pg-badge ok">{t("plugins.badge.installed")}</span>;
  };

  return (
    <div className="pg-page">
      <div className="page-actions">
        <div className="page-header" style={{ marginBottom: 0 }}>
          <h1>{t("plugins.title")}</h1>
          <div className="subtitle">{t("plugins.subtitle")}</div>
        </div>
        {isDesktop && (
          <div className="pg-actions">
            <button className="btn" type="button" onClick={fromFolder} disabled={busyId !== null}>
              {busyId === "__folder__" ? <Spinner /> : <FolderOpen size={13} />} {t("plugins.action.fromFolder")}
            </button>
            <button className="btn primary" type="button" onClick={() => setLinkOpen(true)}>
              <Link2 size={13} /> {t("plugins.action.fromLink")}
            </button>
          </div>
        )}
      </div>

      {!isDesktop && <div className="pg-note">{t("plugins.webOnly")}</div>}
      {error && <div className="pg-alert err">{error}</div>}

      <div className="pg-toolbar">
        {isDesktop && (
          <div className="sb-chips" role="tablist">
            {(["community", "installed"] as const).map((k) => (
              <button key={k} type="button" role="tab" aria-selected={tab === k} className={"sb-chip" + (tab === k ? " active" : "")} onClick={() => setTab(k)}>
                {t(`plugins.tab.${k}`)}
                {k === "installed" && installed.data ? <b> {installed.data.length}</b> : null}
              </button>
            ))}
          </div>
        )}
        <div className="sb-chips" role="tablist">
          {(["all", "theme", "receipt"] as const).map((k) => (
            <button key={k} type="button" role="tab" aria-selected={kind === k} className={"sb-chip" + (kind === k ? " active" : "")} onClick={() => setKind(k)}>
              {k === "all" ? t("plugins.kind.all") : t(`plugins.type.${k}`)}
            </button>
          ))}
        </div>
        {tab === "community" && (
          <>
            <input className="input" placeholder={t("plugins.search")} value={query} onChange={(e) => setQuery(e.target.value)} />
            {allTags.length > 0 && (
              <div className="pg-tags">
                {allTags.map((g) => (
                  <button key={g} type="button" className={"sb-chip" + (tag === g ? " active" : "")} onClick={() => setTag(tag === g ? null : g)}>
                    {g}
                  </button>
                ))}
              </div>
            )}
          </>
        )}
      </div>

      {tab === "community" && (
        <>
          {community.isLoading && <div className="field-hint">{t("plugins.community.loading")}</div>}
          {community.isError && <div className="pg-alert err">{t("plugins.community.failed", { error: errorText(community.error) })}</div>}
          {community.data && entries.length === 0 && (
            <EmptyState
              icon={Palette}
              message={community.data.length === 0 ? t("plugins.community.empty") : t("plugins.community.noMatch")}
              action={
                <button className="btn sm" type="button" onClick={() => runtime.openExternal(COMMUNITY_URL).catch(() => {})}>
                  <ExternalLink size={12} /> {t("plugins.community.submit")}
                </button>
              }
            />
          )}
          {entries.length > 0 && (
            <div className="pg-grid">
              {entries.map((e) => (
                <button key={e.id} type="button" className="card pg-card" onClick={() => setDetail(e)}>
                  {e.screenshots[0] ? (
                    <img className="pg-shot" src={e.screenshots[0]} alt="" loading="lazy" />
                  ) : (
                    <div className="pg-shot empty">{e.type === "receipt" ? <Receipt size={22} /> : <Palette size={22} />}</div>
                  )}
                  <div className="pg-card-body">
                    <div className="pg-title">
                      {e.logo && <img src={e.logo} alt="" loading="lazy" />}
                      <b>{localized(e.name, locale)}</b>
                      {badgeFor(e.id, e)}
                    </div>
                    <div className="pg-by">
                      {t(`plugins.type.${e.type}`)} · {e.author} · v{e.latest.version}
                    </div>
                    {e.description && <div className="pg-desc">{localized(e.description, locale)}</div>}
                    {e.tags.length > 0 && (
                      <div className="pg-tags">
                        {e.tags.map((g) => (
                          <span key={g} className="pg-tag">
                            {g}
                          </span>
                        ))}
                      </div>
                    )}
                  </div>
                </button>
              ))}
            </div>
          )}
        </>
      )}

      {tab === "installed" && (
        <>
          {installed.data && installedList.length === 0 && <EmptyState icon={Puzzle} message={t("plugins.installed.empty")} />}
          <div className="pg-list">
            {installedList.map((p) => {
              const e = communityById.get(p.id);
              const inUse = themeId === `plugin:${p.id}`;
              const canUpdate = isDesktop && e && p.source.kind === "community" && hasUpdate(p.version, e);
              const revoked = e?.revoked && p.source.kind === "community" ? e.revoked : null;
              return (
                <div key={p.id} className="card pg-row">
                  <div className="pg-row-main">
                    <div className="pg-title">
                      <b>{localized(manifestText(p.manifest, "name"), locale) || p.id}</b>
                      {inUse && <span className="pg-badge acc">{t("plugins.badge.current")}</span>}
                      {revoked && <span className="pg-badge err">{t("plugins.badge.revoked")}</span>}
                    </div>
                    <div className="pg-meta">
                      <span>{t(`plugins.type.${p.plugin_type}`)}</span>
                      <span>v{p.version}</span>
                      <span>{fmtBytes(p.size)}</span>
                      <span className="mono">{sourceText(t, p.source)}</span>
                    </div>
                    {revoked && <div className="pg-alert err">{t("plugins.detail.revoked", { reason: revoked })}</div>}
                  </div>
                  <div className="pg-row-actions">
                    {canUpdate && (
                      <button className="btn sm" type="button" disabled={busyId !== null} onClick={() => installFromCommunity(e)}>
                        {busyId === p.id ? <Spinner /> : null} {t("plugins.btn.update")} v{e.latest.version}
                      </button>
                    )}
                    {p.plugin_type === "receipt" && (
                      <button className="btn sm primary" type="button" onClick={() => navigate(`/receipts?theme=plugin:${p.id}`)}>
                        {t("plugins.btn.openReceipt")}
                      </button>
                    )}
                    {p.plugin_type === "theme" && (
                      <button className="btn sm primary" type="button" disabled={inUse} onClick={() => setThemeId(`plugin:${p.id}`)}>
                        {inUse ? t("plugins.btn.inUse") : t("plugins.btn.use")}
                      </button>
                    )}
                    {isDesktop && (
                      <button className="btn sm danger" type="button" disabled={busyId !== null} onClick={() => uninstall(p)}>
                        {t("plugins.btn.uninstall")}
                      </button>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        </>
      )}

      {/* 社区条目详情 */}
      <Dialog open={!!detail} onOpenChange={(v) => !v && busyId === null && setDetail(null)}>
        <DialogContent className="cc-dialog">
          {detail && (
            <>
              <DialogHeader>
                <DialogTitle>{localized(detail.name, locale)}</DialogTitle>
                {detail.description && <DialogDescription>{localized(detail.description, locale)}</DialogDescription>}
              </DialogHeader>
              {detail.screenshots.length > 0 && (
                <div className="pg-shots">
                  {detail.screenshots.map((u) => (
                    <img key={u} src={u} alt="" />
                  ))}
                </div>
              )}
              <dl className="pg-facts">
                <dt>{t("plugins.install.author")}</dt>
                <dd>{detail.author}</dd>
                <dt>{t("plugins.install.version")}</dt>
                <dd>{detail.latest.version}</dd>
                {detail.license && (
                  <>
                    <dt>{t("plugins.install.license")}</dt>
                    <dd>{detail.license}</dd>
                  </>
                )}
                <dt>{t("plugins.install.source")}</dt>
                <dd className="mono">
                  {detail.repo}
                  {detail.path ? `/${detail.path}` : ""} @ {detail.latest.commit.slice(0, 7)}
                </dd>
              </dl>
              <DialogFooter>
                <button className="btn" type="button" onClick={() => runtime.openExternal(`https://github.com/${detail.repo}`).catch(() => {})}>
                  <ExternalLink size={12} /> {t("plugins.btn.repo")}
                </button>
                <button className="btn primary" type="button" disabled={busyId !== null} onClick={() => installFromCommunity(detail)}>
                  {busyId === detail.id && <Spinner />}
                  {installedById.has(detail.id) ? t("plugins.btn.reinstall") : t("plugins.btn.install")}
                </button>
              </DialogFooter>
            </>
          )}
        </DialogContent>
      </Dialog>

      <LinkDialog open={linkOpen} onOpenChange={setLinkOpen} onStaged={(s) => setStaged({ s })} />
      <InstallDialog staged={staged?.s ?? null} communitySha256={staged?.sha} onClose={() => setStaged(null)} />
    </div>
  );
}
