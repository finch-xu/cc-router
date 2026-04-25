import { useState, useEffect, useRef } from "react";
import { AlertTriangle, RefreshCw, Check } from "lucide-react";
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
import { api } from "@/api/tauri";
import {
  useProxyStatus,
  useSettings,
  useUpdateSettings,
  useGenerateNewToken,
} from "@/hooks/useSettings";

export function SettingsPage() {
  const settings = useSettings();
  const proxy = useProxyStatus();
  const updateMut = useUpdateSettings();

  const [port, setPort] = useState<number>(23456);
  const [listenAll, setListenAll] = useState(false);
  const [autostart, setAutostart] = useState(false);
  const [retentionDays, setRetentionDays] = useState(30);
  const [dbLimitMb, setDbLimitMb] = useState(500);
  const [authEnabled, setAuthEnabled] = useState(true);
  const [corsEnabled, setCorsEnabled] = useState(true);
  const [corsAllowOrigin, setCorsAllowOrigin] = useState("*");
  const [resetDialog, setResetDialog] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [tokenJustRegenerated, setTokenJustRegenerated] = useState(false);
  const regenerateTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // 防止 generateNewToken 等 mutation 触发的 settings 失效把用户未保存的编辑覆盖掉
  const initializedRef = useRef(false);

  const generateTokenMut = useGenerateNewToken();

  useEffect(
    () => () => {
      if (regenerateTimerRef.current) clearTimeout(regenerateTimerRef.current);
    },
    [],
  );

  useEffect(() => {
    if (settings.data && !initializedRef.current) {
      setPort(settings.data.proxy_port);
      setListenAll(settings.data.listen_all);
      setAutostart(settings.data.autostart);
      setRetentionDays(settings.data.log_retention_days);
      setDbLimitMb(settings.data.db_size_limit_mb);
      setAuthEnabled(settings.data.auth_enabled);
      setCorsEnabled(settings.data.cors_enabled);
      setCorsAllowOrigin(settings.data.cors_allow_origin);
      initializedRef.current = true;
    }
  }, [settings.data]);

  const needsRestart =
    settings.data !== undefined &&
    (settings.data.proxy_port !== port || settings.data.listen_all !== listenAll);

  async function save() {
    await updateMut.mutateAsync({
      proxy_port: port,
      listen_all: listenAll,
      autostart,
      log_retention_days: retentionDays,
      db_size_limit_mb: dbLimitMb,
      auth_enabled: authEnabled,
      cors_enabled: corsEnabled,
      cors_allow_origin: corsAllowOrigin,
    });
  }

  async function regenerateToken() {
    try {
      await generateTokenMut.mutateAsync();
      setTokenJustRegenerated(true);
      if (regenerateTimerRef.current) clearTimeout(regenerateTimerRef.current);
      regenerateTimerRef.current = setTimeout(
        () => setTokenJustRegenerated(false),
        2000,
      );
    } catch (e) {
      alert(`Token 生成失败: ${e}`);
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
      alert(`恢复出厂失败: ${e}`);
    }
  }

  return (
    <>
      <div className="page-header">
        <h1>设置</h1>
        <div className="subtitle">代理服务、客户端配置、鉴权与数据保留期。</div>
      </div>

      {/* 代理服务 */}
      <div className="card section">
        <div className="card-head">
          <div className="card-title">代理服务</div>
          <span className={"pill " + (proxy.data?.running ? "ok" : "")}>
            <span className="dot" />
            {proxy.data?.running ? "运行中" : "未运行"}
          </span>
        </div>
        <div className="card-body">
          <div className="setting-row">
            <div className="label-col">
              监听端口
              <div className="desc">默认 23456,被占用时自动 +1 递增。</div>
            </div>
            <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
              <input
                className="input mono"
                type="number"
                value={port}
                onChange={(e) => setPort(Number(e.target.value) || 23456)}
                style={{ width: 120 }}
              />
              <span style={{ fontSize: 12, color: "var(--ink-4)" }}>
                当前实际:<span className="mono"> {proxy.data?.port ?? "-"}</span>
              </span>
            </div>
          </div>

          <div className="setting-row">
            <div className="label-col">
              监听地址
              <div className="desc">
                默认仅本机回环。开启「局域网可访问」后其他设备 Claude Code 可以指向本机 IP。
              </div>
            </div>
            <div>
              <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
                <Toggle checked={listenAll} onChange={setListenAll} aria-label="局域网可访问" />
                <span
                  className="mono"
                  style={{
                    fontSize: 12,
                    color: listenAll ? "var(--ink-2)" : "var(--ink-4)",
                  }}
                >
                  {listenAll ? "0.0.0.0:" : "127.0.0.1:"}
                  {port}
                </span>
              </div>
              {listenAll && (
                <div className="field-hint" style={{ color: "var(--err)" }}>
                  ⚠️ 开启后,同网段任何设备都能调用代理。代理不做鉴权,慎用于不受信任的网络。
                </div>
              )}
            </div>
          </div>

          <div className="setting-row">
            <div className="label-col">开机自启动</div>
            <Toggle checked={autostart} onChange={setAutostart} aria-label="开机自启动" />
          </div>

          {needsRestart && (
            <div className="alert warn">
              <AlertTriangle size={14} />
              修改端口或监听地址需要重启 app 才生效。保存后请手动退出 cc-router 再启动。
            </div>
          )}
        </div>
      </div>

      {/* 鉴权与跨域 */}
      <div className="card section">
        <div className="card-head">
          <div className="card-title">鉴权与跨域</div>
        </div>
        <div className="card-body">
          <div className="setting-row">
            <div className="label-col">
              Token 鉴权
              <div className="desc">
                在 Claude Code 启动时配 ANTHROPIC_API_KEY 或 ANTHROPIC_AUTH_TOKEN 等于此 token。重新生成后所有已配置客户端需同步更新。
              </div>
            </div>
            <div>
              <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 10 }}>
                <Toggle checked={authEnabled} onChange={setAuthEnabled} aria-label="Token 鉴权" />
                <span style={{ fontSize: 12, color: "var(--ink-2)" }}>
                  {authEnabled ? "已开启,请求必须携带正确 token" : "已关闭,所有请求放行"}
                </span>
              </div>
              {authEnabled && settings.data && (
                <div style={{ display: "flex", gap: 8 }}>
                  <input
                    className="input mono"
                    value={settings.data.auth_token}
                    readOnly
                    style={{ fontSize: 11.5, color: "var(--ink-2)" }}
                  />
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
                    {tokenJustRegenerated ? "已生成" : "重新生成"}
                  </button>
                </div>
              )}
            </div>
          </div>

          <div className="setting-row">
            <div className="label-col">
              CORS 跨域
              <div className="desc">
                {corsEnabled
                  ? "已开启,响应附加 CORS 头。指定具体值如 http://localhost:5173 仅允许该来源。"
                  : "已关闭,浏览器跨域调用会被拦截。"}
              </div>
            </div>
            <div>
              <div style={{ display: "flex", alignItems: "center", gap: 10, marginBottom: 10 }}>
                <Toggle checked={corsEnabled} onChange={setCorsEnabled} aria-label="CORS 跨域" />
                <span style={{ fontSize: 12, color: "var(--ink-2)" }}>
                  {corsEnabled ? "已开启,响应附加 CORS 头" : "已关闭,浏览器跨域调用会被拦截"}
                </span>
              </div>
              {corsEnabled && (
                <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                  <input
                    className="input mono"
                    value={corsAllowOrigin}
                    onChange={(e) => setCorsAllowOrigin(e.target.value)}
                    placeholder="*"
                    style={{ maxWidth: 280 }}
                  />
                  <span
                    className="mono"
                    style={{ fontSize: 11.5, color: "var(--ink-4)" }}
                  >
                    Access-Control-Allow-Origin
                  </span>
                </div>
              )}
            </div>
          </div>
        </div>
      </div>

      {/* 数据存储 */}
      <div className="card section">
        <div className="card-head">
          <div className="card-title">数据存储</div>
        </div>
        <div className="card-body">
          <div className="setting-row">
            <div className="label-col">请求日志保留期</div>
            <select
              className="select"
              style={{ maxWidth: 200 }}
              value={String(retentionDays)}
              onChange={(e) => setRetentionDays(Number(e.target.value))}
            >
              <option value="7">7 天</option>
              <option value="30">30 天</option>
              <option value="90">90 天</option>
              <option value="36500">永久</option>
            </select>
          </div>
          <div className="setting-row">
            <div className="label-col">数据库大小上限</div>
            <select
              className="select"
              style={{ maxWidth: 200 }}
              value={String(dbLimitMb)}
              onChange={(e) => setDbLimitMb(Number(e.target.value))}
            >
              <option value="100">100 MB</option>
              <option value="500">500 MB</option>
              <option value="1024">1 GB</option>
              <option value="10240">10 GB (近似无限)</option>
            </select>
          </div>
          <div style={{ display: "flex", justifyContent: "flex-end", paddingTop: 16 }}>
            <button className="btn primary" onClick={save} disabled={updateMut.isPending} type="button">
              {updateMut.isPending && <Spinner />}
              保存设置
            </button>
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
            <AlertTriangle size={14} /> 危险区域 · 恢复出厂设置
          </div>
          <div style={{ fontSize: 12, color: "var(--ink-3)", lineHeight: 1.6 }}>
            清空所有订阅、API Key、虚拟模型绑定、请求日志、设置文件。app 会自动重启并进入初始欢迎页。此操作不可撤销。
          </div>
        </div>
        <button className="btn danger" type="button" onClick={() => setResetDialog(true)}>
          恢复出厂设置
        </button>
      </div>

      {/* 恢复出厂确认弹窗 */}
      <Dialog open={resetDialog} onOpenChange={(v) => !resetting && setResetDialog(v)}>
        <DialogContent className="cc-dialog">
          <DialogHeader>
            <DialogTitle>
              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <AlertTriangle size={16} style={{ color: "var(--err)" }} />
                确认恢复出厂设置?
              </div>
            </DialogTitle>
            <DialogDescription asChild>
              <div>
                下面的数据将被永久删除:
                <ul style={{ marginTop: 8, paddingLeft: 20, fontSize: 13, lineHeight: 1.7 }}>
                  <li>所有订阅(包括明文存储的 API Key)</li>
                  <li>虚拟模型绑定</li>
                  <li>请求日志和模型列表缓存</li>
                  <li>app 设置(端口、保留期等)</li>
                </ul>
                <p style={{ marginTop: 12 }}>操作完成后 app 会自动重启。</p>
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
              取消
            </button>
            <button
              className="btn danger"
              onClick={confirmReset}
              disabled={resetting}
              type="button"
            >
              {resetting && <Spinner />}
              确认清空并重启
            </button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
