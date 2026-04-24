import { useState } from "react";
import { ScrollText } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { useRequests } from "@/hooks/useRequests";
import { useProviders } from "@/hooks/useProviders";
import type { RequestStatus } from "@/types";

const PAGE_SIZE = 50;

function statusBadge(status: RequestStatus) {
  switch (status) {
    case "success":
      return <Badge>成功</Badge>;
    case "error":
      return <Badge variant="destructive">错误</Badge>;
    case "timeout":
      return <Badge variant="secondary">超时</Badge>;
  }
}

function fmtTokens(n?: number) {
  if (n === undefined || n === null) return "-";
  return n.toLocaleString("zh-CN");
}

function fmtLatency(ms?: number) {
  if (ms === undefined || ms === null) return "-";
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(2)}s`;
}

function fmtTime(ms: number) {
  return new Date(ms).toLocaleString("zh-CN", { hour12: false });
}

export function RequestLogsPage() {
  const [page, setPage] = useState(1);
  const query = useRequests(page, PAGE_SIZE);
  const providers = useProviders();

  const providerName = (id: string) =>
    providers.data?.find((p) => p.id === id)?.display_name ?? "unknown";

  const total = query.data?.total ?? 0;
  const totalPages = Math.max(1, Math.ceil(total / PAGE_SIZE));
  const items = query.data?.items ?? [];

  return (
    <div className="p-8 space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold">请求日志</h1>
          <p className="text-sm text-muted-foreground">
            最近经过代理的请求记录，按时间倒序
          </p>
        </div>
      </div>

      {query.isLoading && (
        <div className="text-sm text-muted-foreground">加载中…</div>
      )}

      {query.data && items.length === 0 && (
        <Card>
          <CardContent className="py-12 text-center space-y-3">
            <ScrollText className="h-8 w-8 mx-auto text-muted-foreground" />
            <div className="text-sm text-muted-foreground">
              暂无请求日志。让 Claude Code 通过代理跑一次对话即可看到记录。
            </div>
          </CardContent>
        </Card>
      )}

      {items.length > 0 && (
        <>
          <div className="rounded-lg border overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="border-b bg-muted/40 text-xs uppercase text-muted-foreground">
                <tr>
                  <th className="px-4 py-2 text-left font-medium whitespace-nowrap">时间</th>
                  <th className="px-4 py-2 text-left font-medium">状态</th>
                  <th className="px-4 py-2 text-left font-medium">虚拟模型</th>
                  <th className="px-4 py-2 text-left font-medium">真实模型</th>
                  <th className="px-4 py-2 text-left font-medium">厂商</th>
                  <th className="px-4 py-2 text-right font-medium">输入</th>
                  <th className="px-4 py-2 text-right font-medium">输出</th>
                  <th className="px-4 py-2 text-right font-medium">延迟</th>
                </tr>
              </thead>
              <tbody>
                {items.map((row) => (
                  <tr
                    key={row.id}
                    className="border-b last:border-b-0 hover:bg-muted/20"
                  >
                    <td className="px-4 py-2 whitespace-nowrap font-mono text-xs">
                      {fmtTime(row.timestamp)}
                    </td>
                    <td className="px-4 py-2">{statusBadge(row.status)}</td>
                    <td className="px-4 py-2 font-mono text-xs">
                      {row.virtual_model_name}
                    </td>
                    <td className="px-4 py-2 font-mono text-xs">
                      {row.real_model_name}
                    </td>
                    <td className="px-4 py-2">{providerName(row.provider_id)}</td>
                    <td className="px-4 py-2 text-right font-mono text-xs">
                      {fmtTokens(row.input_tokens)}
                    </td>
                    <td className="px-4 py-2 text-right font-mono text-xs">
                      {fmtTokens(row.output_tokens)}
                    </td>
                    <td className="px-4 py-2 text-right font-mono text-xs">
                      {fmtLatency(row.total_latency_ms)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          <div className="flex items-center justify-between text-sm text-muted-foreground">
            <div>
              第 {page} / {totalPages} 页 · 共 {total} 条
            </div>
            <div className="flex gap-2">
              <Button
                variant="outline"
                size="sm"
                disabled={page <= 1}
                onClick={() => setPage((p) => Math.max(1, p - 1))}
              >
                上一页
              </Button>
              <Button
                variant="outline"
                size="sm"
                disabled={page >= totalPages}
                onClick={() => setPage((p) => p + 1)}
              >
                下一页
              </Button>
            </div>
          </div>
        </>
      )}
    </div>
  );
}
