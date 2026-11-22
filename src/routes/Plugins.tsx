import { useMemo, useState } from "react";
import { Coins, ExternalLink, FolderOpen, Link2, Palette, Plug, Puzzle, Receipt } from "lucide-react";
import Github from "@lobehub/icons/es/Github";
import { useNavigate } from "react-router";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { EmptyState } from "@/components/EmptyState";
import { Spinner } from "@/components/Spinner";
import { InstallDialog } from "@/components/plugins/InstallDialog";
import { LinkDialog } from "@/components/plugins/LinkDialog";
import { SparrowBakeryLogo } from "@/components/plugins/SparrowBakeryLogo";
import { fmtBytes, sourceText } from "@/components/plugins/format";
import "@/components/plugins/plugins.css";
import { api } from "@/api/tauri";
import { useT } from "@/i18n";
import { useTheme } from "@/hooks/useTheme";
import { useCommunityIndex, useInstalledPlugins } from "@/hooks/usePlugins";
import { errorText } from "@/lib/errorText";
import { runtime } from "@/runtime";
import { COMMUNITY_URL, entryLink, hasUpdate, type CommunityEntry } from "@/plugins/community";
import { isSystemType, localized, type LocalizedText } from "@/plugins/manifest";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { pluginLinks, type PluginLinks } from "@/plugins/links";
import type { InstalledPlugin, StagedPlugin } from "@/types";

type Tab = "community" | "installed";
type Kind = "all" | "theme" | "receipt" | "provider" | "pricing";
/** 类型 chip 分两组: 外观 (主题 / 小票) 与功能 (系统插件)。某类型两边都没有条目就不显示 */
const LOOK_KINDS = ["theme", "receipt"] as const;
const FEATURE_KINDS = ["provider", "pricing"] as const;

/** 清单原文里的名字 / 介绍 (已安装列表用; 内容在安装时已校验过) */
function manifestText(m: unknown, key: "name" | "description"): LocalizedText | undefined {
  const v = (m as Record<string, unknown> | null)?.[key];
  return typeof v === "string" || (typeof v === "object" && v !== null) ? (v as LocalizedText) : undefined;
}

/** 已安装插件的介绍 / 源码链接 (清单原文的 homepage 在渲染前重新校验) */
function installedLinks(p: InstalledPlugin): PluginLinks {
  const homepage = (p.manifest as Record<string, unknown> | null)?.homepage;
  const fromGithub = p.source.kind === "github" || p.source.kind === "community";
  return pluginLinks({
    homepage,
    repo: fromGithub ? p.source.location : null,
    commit: p.source.commit,
    path: p.source.path,
  });
}

