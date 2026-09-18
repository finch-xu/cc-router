import { useEffect, useRef, useState } from "react";
import { RefreshCw, Check, Copy, Terminal, Trash2 } from "lucide-react";
import { Toggle } from "@/components/Toggle";
import { Spinner } from "@/components/Spinner";
import { CopyableBlock } from "@/components/CopyableBlock";
import {
  useGenerateNewToken,
  useLanAddresses,
  useProxyStatus,
  useTuiLaunchInfo,
  useTuiPathInstall,
} from "@/hooks/useSettings";
import { useT } from "@/i18n";
import { runtime } from "@/runtime";
import type { TuiLaunchInfo } from "@/types";
import type { SettingsForm } from "./useSettingsForm";

/** 安全与访问: 鉴权 token / CORS / 网页界面. */
export function AccessTab({ form }: { form: SettingsForm }) {
  const { t } = useT();
  const { settings } = form;
  const generateTokenMut = useGenerateNewToken();
  const [tokenJustRegenerated, setTokenJustRegenerated] = useState(false);
  const [tokenCopied, setTokenCopied] = useState(false);
  const regenerateTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const copyTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (regenerateTimerRef.current) clearTimeout(regenerateTimerRef.current);
      if (copyTimerRef.current) clearTimeout(copyTimerRef.current);
    },
    [],
  );

  async function copyToken() {
    if (!settings.data) return;
    try {
      await runtime.copyText(settings.data.auth_token);
    } catch {
      /* ignore: 与 CopyableBlock 一致, 剪贴板失败不打断 */
    }
    setTokenCopied(true);
    if (copyTimerRef.current) clearTimeout(copyTimerRef.current);
    copyTimerRef.current = setTimeout(() => setTokenCopied(false), 1500);
  }

  async function regenerateToken() {
    try {
      await generateTokenMut.mutateAsync();
      setTokenJustRegenerated(true);
      if (regenerateTimerRef.current) clearTimeout(regenerateTimerRef.current);
      regenerateTimerRef.current = setTimeout(() => setTokenJustRegenerated(false), 2000);
    } catch (e) {
      alert(`${t("settings.auth.token.alertFailed")}: ${e}`);
    }
  }

  return (
    <>
      {/* 鉴权与跨域 */}
      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("settings.section.auth")}</div>
        </div>
        <div className="card-body">
          <div className="setting-row">
            <div className="label-col">
              {t("settings.auth.token.label")}
              <div className="desc">{t("settings.auth.token.desc")}</div>
            </div>
            <div>
              <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 10 }}>
                <Toggle
                  checked={form.authEnabled}
                  onChange={(v) => void form.changeAuthEnabled(v)}
                  aria-label={t("settings.auth.token.label")}
                />
                <span style={{ fontSize: 12, color: "var(--ink-2)" }}>
                  {form.authEnabled
                    ? t("settings.auth.token.enabled")
                    : t("settings.auth.token.disabled")}
                </span>
              </div>
              {form.authEnabled && settings.data && (
                <div style={{ display: "flex", gap: 8 }}>
                  <input
                    className="input mono"
                    value={settings.data.auth_token}
                    readOnly
                    style={{ fontSize: 11.5, color: "var(--ink-2)" }}
                  />
                  <button className="btn" onClick={copyToken} type="button">
                    {tokenCopied ? (
                      <Check size={12} style={{ color: "var(--ok)" }} />
                    ) : (
                      <Copy size={12} />
                    )}
                    {tokenCopied ? t("common.copied") : t("common.copy")}
                  </button>
                  <button
                    className="btn"
                    onClick={regenerateToken}
                    disabled={generateTokenMut.isPending}
                    type="button"
                  >
                    {generateTokenMut.isPending ? (
                      <Spinner />
                    ) : tokenJustRegenerated ? (
                      <Check size={12} style={{ color: "var(--ok)" }} />
                    ) : (
                      <RefreshCw size={12} />
                    )}
                    {tokenJustRegenerated
                      ? t("settings.auth.token.regenerated")
                      : t("settings.auth.token.regenerate")}
                  </button>
                </div>
              )}
            </div>
          </div>

          <div className="setting-row">
            <div className="label-col">
              {t("settings.auth.cors.label")}
              <div className="desc">
                {form.corsEnabled
                  ? t("settings.auth.cors.descEnabled")
                  : t("settings.auth.cors.descDisabled")}
              </div>
            </div>
            <div>
              <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 10 }}>
                <Toggle
                  checked={form.corsEnabled}
                  onChange={(v) => void form.changeCorsEnabled(v)}
                  aria-label={t("settings.auth.cors.label")}
                />
                <span style={{ fontSize: 12, color: "var(--ink-2)" }}>
                  {form.corsEnabled
                    ? t("settings.auth.cors.statusEnabled")
                    : t("settings.auth.cors.statusDisabled")}
                </span>
              </div>
              {form.corsEnabled && (
                <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                  <input
                    className="input mono"
                    value={form.corsAllowOrigin}
                    onChange={(e) => form.setCorsAllowOrigin(e.target.value)}
                    onBlur={() => {
                      if (
                        settings.data &&
                        form.corsAllowOrigin !== settings.data.cors_allow_origin
                      ) {
                        void form.changeCorsOrigin(form.corsAllowOrigin);
                      }
                    }}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                    }}
                    placeholder="*"
                    style={{ maxWidth: 280 }}
                  />
                  <span className="mono" style={{ fontSize: 11.5, color: "var(--ink-4)" }}>
                    Access-Control-Allow-Origin
                  </span>
                </div>
              )}
            </div>
          </div>
        </div>
      </div>

      {/* 网页界面 */}
      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("settings.section.webUi")}</div>
        </div>
        <div className="card-body">
          <div className="setting-row">
            <div className="label-col">
              {t("settings.webUi.enabled.label")}
              <div className="desc">{t("settings.webUi.enabled.desc")}</div>
            </div>
            <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
              <Toggle
                checked={form.webUiEnabled}
                onChange={(v) => void form.changeWebUiEnabled(v)}
                aria-label={t("settings.webUi.enabled.label")}
              />
              <span style={{ fontSize: 12, color: "var(--ink-2)" }}>
                {form.webUiEnabled ? t("settings.webUi.enabled.on") : t("settings.webUi.enabled.off")}
              </span>
            </div>
          </div>
          <div className="setting-row">
            <div className="label-col">
              {t("settings.webUi.auth.label")}
              <div className="desc">{t("settings.webUi.auth.desc")}</div>
            </div>
            <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
              <Toggle
                checked={form.webUiAuthEnabled}
                disabled={!form.webUiEnabled}
                onChange={(v) => void form.changeWebUiAuthEnabled(v)}
                aria-label={t("settings.webUi.auth.label")}
              />
              <span style={{ fontSize: 12, color: "var(--ink-2)" }}>
                {form.webUiAuthEnabled ? t("settings.webUi.auth.on") : t("settings.webUi.auth.off")}
              </span>
            </div>
          </div>
          {form.webUiEnabled && (
            <WebUiAddresses listenAll={form.listenAll} authEnabled={form.webUiAuthEnabled} />
          )}
        </div>
      </div>

      {/* 终端界面 */}
      <div className="card section">
        <div className="card-head">
          <div className="card-title">{t("settings.section.tui")}</div>
        </div>
        <div className="card-body">
          <div className="setting-row">
            <div className="label-col">
              {t("settings.tui.enabled.label")}
              <div className="desc">{t("settings.tui.enabled.desc")}</div>
            </div>
            <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
              <Toggle
                checked={form.tuiEnabled}
                onChange={(v) => void form.changeTuiEnabled(v)}
                aria-label={t("settings.tui.enabled.label")}
              />
              <span style={{ fontSize: 12, color: "var(--ink-2)" }}>
                {form.tuiEnabled ? t("settings.tui.enabled.on") : t("settings.tui.enabled.off")}
              </span>
            </div>
          </div>
          <TuiLaunchRow enabled={form.tuiEnabled} />
        </div>
      </div>
    </>
  );
}

