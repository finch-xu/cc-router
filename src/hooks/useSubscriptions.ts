import {
  useQuery,
  useMutation,
  useQueryClient,
  type QueryClient,
} from "@tanstack/react-query";
import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "@/api/tauri";
import type {
  CreateSubscriptionInput,
  SubscriptionPatch,
} from "@/types";

export const SUBSCRIPTIONS_KEY = ["subscriptions"] as const;
export const SUBSCRIPTION_DETAIL_KEY = ["subscription"] as const;

// 列表 + 所有详情一并失效。详情页用 ["subscription", id],按前缀匹配。
function invalidateSubscriptions(qc: QueryClient) {
  qc.invalidateQueries({ queryKey: SUBSCRIPTIONS_KEY });
  qc.invalidateQueries({ queryKey: SUBSCRIPTION_DETAIL_KEY });
}

/** 在 App 顶层挂一次,把后端 subscription_state_changed 事件转成 query 失效。 */
export function useSubscriptionEventBridge() {
  const queryClient = useQueryClient();
  useEffect(() => {
    const promise = listen("subscription_state_changed", () => {
      invalidateSubscriptions(queryClient);
    });
    return () => {
      promise.then((unlisten) => unlisten()).catch(() => {});
    };
  }, [queryClient]);
}

export function useSubscriptions() {
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
    onSuccess: () => invalidateSubscriptions(queryClient),
  });
}

export function useUpdateSubscription() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, patch }: { id: string; patch: SubscriptionPatch }) =>
      api.updateSubscription(id, patch),
    onSuccess: () => invalidateSubscriptions(queryClient),
  });
}

export function useDeleteSubscription() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => api.deleteSubscription(id),
    onSuccess: () => {
      invalidateSubscriptions(queryClient);
      queryClient.invalidateQueries({ queryKey: ["virtual-models"] });
    },
  });
}

export function useSetSubscriptionEnabled() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      api.setSubscriptionEnabled(id, enabled),
    onSuccess: () => invalidateSubscriptions(queryClient),
  });
}

export function useUpdateSubscriptionKey() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ id, newKey }: { id: string; newKey: string }) =>
      api.updateSubscriptionKey(id, newKey),
    onSuccess: () => invalidateSubscriptions(queryClient),
  });
}
