import { useCallback, useEffect, useSyncExternalStore } from "react";
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

/**
 * 跨虚拟模型聚合: 给定一组订阅, 返回其中最新的 flash。
 * 用于按 provider 维度高亮 —— 一个 provider 下可能有多条订阅, 每条又被多个
 * 虚拟模型引用, 逐对调 useRouteFlashState 会让 hook 数量随数据变化。
 *
 * getSnapshot 返回的是 FLASHES 里的 entry 引用本身(同一次 flash 内不变),
 * 无匹配时返回 undefined —— 两者都引用稳定, 不会触发重渲染循环。
 */
export function useAnyRouteFlashState(subIds: string[]): FlashEntry | undefined {
  const joined = subIds.join("|");
  const getSnapshot = useCallback(() => {
    if (!joined) return undefined;
    const wanted = new Set(joined.split("|"));
    let latest: FlashEntry | undefined;
    for (const [key, entry] of FLASHES) {
      const sep = key.indexOf("::");
      if (sep < 0 || !wanted.has(key.slice(sep + 2))) continue;
      if (!latest || entry.token > latest.token) latest = entry;
    }
    return latest;
  }, [joined]);
  return useSyncExternalStore(subscribe, getSnapshot, () => undefined);
}