function WebUiAddresses({ listenAll, authEnabled }: { listenAll: boolean; authEnabled: boolean }) {
  const { t } = useT();
  const proxy = useProxyStatus();
  const lan = useLanAddresses(listenAll);
  const scheme = proxy.data?.http_port ? "http" : "https";
  const port = proxy.data?.http_port ?? proxy.data?.https_port ?? 23456;
  const urls = [`${scheme}://127.0.0.1:${port}/ui/`];
  if (listenAll) {
    for (const ip of lan.data ?? []) urls.push(`${scheme}://${ip}:${port}/ui/`);
  }
  return (
    <div className="setting-row">
      <div className="label-col">
        {t("settings.webUi.addresses.label")}
        {!listenAll && <div className="desc">{t("settings.webUi.addresses.localOnly")}</div>}
      </div>
      <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
        {urls.map((u) => (
          <CopyableBlock key={u} text={u} variant="inline" />
        ))}
        {listenAll && (
          <div className={authEnabled ? "alert warn" : "alert err"} style={{ marginTop: 6 }}>
            {authEnabled ? t("settings.webUi.warn.lanWithAuth") : t("settings.webUi.warn.lanNoAuth")}
          </div>
        )}
      </div>
    </div>
  );
}

/** 启动命令 + 添加到 PATH. 开关关着时置灰但仍可见 —— 可以先装好命令再开开关.
 * 返回一个 fragment 而不是包一层 <div>: 两个 `.setting-row` 必须是 `.card-body` 的直接子元素,
 * 不然 `.setting-row:first-child/:last-child` 那套边框/间距样式就会认错人 (fix-1 R11)。
 * opacity 因此分别写在每个 `.setting-row` 自己身上, 不再挂在已经消失的外层 div 上。 */
