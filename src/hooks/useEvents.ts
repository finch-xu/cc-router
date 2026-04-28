import { useQuery, keepPreviousData } from "@tanstack/react-query";
import { api } from "@/api/tauri";
import type { EventFilters } from "@/types";

export const EVENTS_KEY = "events";

export function useEvents(
  page: number,
  pageSize: number,
  filters?: EventFilters,
) {
  return useQuery({
    queryKey: [EVENTS_KEY, page, pageSize, filters],
    queryFn: () => api.listEvents(page, pageSize, filters),
    placeholderData: keepPreviousData,
    staleTime: 5_000,
  });
}
