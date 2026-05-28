import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "@/api/tauri";
import type { CodexReadResult } from "@/types";

export const CODEX_CONFIG_READ_KEY = ["codex", "config", "read"] as const;
export const CODEX_CONFIG_STATUS_KEY = ["codex", "config", "status"] as const;
export const CODEX_AUTH_READ_KEY = ["codex", "auth", "read"] as const;
export const CODEX_AUTH_STATUS_KEY = ["codex", "auth", "status"] as const;

/**
 * 读取 ~/.codex/config.toml 原文件文本.
 * 与 useClaudeCodeSettings 一致: 窗口聚焦时 refetch, 用户从 VS Code 改了 toml 后切回也能拿到最新.
 */
export function useCodexConfig() {
  return useQuery({
    queryKey: CODEX_CONFIG_READ_KEY,
    queryFn: () => api.readCodexConfig(),
    refetchOnWindowFocus: true,
    staleTime: 30_000,
  });
}

export function useCodexConfigStatus() {
  return useQuery({
    queryKey: CODEX_CONFIG_STATUS_KEY,
    queryFn: () => api.inspectCodexConfig(),
    refetchOnWindowFocus: true,
    refetchInterval: 30_000,
  });
}

export function useCodexAuth() {
  return useQuery({
    queryKey: CODEX_AUTH_READ_KEY,
    queryFn: () => api.readCodexAuth(),
    refetchOnWindowFocus: true,
    staleTime: 30_000,
  });
}

export function useCodexAuthStatus() {
  return useQuery({
    queryKey: CODEX_AUTH_STATUS_KEY,
    queryFn: () => api.inspectCodexAuth(),
    refetchOnWindowFocus: true,
    refetchInterval: 30_000,
  });
}

/**
 * 写 config.toml. onSuccess 立即更新 read cache, 避免编辑器闪烁回旧值.
 */
export function useApplyCodexConfig() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (newContent: string) => api.writeCodexConfig(newContent),
    onSuccess: (outcome, newContent) => {
      qc.setQueryData<CodexReadResult>(CODEX_CONFIG_READ_KEY, {
        path: outcome.path,
        content: newContent,
      });
      qc.invalidateQueries({ queryKey: CODEX_CONFIG_STATUS_KEY });
    },
  });
}

export function useApplyCodexAuth() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (newContent: string) => api.writeCodexAuth(newContent),
    onSuccess: (outcome, newContent) => {
      qc.setQueryData<CodexReadResult>(CODEX_AUTH_READ_KEY, {
        path: outcome.path,
        content: newContent,
      });
      qc.invalidateQueries({ queryKey: CODEX_AUTH_STATUS_KEY });
    },
  });
}