function TuiLaunchRow({ enabled }: { enabled: boolean }) {
  const { t } = useT();
  const info = useTuiLaunchInfo();
  if (!info.data) return null;
  const { path, is_appimage, in_path, local_bin_off_path } = info.data;
  // 完整路径永远展示 (spec §7.3): 哪怕已经在 PATH 上, 换一台没重开过的终端 (尤其 Windows) 仍然
  // 只认得完整路径, 命令名会报「找不到命令」——不能把它换没了。只有含空格时才加引号:
  // PowerShell 里带引号的裸字符串会被当成字符串回显而不是执行。
  const fullPathCommand = path ? (/\s/.test(path) ? `"${path}"` : path) : null;
  // Linux copy 装到了 ~/.local/bin, 但那个目录本身不在当前 PATH 里 (下面 TuiPathRow 会警告这件事)
  // 时, 短命令 cc-router-tui 其实还是敲不出来的——不能同时又在这里宣称「已在 PATH 上」(fix-2 F8)。
  const showShort = in_path && !local_bin_off_path;
  return (
    <>
      <div className="setting-row" style={{ opacity: enabled ? 1 : 0.55 }}>
        <div className="label-col">
          {t("settings.tui.launch.label")}
          <div className="desc">
            {path ? t("settings.tui.launch.desc") : t("settings.tui.launch.missing")}
            {path && is_appimage && !in_path && <> {t("settings.tui.launch.appimage")}</>}
            {path && runtime.kind === "web" && <> {t("settings.tui.launch.webHint")}</>}
          </div>
        </div>
        {fullPathCommand && (
          <div style={{ minWidth: 0, flex: 1, display: "flex", flexDirection: "column", gap: 6 }}>
            {showShort && (
              <>
                <CopyableBlock text="cc-router-tui" variant="inline" />
                <span style={{ fontSize: 12, color: "var(--ink-2)" }}>{t("settings.tui.launch.short")}</span>
              </>
            )}
            <CopyableBlock text={fullPathCommand} variant="inline" />
          </div>
        )}
      </div>
      {path && <TuiPathRow info={info.data} enabled={enabled} />}
    </>
  );
}

/** 后端错误是 `{ code, message }` 对象 (Tauri 拒绝值 / 网页端 JSON), 不是 Error. */
function errorText(e: unknown): string {
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}

