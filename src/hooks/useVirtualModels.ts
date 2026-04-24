import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "@/api/tauri";
import type { UpdateVirtualModelInput, VirtualModelName } from "@/types";

export const VIRTUAL_MODELS_KEY = ["virtual-models"] as const;

export function useVirtualModels() {
  return useQuery({
    queryKey: VIRTUAL_MODELS_KEY,
    queryFn: () => api.listVirtualModels(),
  });
}

export function useUpdateVirtualModel() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ name, input }: { name: VirtualModelName; input: UpdateVirtualModelInput }) =>
      api.updateVirtualModel(name, input),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: VIRTUAL_MODELS_KEY });
    },
  });
}
