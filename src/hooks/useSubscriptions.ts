import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "@/api/tauri";
import type {
  CreateSubscriptionInput,
  SubscriptionPatch,
} from "@/types";

export const SUBSCRIPTIONS_KEY = ["subscriptions"] as const;

export function useSubscriptions() {
  const queryClient = useQueryClient();

  useEffect(() => {
    const promise = listen("subscription_state_changed", () => {
      queryClient.invalidateQueries({ queryKey: SUBSCRIPTIONS_KEY });
    });
    return () => {
      promise.then((unlisten) => unlisten()).catch(() => {});
    };
  }, [queryClient]);

  return useQuery({
    queryKey: SUBSCRIPTIONS_KEY,
    queryFn: () => api.listSubscriptions(),
    refetchInterval: 10_000,
  });
}

export function useCreateSubscription() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: CreateSubscriptionInput) => api.createSubscription(input),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: SUBSCRIPTIONS_KEY });
    },
  });
}

export function useUpdateSubscription() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, patch }: { id: string; patch: SubscriptionPatch }) =>
      api.updateSubscription(id, patch),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: SUBSCRIPTIONS_KEY });
    },
  });
}

export function useDeleteSubscription() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => api.deleteSubscription(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: SUBSCRIPTIONS_KEY });
      queryClient.invalidateQueries({ queryKey: ["virtual-models"] });
    },
  });
}

export function useSetSubscriptionEnabled() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      api.setSubscriptionEnabled(id, enabled),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: SUBSCRIPTIONS_KEY });
    },
  });
}

export function useUpdateSubscriptionKey() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, newKey }: { id: string; newKey: string }) =>
      api.updateSubscriptionKey(id, newKey),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: SUBSCRIPTIONS_KEY });
    },
  });
}
