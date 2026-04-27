import { useEffect, useSyncExternalStore } from "react";
import { listen } from "@tauri-apps/api/event";
import type {
  RouteAttemptFinishedEvent,
  RouteAttemptStartedEvent,
  RouteFlashKind,
  VirtualModelName,
} from "@/types";

// (vm, subId) → 当前 flash 状态。token 单调递增, 用作组件 React key 触发动画 restart。
type FlashEntry = { kind: RouteFlashKind; token: number };

const FLASH_DURATION_MS = 600;
const FLASHES = new Map<string, FlashEntry>();
const SUBSCRIBERS = new Set<() => void>();
let TOKEN_COUNTER = 0;

function flashKey(vm: VirtualModelName, subId: string) {
  return `${vm}::${subId}`;
}

function notify() {
  SUBSCRIBERS.forEach((fn) => fn());
}

function setFlash(key: string, kind: RouteFlashKind) {
  TOKEN_COUNTER += 1;
  const myToken = TOKEN_COUNTER;
  FLASHES.set(key, { kind, token: myToken });
  notify();
  setTimeout(() => {
    const cur = FLASHES.get(key);
    if (cur && cur.token === myToken) {
      FLASHES.delete(key);
      notify();
    }
  }, FLASH_DURATION_MS);
}

function subscribe(callback: () => void) {
  SUBSCRIBERS.add(callback);
  return () => {
    SUBSCRIBERS.delete(callback);
  };
}

let listenerInstalled = false;

async function installListener() {
  if (listenerInstalled) return;
  listenerInstalled = true;
  try {
    await listen<RouteAttemptStartedEvent>("route_attempt_started", (e) => {
      setFlash(flashKey(e.payload.virtual_model, e.payload.subscription_id), "attempt");
    });
    await listen<RouteAttemptFinishedEvent>("route_attempt_finished", (e) => {
      setFlash(
        flashKey(e.payload.virtual_model, e.payload.subscription_id),
        e.payload.success ? "success" : "error",
      );
    });
  } catch {
    listenerInstalled = false;
  }
}

/** 在 App 顶层挂一次, 启动全局事件监听。 */
export function useRouteFlashListener() {
  useEffect(() => {
    installListener();
  }, []);
}

/** 组件读取自身 (vm, subId) 的当前 flash 状态。无 flash 时返回 undefined。 */
export function useRouteFlashState(
  vm: VirtualModelName,
  subId: string,
): FlashEntry | undefined {
  const key = flashKey(vm, subId);
  return useSyncExternalStore(
    subscribe,
    () => FLASHES.get(key),
    () => undefined,
  );
}
