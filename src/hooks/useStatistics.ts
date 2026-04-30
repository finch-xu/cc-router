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
