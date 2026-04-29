import { useEffect, useRef } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "@/api/tauri";
import type { SettingsPatch, UpdateSource } from "@/types";

export const SETTINGS_KEY = ["settings"] as const;
export const PROXY_STATUS_KEY = ["proxy-status"] as const;

export function useSettings() {
  return useQuery({
    queryKey: SETTINGS_KEY,
    queryFn: () => api.getSettings(),
  });
}

export function useUpdateSettings() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (patch: SettingsPatch) => api.updateSettings(patch),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: SETTINGS_KEY });
    },
  });
}

export function useGenerateNewToken() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => api.generateNewToken(),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: SETTINGS_KEY });
    },
  });
}

export function useProxyStatus() {
  return useQuery({
    queryKey: PROXY_STATUS_KEY,
    queryFn: () => api.proxyStatus(),
    refetchInterval: 5_000,
  });
}

export function useEnvSnippet() {
  return useQuery({
    queryKey: ["env-snippet"],
    queryFn: () => api.envSnippet(),
    refetchInterval: 5_000,
  });
}

export function useProxyEndpoint() {
  const proxy = useProxyStatus();
  const settings = useSettings();
  return {
    port: proxy.data?.port ?? 23456,
    token: settings.data?.auth_token ?? "",
    running: proxy.data?.running ?? false,
  };
}

/**
 * 首次启动按 navigator.language 推断默认更新源,只在 update_source 为 null 时触发一次。
 * zh-* → china,其他 → international。用户后续手动改不会被覆盖(因为 != null 即跳过)。
 *
 * 选择 navigator.language 而非 IP 探测的原因:
 * 1. 不需要任何网络请求,首次启动即可决定
 * 2. preferred_language 走的也是这个路径,口径一致
 * 3. zh-HK / zh-TW 误判到大陆源是可接受的(用户手动切回即可)
 */
export function useUpdateSourceAutoInit() {
  const { data } = useSettings();
  const updateMut = useUpdateSettings();
  const sentRef = useRef(false);

  useEffect(() => {
    if (!data || sentRef.current) return;
    if (data.update_source != null) return; // 已经设置过

    const lang = (navigator.language ?? "").toLowerCase();
    const inferred: UpdateSource = lang.startsWith("zh") ? "china" : "international";
    sentRef.current = true;
    void updateMut.mutateAsync({ update_source: inferred });
  }, [data, updateMut]);
}
