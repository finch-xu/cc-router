import { useIsMutating, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { runtime } from "@/runtime";

export const agentNames = { claude: "Claude Code", codex: "Codex CLI" } as const;
export type AgentClient = keyof typeof agentNames;
export type MonitorAction = "enable" | "pause" | "clear";

interface AgentConnection {
  client: AgentClient;
  connection: "unconfigured" | "enabled" | "paused" | "cleanup_pending";
  last_report_ms: number | null;
  plugin_version: string | null;
  error: string | null;
}

export const liveSessionQueryKey = ["live-client-sessions"] as const;

export function useAgentMonitor(interval?: number) {
  return useQuery({
    queryKey: [...liveSessionQueryKey, "configuration"],
    queryFn: () => runtime.invoke<AgentConnection[]>("session_monitor_status"),
    refetchInterval: interval,
  });
}

const configureKey = [...liveSessionQueryKey, "configure"] as const;

export function useAgentMonitorBusy() {
  return useIsMutating({ mutationKey: configureKey }) > 0;
}

export function useConfigureAgentMonitor() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationKey: configureKey,
    mutationFn: ({ client, action }: { client: AgentClient; action: MonitorAction }) =>
      runtime.invoke<void>("configure_session_monitor", { client, action }),
    onSettled: () => queryClient.invalidateQueries({ queryKey: liveSessionQueryKey }),
  });
}

export function useAgentVersion(client: AgentClient, enabled: boolean) {
  return useQuery({
    queryKey: ["agent-monitor-version", client],
    queryFn: () => runtime.invoke<string | null>("session_monitor_client_version", { client }),
    staleTime: 30_000,
    enabled,
  });
}
