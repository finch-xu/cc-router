import { useQuery, keepPreviousData } from "@tanstack/react-query";
import { api } from "@/api/tauri";
import type { BreakdownBy, StatsRange } from "@/types";

export const STATS_KEY = "statistics";

export function useOverallStats(range: StatsRange) {
  return useQuery({
    queryKey: [STATS_KEY, "overall", range],
    queryFn: () => api.getOverallStats(range),
    placeholderData: keepPreviousData,
    staleTime: 5_000,
  });
}

/** 费用卡片: 按当前价格表现算; 计价插件装 / 卸 / 启停经 plugins_changed 让 STATS_KEY 失效 */
export function useCostSummary(range: StatsRange) {
  return useQuery({
    queryKey: [STATS_KEY, "cost", range],
    queryFn: () => api.getCostSummary(range),
    placeholderData: keepPreviousData,
    staleTime: 5_000,
  });
}

export function useDailySeries(range: StatsRange) {
  return useQuery({
    queryKey: [STATS_KEY, "daily", range],
    queryFn: () => api.getDailySeries(range),
    placeholderData: keepPreviousData,
    staleTime: 5_000,
  });
}

export function useBreakdown(range: StatsRange, by: BreakdownBy) {
  return useQuery({
    queryKey: [STATS_KEY, "breakdown", range, by],
    queryFn: () => api.getBreakdown(range, by),
    placeholderData: keepPreviousData,
    staleTime: 5_000,
  });
}

export function useTokenHeatmap(days = 365) {
  return useQuery({
    queryKey: [STATS_KEY, "heatmap", days],
    queryFn: () => api.getTokenHeatmap(days),
    placeholderData: keepPreviousData,
    // 365 天聚合, 一天才更新一次 — 长 staleTime 避免连点刷新重复查
    staleTime: 60_000,
  });
}

export function useToolBreakdown(range: StatsRange, limit = 10) {
  return useQuery({
    queryKey: [STATS_KEY, "tools", range, limit],
    queryFn: () => api.getToolBreakdown(range, limit),
    placeholderData: keepPreviousData,
    staleTime: 5_000,
  });
}

export function useRequestMix(range: StatsRange) {
  return useQuery({
    queryKey: [STATS_KEY, "mix", range],
    queryFn: () => api.getRequestMix(range),
    placeholderData: keepPreviousData,
    staleTime: 5_000,
  });
}

/** 订阅列表的「近 24 小时」小柱图; 与订阅列表同节奏轮询 */
export function useSubscriptionActivity() {
  return useQuery({
    queryKey: [STATS_KEY, "subscription-activity"],
    queryFn: () => api.getSubscriptionActivity(),
    refetchInterval: 30_000,
    staleTime: 10_000,
  });
}
