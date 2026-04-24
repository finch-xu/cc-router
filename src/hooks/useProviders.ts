import { useQuery } from "@tanstack/react-query";
import { api } from "@/api/tauri";

export function useProviders() {
  return useQuery({
    queryKey: ["providers"],
    queryFn: () => api.listProviders(),
    staleTime: Infinity,
  });
}
