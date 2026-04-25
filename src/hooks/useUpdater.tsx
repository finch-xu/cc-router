import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  checkForUpdate,
  isAppImageRuntime,
  isLinuxPlatform,
  relaunchApp,
  type Update,
} from "@/lib/updater";
import { version as PKG_VERSION } from "../../package.json";

export type UpdaterStatus =
  | "idle"
  | "checking"
  | "up_to_date"
  | "available"
  | "downloading"
  | "ready"
  | "error";

export interface UpdaterProgress {
  downloaded: number;
  total: number | null;
}

/**
 * 区分两种检测结果:
 * - auto: 走 Tauri plugin-updater 完整流程 (下载 + 验签 + 安装)
 * - manual: 仅检测到新版,需用户手动下载 (Linux deb 唯一情况)
 */
export type DetectedUpdate =
  | { kind: "auto"; update: Update; version: string; body?: string }
  | { kind: "manual"; version: string; body?: string };

export interface UpdaterState {
  status: UpdaterStatus;
  detected: DetectedUpdate | null;
  progress: UpdaterProgress | null;
  errorMessage: string | null;
  check: () => Promise<void>;
  install: () => Promise<void>;
  restart: () => Promise<void>;
  dismiss: () => void;
}

const UpdaterContext = createContext<UpdaterState | null>(null);

const MANIFEST_URL =
  "https://github.com/finch-xu/cc-router/releases/latest/download/latest.json";

export function UpdaterProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<UpdaterStatus>("idle");
  const [detected, setDetected] = useState<DetectedUpdate | null>(null);
  const [progress, setProgress] = useState<UpdaterProgress | null>(null);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);

  const downloadedRef = useRef(0);
  const totalRef = useRef<number | null>(null);
  const rafIdRef = useRef<number | null>(null);

  const flushProgress = useCallback(() => {
    rafIdRef.current = null;
    setProgress({ downloaded: downloadedRef.current, total: totalRef.current });
  }, []);

  const scheduleProgressFlush = useCallback(() => {
    if (rafIdRef.current === null) {
      rafIdRef.current = requestAnimationFrame(flushProgress);
    }
  }, [flushProgress]);

  const check = useCallback(async () => {
    setStatus("checking");
    setErrorMessage(null);
    try {
      const linux = isLinuxPlatform();
      if (linux && !(await isAppImageRuntime())) {
        const manual = await fetchManifestForManualCompare();
        if (manual) {
          setDetected(manual);
          setStatus("available");
        } else {
          setDetected(null);
          setStatus("up_to_date");
        }
        return;
      }

      const update = await checkForUpdate();
      if (!update) {
        setDetected(null);
        setStatus("up_to_date");
        return;
      }
      setDetected({
        kind: "auto",
        update,
        version: update.version,
        body: update.body,
      });
      setStatus("available");
    } catch (e) {
      console.warn("[updater] check failed", e);
      setErrorMessage(e instanceof Error ? e.message : String(e));
      setStatus("error");
    }
  }, []);

  const install = useCallback(async () => {
    if (!detected || detected.kind !== "auto") return;
    setStatus("downloading");
    downloadedRef.current = 0;
    totalRef.current = null;
    setProgress({ downloaded: 0, total: null });
    try {
      await detected.update.download((event) => {
        if (event.event === "Started") {
          totalRef.current = event.data.contentLength ?? null;
          downloadedRef.current = 0;
          scheduleProgressFlush();
        } else if (event.event === "Progress") {
          downloadedRef.current += event.data.chunkLength;
          scheduleProgressFlush();
        } else if (event.event === "Finished") {
          if (rafIdRef.current !== null) {
            cancelAnimationFrame(rafIdRef.current);
            rafIdRef.current = null;
          }
          setProgress({
            downloaded: downloadedRef.current,
            total: totalRef.current,
          });
        }
      });
      // 解压安装但 *不* 自动重启,留给用户决定时机 (cc-router 是常驻代理)
      await detected.update.install();
      setStatus("ready");
    } catch (e) {
      console.warn("[updater] install failed", e);
      setErrorMessage(e instanceof Error ? e.message : String(e));
      setStatus("error");
    }
  }, [detected, scheduleProgressFlush]);

  const restart = useCallback(async () => {
    try {
      await relaunchApp();
    } catch (e) {
      console.warn("[updater] relaunch failed", e);
    }
  }, []);

  const dismiss = useCallback(() => {
    setStatus("idle");
    setProgress(null);
    setErrorMessage(null);
  }, []);

  useEffect(
    () => () => {
      if (rafIdRef.current !== null) {
        cancelAnimationFrame(rafIdRef.current);
      }
    },
    [],
  );

  const value = useMemo<UpdaterState>(
    () => ({
      status,
      detected,
      progress,
      errorMessage,
      check,
      install,
      restart,
      dismiss,
    }),
    [status, detected, progress, errorMessage, check, install, restart, dismiss],
  );

  return <UpdaterContext.Provider value={value}>{children}</UpdaterContext.Provider>;
}

export function useUpdater(): UpdaterState {
  const ctx = useContext(UpdaterContext);
  if (!ctx) {
    throw new Error("useUpdater must be used within UpdaterProvider");
  }
  return ctx;
}

export function useUpdaterAutoCheck() {
  const { check } = useUpdater();
  useEffect(() => {
    void check();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
}

async function fetchManifestForManualCompare(): Promise<DetectedUpdate | null> {
  try {
    const resp = await fetch(MANIFEST_URL, { cache: "no-store" });
    if (!resp.ok) return null;
    const manifest = (await resp.json()) as { version: string; notes?: string };
    if (!isNewer(manifest.version, PKG_VERSION)) return null;
    return { kind: "manual", version: manifest.version, body: manifest.notes };
  } catch (e) {
    console.warn("[updater] manual manifest fetch failed", e);
    return null;
  }
}

function isNewer(a: string, b: string): boolean {
  const pa = a.split(".").map((n) => parseInt(n, 10) || 0);
  const pb = b.split(".").map((n) => parseInt(n, 10) || 0);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const x = pa[i] ?? 0;
    const y = pb[i] ?? 0;
    if (x > y) return true;
    if (x < y) return false;
  }
  return false;
}