/** 「添加到 PATH」一栏. 三个平台做法不同, 说明文字跟着 install_kind 走. */
function TuiPathRow({ info, enabled }: { info: TuiLaunchInfo; enabled: boolean }) {
  const { t } = useT();
  const { install, uninstall } = useTuiPathInstall();
  const { install_kind: kind, in_path, installed_at, can_install, install_blocked, local_bin_off_path } = info;
  if (kind === "unavailable") return null;

  const busy = install.isPending || uninstall.isPending;
  const error = install.error ?? uninstall.error;
  // 上一次跑完的 (install 或 uninstall, 谁最近成功谁算) 是不是被用户在系统授权框里取消了——
  // 两边的 mutate() 都会先 reset() 对方和自己, 所以 busy 期间 / 换一次操作后这两个 data 都是 undefined。
  const cancelled = install.data?.cancelled === true || uninstall.data?.cancelled === true;
  // 改宿主机的 PATH / 弹系统授权框只能在桌面窗口里做, 后端对网页端是拒绝桩。
  const desktop = runtime.kind !== "web";

  // occupied 提示要指名冲突的具体路径, 不新增一个 DTO 字段——两种可能的 install_kind 各自的
  // 目标路径是固定的常量, 前端算得出来 (fix-2 F8)。其余两个 blocked.* 文案不吃参数, 传了也没事:
  // applyParams 按 `{path}` 逐个 split/join, 模板里没有这个占位符时循环等于空操作。
  const occupiedPath = kind === "symlink" ? "/usr/local/bin/cc-router-tui" : kind === "copy" ? "~/.local/bin/cc-router-tui" : "";

  return (
    <div className="setting-row" style={{ opacity: enabled ? 1 : 0.55 }}>
      <div className="label-col">
        {t("settings.tui.path.label")}
        {/* 200px 的 label-col 宽度放不下一条完整的 Windows 路径, 又没有天然的换行机会 (反斜杠不是
            换行点) —— overflowWrap: anywhere 允许在任意字符处断行, 而不是把整行文字推出容器 (fix-2 F8)。 */}
        <div className="desc" style={{ overflowWrap: "anywhere" }}>
          {kind === "system"
            ? t("settings.tui.path.system")
            : in_path
              ? t("settings.tui.path.installed", { at: installed_at ?? "" })
              : t(`settings.tui.path.desc.${kind}`)}
          {in_path && (kind === "user_path" || kind === "copy") && (
            <div style={{ marginTop: 4 }}>{t(`settings.tui.path.installedNote.${kind}`)}</div>
          )}
        </div>
      </div>
      <div style={{ display: "flex", flexDirection: "column", gap: 6, alignItems: "flex-start" }}>
        {kind !== "system" && !desktop && (
          <span style={{ fontSize: 12, color: "var(--ink-2)" }}>{t("settings.tui.path.desktopOnly")}</span>
        )}
        {kind !== "system" && desktop && !in_path && (
          <button
            className="btn"
            type="button"
            disabled={busy || !can_install}
            onClick={() => {
              install.reset();
              uninstall.reset();
              install.mutate();
            }}
          >
            {install.isPending ? <Spinner /> : <Terminal size={12} />}
            {t("settings.tui.path.add")}
          </button>
        )}
        {kind !== "system" && desktop && in_path && (
          <button
            className="btn"
            type="button"
            disabled={busy}
            onClick={() => {
              install.reset();
              uninstall.reset();
              uninstall.mutate();
            }}
          >
            {uninstall.isPending ? <Spinner /> : <Trash2 size={12} />}
            {t("settings.tui.path.remove")}
          </button>
        )}
        {install_blocked && !in_path && (
          <div className="alert warn">{t(`settings.tui.path.blocked.${install_blocked}`, { path: occupiedPath })}</div>
        )}
        {kind === "copy" && in_path && local_bin_off_path && (
          <div className="alert warn" style={{ display: "flex", flexDirection: "column", gap: 6 }}>
            {t("settings.tui.path.localBinOffPath")}
            <CopyableBlock text={'export PATH="$HOME/.local/bin:$PATH"'} variant="inline" />
          </div>
        )}
        {!busy && cancelled && <span style={{ fontSize: 12, color: "var(--ink-2)" }}>{t("settings.tui.path.cancelled")}</span>}
        {error != null && <div className="alert err">{errorText(error)}</div>}
      </div>
    </div>
  );
}
