import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "@/api/tauri";
import type { ClaudeCodeReadResult } from "@/types";

export const CLAUDE_CODE_SETTINGS_KEY = ["claude-code-settings"] as const;
export const CLAUDE_CODE_STATUS_KEY = ["claude-code-settings", "status"] as const;

/**
 * 读取 ~/.claude/settings.json 原文件文本.
 * 30s staleTime + 窗口聚焦时 refetch — 用户在外部编辑器 (vim/VS Code) 改了文件后,
 * 切回 cc-router Guide 页会自动拉到最新, 避免编辑器拿陈旧内容覆盖用户外部修改.
 */
export function useClaudeCodeSettings() {
  return useQuery({
    queryKey: CLAUDE_CODE_SETTINGS_KEY,
    queryFn: () => api.readClaudeCodeSettings(),
    refetchOnWindowFocus: true,
    staleTime: 30_000,
  });
}

/** 探测同步状态. 窗口聚焦或 30s 周期 refetch, 让徽章及时反映外部改动. */
export function useClaudeCodeStatus() {
  return useQuery({
    queryKey: CLAUDE_CODE_STATUS_KEY,
    queryFn: () => api.inspectClaudeCodeSettings(),
    refetchOnWindowFocus: true,
    refetchInterval: 30_000,
  });
}

/**
 * 把整文件文本写回 ~/.claude/settings.json (原子写 + 备份判定).
 *
 * onSuccess 直接 setQueryData 把新文本塞进 read cache, 避免编辑器在 refetch 落地前
 * 短暂回退到 stale serverContent — 视觉上会闪一下旧内容再跳到新内容.
 */
export function useApplyClaudeCodeSettings() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (newContent: string) => api.writeClaudeCodeSettings(newContent),
    onSuccess: (outcome, newContent) => {
      queryClient.setQueryData<ClaudeCodeReadResult>(CLAUDE_CODE_SETTINGS_KEY, {
        path: outcome.path,
        content: newContent,
      });
      // status 仍需 invalidate — 同步徽章
      queryClient.invalidateQueries({ queryKey: CLAUDE_CODE_STATUS_KEY });
    },
  });
}
