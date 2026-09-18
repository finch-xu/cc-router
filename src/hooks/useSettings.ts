import { useEffect, useRef } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "@/api/tauri";
import type { SettingsPatch, TuiInstallOutcome } from "@/types";

export const SETTINGS_KEY = ["settings"] as const;
export const PROXY_STATUS_KEY = ["proxy-status"] as const;
export const TUI_LAUNCH_INFO_KEY = ["tui-launch-info"] as const;

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

export function useLanAddresses(enabled: boolean) {
  return useQuery({
    queryKey: ["lan-addresses"],
    queryFn: () => api.listLanAddresses(),
    enabled,
    staleTime: 60_000,
  });
}

/** sidecar 的路径在进程生命周期内不变, 但 `in_path` / `install_blocked` / `local_bin_off_path`
 * 会在 app 外面发生变化 (用户在终端里手动装了 / 卸了, 或者手动解决了「已被占用」的冲突) ——
 * 这三个字段不能跟 path 一样按「永不过期」处理, 否则用户在终端修好冲突之后回到设置页, 按钮还是
 * 灰的。改成每次挂载都重新读一次、窗口重新聚焦也重新读 (fix-2 F2)。 */
export function useTuiLaunchInfo() {
  return useQuery({
    queryKey: TUI_LAUNCH_INFO_KEY,
    queryFn: () => api.tuiLaunchInfo(),
    staleTime: 0,
    refetchOnMount: "always",
    refetchOnWindowFocus: true,
  });
}

/** 添加到 PATH / 移除. 两个 command 都返回 TuiInstallOutcome, 成功时把其中的 info 立即写回缓存
 * (界面不用等下一次 refetch 就能翻转); cancelled 标志留给调用方从 mutation 的 `.data` 里自己读
 * (展示「已取消」提示用)。`onSettled` 无论成败都强制重新拉一次最新状态——installed_at 的具体值、
 * blocked 的具体原因这些只有后端知道, 成功时的 info 只是"这次操作完之后"的快照, 不代表期间没有
 * 别的因素 (比如用户几乎同时在终端里也动了一下) 让它又变了 (fix-2 F2)。 */
export function useTuiPathInstall() {
  const queryClient = useQueryClient();
  const onSuccess = (outcome: TuiInstallOutcome) => queryClient.setQueryData(TUI_LAUNCH_INFO_KEY, outcome.info);
  const onSettled = () => queryClient.invalidateQueries({ queryKey: TUI_LAUNCH_INFO_KEY });
  return {
    install: useMutation({ mutationFn: () => api.installTuiCommand(), onSuccess, onSettled }),
    uninstall: useMutation({ mutationFn: () => api.uninstallTuiCommand(), onSuccess, onSettled }),
  };
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
