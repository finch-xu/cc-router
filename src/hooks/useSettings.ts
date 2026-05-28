import { useEffect, useRef } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "@/api/tauri";
import type { SettingsPatch } from "@/types";

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
    /** 后端给定的真实 base URL (含 scheme + 真实端口); 未加载完成时为 undefined. */
    baseUrl: proxy.data?.base_url,
    token: settings.data?.auth_token ?? "",
    running: proxy.data?.running ?? false,
  };
}

/**
 * 首次启动写入默认更新源,只在 update_source 为 null 时触发一次。
 * 默认 "china" — 主要用户群是国内, GitHub 直连不稳定,中国大陆 OSS 镜像可达性更好;
 * 国际用户在 Settings 里一键切回 "international" 即可,后续不再被覆盖.
 */
export function useUpdateSourceAutoInit() {
  const { data } = useSettings();
  const updateMut = useUpdateSettings();
  const sentRef = useRef(false);

  useEffect(() => {
    if (!data || sentRef.current) return;
    if (data.update_source != null) return; // 已经设置过

    sentRef.current = true;
    void updateMut.mutateAsync({ update_source: "china" });
  }, [data, updateMut]);
}
