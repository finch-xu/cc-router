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
