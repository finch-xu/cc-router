import { useEffect, useMemo, useState } from "react";
import { RefreshCw, ScrollText } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useRequests } from "@/hooks/useRequests";
import { useProviders } from "@/hooks/useProviders";
import type {
  RequestLogFilters,
  RequestStatus,
  VirtualModelName,
} from "@/types";

const PAGE_SIZE = 50;
const ALL = "__all__";

const VM_LABEL: Record<VirtualModelName, string> = {
  "model-opus": "高级任务 / Plan Mode",
  "model-sonnet": "主对话",
  "model-haiku": "小任务 / 工具调用",
  "model-fallback": "兜底 · 未知模型透传",
};

const STATUS_LABEL: Record<RequestStatus, string> = {
  success: "成功",
  error: "错误",
  timeout: "超时",
};

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
  const [vmFilter, setVmFilter] = useState<VirtualModelName | undefined>();
  const [providerFilter, setProviderFilter] = useState<string | undefined>();
  const [statusFilter, setStatusFilter] = useState<RequestStatus | undefined>();

  const filters = useMemo<RequestLogFilters | undefined>(() => {
    if (!vmFilter && !providerFilter && !statusFilter) return undefined;
    return {
      virtual_model_name: vmFilter,
      provider_id: providerFilter,
      status: statusFilter,
    };
  }, [vmFilter, providerFilter, statusFilter]);

  const query = useRequests(page, PAGE_SIZE, filters);
  const providers = useProviders();

  const providerName = (id: string) =>
    providers.data?.find((p) => p.id === id)?.display_name ?? "unknown";

  const total = query.data?.total ?? 0;
  const totalPages = Math.max(1, Math.ceil(total / PAGE_SIZE));
  const items = query.data?.items ?? [];
  const hasActiveFilter = !!filters;

  // 当筛选收窄让当前 page 超出总页数时，自动夹回。不 reset 到 1 是为了用户翻页体验。
  useEffect(() => {
    if (query.data && total > 0 && page > totalPages) {
      setPage(totalPages);
    }
  }, [query.data, total, totalPages, page]);

  function resetPageAnd(setter: () => void) {
    setter();
    setPage(1);
  }

  function clearFilters() {
    setVmFilter(undefined);
    setProviderFilter(undefined);
    setStatusFilter(undefined);
    setPage(1);
  }

  return (
    <div className="p-8 space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold">请求日志</h1>
          <p className="text-sm text-muted-foreground">
            最近经过代理的请求记录，按时间倒序
          </p>
        </div>
        <Button
          variant="outline"
          size="sm"
          onClick={() => query.refetch()}
          disabled={query.isFetching}
        >
          <RefreshCw
            className={
              query.isFetching ? "h-4 w-4 animate-spin" : "h-4 w-4"
            }
          />
          刷新
        </Button>
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <Select
          value={vmFilter ?? ALL}
          onValueChange={(v) =>
            resetPageAnd(() =>
              setVmFilter(v === ALL ? undefined : (v as VirtualModelName)),
            )
          }
        >
          <SelectTrigger className="w-[200px]">
            <SelectValue placeholder="虚拟模型" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ALL}>全部虚拟模型</SelectItem>
            {(Object.keys(VM_LABEL) as VirtualModelName[]).map((name) => (
              <SelectItem key={name} value={name}>
                <span className="font-mono text-xs">{name}</span>
                <span className="text-xs text-muted-foreground ml-2">
                  {VM_LABEL[name]}
                </span>
              </SelectItem>
            ))}
          </SelectContent>
        </Select>

        <Select
          value={providerFilter ?? ALL}
          onValueChange={(v) =>
            resetPageAnd(() =>
              setProviderFilter(v === ALL ? undefined : v),
            )
          }
        >
          <SelectTrigger className="w-[200px]">
            <SelectValue placeholder="厂商" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ALL}>全部厂商</SelectItem>
            {providers.data?.map((p) => (
              <SelectItem key={p.id} value={p.id}>
                {p.display_name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>

        <Select
          value={statusFilter ?? ALL}
          onValueChange={(v) =>
            resetPageAnd(() =>
              setStatusFilter(v === ALL ? undefined : (v as RequestStatus)),
            )
          }
        >
          <SelectTrigger className="w-[140px]">
            <SelectValue placeholder="状态" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ALL}>全部状态</SelectItem>
            {(Object.keys(STATUS_LABEL) as RequestStatus[]).map((s) => (
              <SelectItem key={s} value={s}>
                {STATUS_LABEL[s]}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>

        {hasActiveFilter && (
          <Button variant="ghost" size="sm" onClick={clearFilters}>
            清除筛选
          </Button>
        )}
      </div>

      {query.isLoading && (
        <div className="text-sm text-muted-foreground">加载中…</div>
      )}

      {query.data && total === 0 && (
        <Card>
          <CardContent className="py-12 text-center space-y-3">
            <ScrollText className="h-8 w-8 mx-auto text-muted-foreground" />
            <div className="text-sm text-muted-foreground">
              {hasActiveFilter
                ? "当前筛选下无记录。"
                : "暂无请求日志。让 Claude Code 通过代理跑一次对话即可看到记录。"}
            </div>
            {hasActiveFilter && (
              <Button variant="outline" size="sm" onClick={clearFilters}>
                清除筛选
              </Button>
            )}
          </CardContent>
        </Card>
      )}

      {items.length > 0 && (
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
      )}

      {query.data && total > 0 && (
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
      )}
    </div>
  );
}
