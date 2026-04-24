import { useQuery, keepPreviousData } from "@tanstack/react-query";
import { api } from "@/api/tauri";
import type { RequestLogFilters } from "@/types";

export const REQUESTS_KEY = "requests";

export function useRequests(
  page: number,
  pageSize: number,
  filters?: RequestLogFilters,
) {
  return useQuery({
    queryKey: [REQUESTS_KEY, page, pageSize, filters],
    queryFn: () => api.listRequests(page, pageSize, filters),
    placeholderData: keepPreviousData,
    staleTime: 5_000,
  });
}
