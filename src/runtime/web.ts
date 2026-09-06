import { AUTH_REQUIRED_EVENT, type Runtime } from "./types";

const API_PREFIX = "/ui/api";

/** 与 Rust proxy/web/auth.rs::CSRF_HEADER 一致 */
const CSRF_HEADER = "x-ccr-ui";

/**
 * 这两个命令会重启/重建整个进程, 后端永远不会把 HTTP 响应写回来——
 * 浏览器里 fetch 因此必然以网络错误 (TypeError) 收场, 而不是正常的
 * non-2xx 响应。桌面端同理"调用后进程消失", 因此把这类网络失败当作
 * 预期结果吞掉、resolve 成 undefined, 而不是让调用方处理一个误导性的报错。
 * 除这两个命令外的其它失败一律照常向上抛出。
 */
const COMMANDS_THAT_NEVER_RESPOND = new Set(["factory_reset", "relaunch_app"]);

type Handler = (event: { payload: unknown }) => void;

/** 单例 EventSource, 按事件名分发; 首个 listen 时才建连 */
let source: EventSource | null = null;
const handlers = new Map<string, Set<Handler>>();

function ensureSource(): EventSource {
  if (source) return source;
  source = new EventSource(`${API_PREFIX}/events`);
  for (const name of handlers.keys()) attach(source, name);
  return source;
}

function attach(es: EventSource, name: string) {
  es.addEventListener(name, (raw) => {
    const set = handlers.get(name);
    if (!set) return;
    let payload: unknown = null;
    try {
      payload = JSON.parse((raw as MessageEvent).data);
    } catch {
      payload = (raw as MessageEvent).data;
    }
    set.forEach((h) => h({ payload }));
  });
}

/** 登出 / 会话失效时关闭 SSE, 下次 listen 会重新建连 */
export function resetWebEventSource() {
  source?.close();
  source = null;
}

export const webRuntime: Runtime = {
  kind: "web",
  invoke: async <T,>(cmd: string, args?: Record<string, unknown>): Promise<T> => {
    let res: Response;
    try {
      res = await fetch(`${API_PREFIX}/cmd/${cmd}`, {
        method: "POST",
        headers: { "content-type": "application/json", [CSRF_HEADER]: "1" },
        credentials: "same-origin",
        body: JSON.stringify(args ?? {}),
      });
    } catch (err) {
      if (COMMANDS_THAT_NEVER_RESPOND.has(cmd) && err instanceof TypeError) {
        return undefined as T;
      }
      throw err;
    }
    if (res.status === 401) {
      window.dispatchEvent(new Event(AUTH_REQUIRED_EVENT));
      throw { code: "unauthorized", message: "web ui session required" };
    }
    if (!res.ok) {
      throw await res.json().catch(() => ({ code: `http_${res.status}`, message: res.statusText }));
    }
    return (await res.json()) as T;
  },
  listen: async (name, handler) => {
    let set = handlers.get(name);
    const isNew = !set;
    if (!set) {
      set = new Set();
      handlers.set(name, set);
    }
    set.add(handler as Handler);
    const es = ensureSource();
    if (isNew) attach(es, name);
    return () => {
      handlers.get(name)?.delete(handler as Handler);
    };
  },
  openExternal: async (url) => {
    window.open(url, "_blank", "noopener,noreferrer");
  },
  copyText: async (text) => {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      const ta = document.createElement("textarea");
      ta.value = text;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      ta.remove();
    }
  },
  pickSavePath: () => {
    throw new Error("pickSavePath is desktop-only");
  },
  downloadText: (filename, text, mime) => {
    const blob = new Blob([text], { type: mime });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  },
  openDownloadsDir: async () => {},
  windowAction: () => {},
  platformName: () => "web",
};

/** 登录 / 登出 / 会话探测: 不走 invoke, 因为它们的 401 不该触发 AUTH_REQUIRED */
export async function webLogin(token: string): Promise<{ ok: boolean; message?: string }> {
  const res = await fetch(`${API_PREFIX}/login`, {
    method: "POST",
    headers: { "content-type": "application/json", [CSRF_HEADER]: "1" },
    credentials: "same-origin",
    body: JSON.stringify({ token }),
  });
  if (res.status === 204) return { ok: true };
  const body = await res.json().catch(() => ({}));
  return { ok: false, message: body?.error?.message ?? res.statusText };
}

export async function webLogout(): Promise<void> {
  await fetch(`${API_PREFIX}/logout`, {
    method: "POST",
    headers: { [CSRF_HEADER]: "1" },
    credentials: "same-origin",
  }).catch(() => {});
  resetWebEventSource();
}

export async function webSession(): Promise<{ authenticated: boolean; auth_enabled: boolean }> {
  const res = await fetch(`${API_PREFIX}/session`, { credentials: "same-origin" });
  if (!res.ok) return { authenticated: false, auth_enabled: true };
  return (await res.json()) as { authenticated: boolean; auth_enabled: boolean };
}
