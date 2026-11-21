/**
 * Agent 监控里的会话 (hooks 上报), 实时路由页只取两样: 项目目录名、正在跑的子 Agent 类型。
 * 只有桌面端有这个接口 (网页界面里是拒绝桩), 网页端返回空表, 事件流退回显示会话短 id。
 */
import { useQuery } from "@tanstack/react-query";
import { liveSessionQueryKey } from "@/hooks/useAgentMonitor";
import { isTauriHost, runtime } from "@/runtime";

interface RawSession {
  client: "claude" | "codex";
  id: string;
  cwd: string | null;
  agents: { id: string; kind: string | null; running: boolean }[];
}

export interface SessionInfo {
  project: string | null;
  runningAgents: string[];
}

export function useLiveSessions(): Map<string, SessionInfo> {
  const q = useQuery({
    queryKey: [...liveSessionQueryKey, "live-routing"],
    queryFn: () => runtime.invoke<RawSession[]>("get_live_sessions"),
    enabled: isTauriHost,
    refetchInterval: 3_000,
    select: (rows) =>
      new Map(
        rows.map((s) => [
          s.id,
          {
            project: s.cwd?.split(/[\\/]/).filter(Boolean).at(-1) ?? null,
            runningAgents: s.agents.filter((a) => a.running && a.kind).map((a) => a.kind!),
          },
        ]),
      ),
  });
  return q.data ?? EMPTY;
}

const EMPTY = new Map<string, SessionInfo>();
