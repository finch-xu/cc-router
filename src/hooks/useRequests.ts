import { useQuery, keepPreviousData } from "@tanstack/react-query";
import { api } from "@/api/tauri";

export const REQUESTS_KEY = "requests";

export function useRequests(page: number, pageSize: number) {
  return useQuery({
    queryKey: [REQUESTS_KEY, page, pageSize],
    queryFn: () => api.listRequests(page, pageSize),
    placeholderData: keepPreviousData,
    staleTime: 5_000,
  });
}