export function PluginsPage() {
  const { t, locale } = useT();
  const isDesktop = runtime.kind === "desktop";
  const installed = useInstalledPlugins();
  const subscriptions = useSubscriptions();
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
    setBusyId(`__confirm:${p.id}`);
    setError(null);
    try {
      const inUse =
        p.plugin_type === "provider" ? (subscriptions.data ?? []).filter((s) => s.provider_id === `plugin:${p.id}`).length : 0;
      const message = inUse > 0 ? t("plugins.uninstall.confirmProvider", { name, count: inUse }) : t("plugins.uninstall.confirm", { name });
      if (!(await runtime.confirm(message))) return;
      setBusyId(p.id);
      if (themeId === `plugin:${p.id}`) setThemeId("quiet");
      await api.uninstallPlugin(p.id);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusyId(null);
    }
  }

  async function toggleEnabled(p: InstalledPlugin) {
    setBusyId(p.id);
    setError(null);
    try {
      if (p.enabled) await api.disablePlugin(p.id);
      else await api.enablePlugin(p.id);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusyId(null);
    }
  }

  const presentKinds = new Set<string>([
    ...(community.data ?? []).map((e) => e.type),
    ...(installed.data ?? []).map((p) => p.plugin_type),
  ]);
  const lookKinds = LOOK_KINDS.filter((k) => presentKinds.has(k));
  const featureKinds = FEATURE_KINDS.filter((k) => presentKinds.has(k));

  const badgeFor = (id: string, e?: CommunityEntry) => {
    const ins = installedById.get(id);
    if (!ins) return null;
    if (themeId === `plugin:${id}`) return <span className="pg-badge acc">{t("plugins.badge.current")}</span>;
    if (e && ins.source.kind === "community" && hasUpdate(ins.version, e)) return <span className="pg-badge acc">{t("plugins.badge.update")}</span>;
    return <span className="pg-badge ok">{t("plugins.badge.installed")}</span>;
  };

  const detailLinks: PluginLinks = detail
    ? pluginLinks({ homepage: detail.homepage, repo: detail.repo, commit: detail.latest.commit, path: detail.path })
    : {};

  return (
    <div className="pg-page">
      <section className="card pg-hero">
        <div className="pg-hero-top">
          <div className="pg-hero-brand">
            <div className="pg-hero-title-row">
              <h1 className="pg-hero-h1">
                <SparrowBakeryLogo className="pg-hero-logo" />
                <span className="sr-only">{t("plugins.hero.brand")}</span>
              </h1>
              <span className="pg-hero-kicker">{t("plugins.hero.kicker")}</span>
            </div>
            <p className="pg-hero-desc">{t("plugins.hero.desc")}</p>
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
        <div className="pg-hero-create">
          <span className="pg-hero-create-text">{t("plugins.hero.createHint")}</span>
          <button
            className="btn sm"
            type="button"
            onClick={() => runtime.openExternal(COMMUNITY_URL).catch(() => {})}
          >
            <Github size={13} /> {t("plugins.hero.openRepo")} <ExternalLink size={11} />
          </button>
        </div>
      </section>

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
          {(["all", ...lookKinds, ...featureKinds] as Kind[]).map((k, i) => (
            <span key={k} style={{ display: "contents" }}>
              {i > 0 && i === 1 + lookKinds.length && featureKinds.length > 0 && lookKinds.length > 0 && (
                <span className="pg-chip-sep" aria-hidden="true" />
              )}
              <button type="button" role="tab" aria-selected={kind === k} className={"sb-chip" + (kind === k ? " active" : "")} onClick={() => setKind(k)}>
                {k === "all" ? t("plugins.kind.all") : t(`plugins.type.${k}`)}
              </button>
            </span>
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
                    <div className="pg-shot empty">
                      {e.type === "receipt" ? (
                        <Receipt size={22} />
                      ) : e.type === "provider" ? (
                        <Plug size={22} />
                      ) : e.type === "pricing" ? (
                        <Coins size={22} />
                      ) : (
                        <Palette size={22} />
                      )}
                    </div>
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
              const links = installedLinks(p);
              return (
                <div key={p.id} className="card pg-row">
                  <div className="pg-row-main">
                    <div className="pg-title">
                      <b>{localized(manifestText(p.manifest, "name"), locale) || p.id}</b>
                      {p.source.kind === "community" && <span className="src-tag">{t("plugins.source.community")}</span>}
                      {inUse && <span className="pg-badge acc">{t("plugins.badge.current")}</span>}
                      {isSystemType(p.plugin_type) && !p.enabled && <span className="pg-badge">{t("plugins.badge.disabled")}</span>}
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
                    {links.homepage && (
                      <button className="btn sm" type="button" title={links.homepage} onClick={() => runtime.openExternal(links.homepage!).catch(() => {})}>
                        <ExternalLink size={12} /> {t("plugins.btn.homepage")}
                      </button>
                    )}
                    {links.source && (
                      <button className="btn sm" type="button" title={links.source} onClick={() => runtime.openExternal(links.source!).catch(() => {})}>
                        <Github size={12} /> {t("plugins.btn.repo")}
                      </button>
                    )}
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
                    {p.plugin_type === "provider" && p.enabled && (
                      <button
                        className="btn sm primary"
                        type="button"
                        onClick={() => navigate(`/subscriptions/new?provider=${encodeURIComponent(`plugin:${p.id}`)}`)}
                      >
                        {t("plugins.btn.addSubscription")}
                      </button>
                    )}
                    {isSystemType(p.plugin_type) && (isDesktop || p.enabled) && (
                      <button className="btn sm" type="button" disabled={busyId !== null} onClick={() => toggleEnabled(p)}>
                        {busyId === p.id ? <Spinner /> : null} {p.enabled ? t("plugins.btn.disable") : t("plugins.btn.enable")}
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
                {detailLinks.homepage && (
                  <>
                    <dt>{t("plugins.install.homepage")}</dt>
                    <dd>
                      <button className="pg-link" type="button" onClick={() => runtime.openExternal(detailLinks.homepage!).catch(() => {})}>
                        {detailLinks.homepage}
                      </button>
                    </dd>
                  </>
                )}
                <dt>{t("plugins.install.source")}</dt>
                <dd className="mono">
                  {detail.repo}
                  {detail.path ? `/${detail.path}` : ""} @ {detail.latest.commit.slice(0, 7)}
                </dd>
              </dl>
              <DialogFooter>
                {detailLinks.homepage && (
                  <button className="btn" type="button" onClick={() => runtime.openExternal(detailLinks.homepage!).catch(() => {})}>
                    <ExternalLink size={12} /> {t("plugins.btn.homepage")}
                  </button>
                )}
                {detailLinks.source && (
                  <button className="btn" type="button" onClick={() => runtime.openExternal(detailLinks.source!).catch(() => {})}>
                    <Github size={12} /> {t("plugins.btn.repo")}
                  </button>
                )}
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
