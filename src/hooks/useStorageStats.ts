import { useQuery } from "@tanstack/react-query";
import { api } from "@/api/tauri";

export function useStorageStats() {
  return useQuery({
    queryKey: ["storage-stats"],
    queryFn: () => api.getStorageStats(),
    staleTime: 30_000,
  });
}
