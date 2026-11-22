import { useEffect } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "@/api/tauri";
import { runtime } from "@/runtime";
import { parseCommunityIndex } from "@/plugins/community";
import { SUBSCRIPTIONS_KEY, SUBSCRIPTION_DETAIL_KEY } from "@/hooks/useSubscriptions";

export const PLUGINS_KEY = ["plugins"] as const;
export const COMMUNITY_INDEX_KEY = ["plugin-community-index"] as const;

/** 已安装插件; 装 / 卸 (含另一端操作) 后经 plugins_changed 事件刷新 */
export function useInstalledPlugins() {
  const qc = useQueryClient();
  useEffect(() => {
    const promise = runtime.listen("plugins_changed", () => {
      qc.invalidateQueries({ queryKey: PLUGINS_KEY });
      // 厂商插件装 / 卸 / 启停会改变厂商列表, 订阅的厂商名 / 「描述已更新」也跟着变
      qc.invalidateQueries({ queryKey: ["providers"] });
      qc.invalidateQueries({ queryKey: SUBSCRIPTIONS_KEY });
      qc.invalidateQueries({ queryKey: SUBSCRIPTION_DETAIL_KEY });
    });
    return () => {
      promise.then((un) => un()).catch(() => {});
    };
  }, [qc]);
  return useQuery({ queryKey: PLUGINS_KEY, queryFn: api.listPlugins });
}

/** 社区目录 (仅桌面端; 网页端后端拒绝) */
export function useCommunityIndex(enabled: boolean) {
  return useQuery({
    queryKey: COMMUNITY_INDEX_KEY,
    queryFn: async () => parseCommunityIndex(await api.fetchCommunityIndex()),
    enabled,
    staleTime: 10 * 60 * 1000,
    retry: 1,
  });
}
