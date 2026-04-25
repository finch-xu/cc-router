import { useEffect, useMemo, useState } from "react";
import { RefreshCw, ScrollText } from "lucide-react";
import { EmptyState } from "@/components/EmptyState";
import { ProviderLogo } from "@/components/ProviderLogo";
import { useRequests } from "@/hooks/useRequests";
import { useProviders } from "@/hooks/useProviders";
import { fmtNum, fmtKilo, fmtTime } from "@/lib/format";
import { VM_META } from "@/lib/virtualModels";
import type {
  RequestLogDto,
  RequestLogFilters,
  RequestStatus,
  VirtualModelName,
} from "@/types";

const PAGE_SIZE = 10;
const ALL = "__all__";
const SPARK_BUCKETS = 14;
const EMPTY_ITEMS: RequestLogDto[] = [];

const STATUS_TONE: Record<RequestStatus, { tone: "ok" | "warn" | "err"; label: string }> = {
  success: { tone: "ok",   label: "成功" },
  timeout: { tone: "warn", label: "超时" },
  error:   { tone: "err",  label: "失败" },
};

/** 把最近 N 条延迟样本均匀分桶,产出 SPARK_BUCKETS 高度数组(0-1) */
function buildSparkline(items: RequestLogDto[]): number[] {
  const lats = items
    .filter((r) => r.total_latency_ms != null)
    .slice(0, 60)
    .map((r) => r.total_latency_ms! / 1000)
    .reverse();
  if (lats.length === 0) return new Array(SPARK_BUCKETS).fill(0);
  if (lats.length <= SPARK_BUCKETS) {
    const max = Math.max(...lats, 0.001);
    return lats.map((v) => v / max);
  }
  const bucketSize = lats.length / SPARK_BUCKETS;
  const buckets: number[] = [];
  for (let i = 0; i < SPARK_BUCKETS; i++) {
    const start = Math.floor(i * bucketSize);
    const end = Math.floor((i + 1) * bucketSize);
    const slice = lats.slice(start, end);
    const avg = slice.reduce((a, b) => a + b, 0) / Math.max(1, slice.length);
    buckets.push(avg);
  }
  const max = Math.max(...buckets, 0.001);
  return buckets.map((v) => v / max);
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
  const providerOf = (id: string) => providers.data?.find((p) => p.id === id);

  const total = query.data?.total ?? 0;
  const totalPages = Math.max(1, Math.ceil(total / PAGE_SIZE));
  const items = query.data?.items ?? EMPTY_ITEMS;
  const hasActiveFilter = !!filters;

  useEffect(() => {
    if (query.data && total > 0 && page > totalPages) setPage(totalPages);
  }, [query.data, total, totalPages, page]);

  function clearFilters() {
    setVmFilter(undefined);
    setProviderFilter(undefined);
    setStatusFilter(undefined);
    setPage(1);
  }

  // 4 个 stat 是当前页的本地估算(不是后端聚合) —— 切页/筛选会变
  const pageStats = useMemo(() => {
    const successCount = items.filter((i) => i.status === "success").length;
    const failCount = items.length - successCount;
    const successRate = items.length > 0 ? (successCount / items.length) * 100 : 0;
    const lats = items.filter((i) => i.total_latency_ms != null).map((i) => i.total_latency_ms!);
    const avgLat = lats.length > 0 ? lats.reduce((a, b) => a + b, 0) / lats.length / 1000 : 0;
    const totalOut = items.reduce((sum, i) => sum + (i.output_tokens ?? 0), 0);
    const totalIn = items.reduce((sum, i) => sum + (i.input_tokens ?? 0), 0);
    return { successRate, failCount, avgLat, totalIn, totalOut };
  }, [items]);

  const spark = useMemo(() => buildSparkline(items), [items]);

  return (
    <>
      <div className="page-actions">
        <div className="page-header" style={{ margin: 0 }}>
          <h1>请求日志</h1>
          <div className="subtitle">最近经过代理的请求记录,按时间倒序。</div>
        </div>
        <button
          className="btn"
          onClick={() => query.refetch()}
          disabled={query.isFetching}
          type="button"
        >
          <RefreshCw size={12} className={query.isFetching ? "spin" : undefined} />
          刷新
        </button>
      </div>

      <div className="log-stats">
        <div className="stat">
          <div className="stat-label">总请求</div>
          <div className="stat-val tnum">{fmtNum(total)}</div>
          <div className="stat-delta">本页 {items.length} 条</div>
        </div>
        <div className="stat">
          <div className="stat-label">成功率</div>
          <div className="stat-val tnum">
            {pageStats.successRate.toFixed(1)}
            <span style={{ fontSize: 14, color: "var(--ink-3)" }}>%</span>
          </div>
          <div className={"stat-delta" + (pageStats.failCount > 0 ? " down" : "")}>
            {pageStats.failCount} 失败 / {items.length}
          </div>
        </div>
        <div className="stat">
          <div className="stat-label">平均延迟</div>
          <div className="stat-val tnum">
            {pageStats.avgLat > 0 ? pageStats.avgLat.toFixed(2) : "-"}
            <span style={{ fontSize: 14, color: "var(--ink-3)" }}>s</span>
          </div>
          <div className="spark">
            {spark.map((v, i) => (
              <span key={i} style={{ height: `${Math.max(8, v * 100)}%` }} />
            ))}
          </div>
        </div>
        <div className="stat">
          <div className="stat-label">总 Token (输出)</div>
          <div className="stat-val tnum">{fmtKilo(pageStats.totalOut)}</div>
          <div className="stat-delta">≈ {fmtKilo(pageStats.totalIn)} 输入</div>
        </div>
      </div>

      <div className="log-filters">
        <select
          className="select"
          value={vmFilter ?? ALL}
          onChange={(e) => {
            const v = e.target.value;
            setVmFilter(v === ALL ? undefined : (v as VirtualModelName));
            setPage(1);
          }}
        >
          <option value={ALL}>全部虚拟模型</option>
          {(Object.keys(VM_META) as VirtualModelName[]).map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </select>

        <select
          className="select"
          value={providerFilter ?? ALL}
          onChange={(e) => {
            const v = e.target.value;
            setProviderFilter(v === ALL ? undefined : v);
            setPage(1);
          }}
        >
          <option value={ALL}>全部厂商</option>
          {providers.data?.map((p) => (
            <option key={p.id} value={p.id}>
              {p.display_name}
            </option>
          ))}
        </select>

        <select
          className="select"
          value={statusFilter ?? ALL}
          onChange={(e) => {
            const v = e.target.value;
            setStatusFilter(v === ALL ? undefined : (v as RequestStatus));
            setPage(1);
          }}
        >
          <option value={ALL}>全部状态</option>
          {(Object.keys(STATUS_TONE) as RequestStatus[]).map((s) => (
            <option key={s} value={s}>
              {STATUS_TONE[s].label}
            </option>
          ))}
        </select>

        <span
          className="mono"
          style={{ marginLeft: "auto", fontSize: 12, color: "var(--ink-3)" }}
        >
          {total > 0 ? `${total} 条 · 第 ${page} 页 / 共 ${totalPages} 页` : "0 条"}
        </span>
      </div>

      {query.isLoading && <div className="field-hint">加载中…</div>}

      {query.data && total === 0 && (
        <EmptyState
          icon={ScrollText}
          message={
            hasActiveFilter
              ? "当前筛选下无记录。"
              : "暂无请求日志。让 Claude Code 通过代理跑一次对话即可看到记录。"
          }
          action={
            hasActiveFilter ? (
              <button className="btn sm" onClick={clearFilters} type="button">
                清除筛选
              </button>
            ) : undefined
          }
        />
      )}

      {items.length > 0 && (
        <div className="card">
          <table className="table">
            <thead>
              <tr>
                <th style={{ width: 150 }}>时间</th>
                <th style={{ width: 110 }}>状态</th>
                <th style={{ width: 130 }}>虚拟模型</th>
                <th style={{ width: 200 }}>真实模型</th>
                <th>响应模型</th>
                <th style={{ width: 140 }}>厂商</th>
                <th style={{ width: 70, textAlign: "right" }}>输入</th>
                <th style={{ width: 70, textAlign: "right" }}>输出</th>
                <th style={{ width: 90, textAlign: "right" }}>延迟</th>
              </tr>
            </thead>
            <tbody>
              {items.map((row) => {
                const lat = (row.total_latency_ms ?? 0) / 1000;
                const provider = providerOf(row.provider_id);
                const status = STATUS_TONE[row.status];
                return (
                  <tr key={row.id}>
                    <td className="mono muted">{fmtTime(row.timestamp)}</td>
                    <td>
                      <div style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
                        <span className={`pill ${status.tone}`}>
                          <span className="dot" />
                          {status.label}
                        </span>
                        {row.is_streaming && (
                          <span className="pill tag mono" title="SSE 流式">
                            SSE
                          </span>
                        )}
                      </div>
                    </td>
                    <td className="mono">{row.virtual_model_name}</td>
                    <td className="mono strong">{row.real_model_name}</td>
                    <td className="mono muted">{row.response_model_name ?? "—"}</td>
                    <td>
                      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                        <ProviderLogo iconId={provider?.icon} size={18} iconSize={12} />
                        <span style={{ fontSize: 12.5 }}>
                          {provider?.display_name ?? row.provider_id}
                        </span>
                      </div>
                    </td>
                    <td className="mono tnum muted" style={{ textAlign: "right" }}>
                      {fmtNum(row.input_tokens)}
                    </td>
                    <td className="mono tnum strong" style={{ textAlign: "right" }}>
                      {fmtNum(row.output_tokens)}
                    </td>
                    <td className="mono tnum" style={{ textAlign: "right" }}>
                      {row.total_latency_ms != null ? `${lat.toFixed(2)}s` : "-"}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      {query.data && total > 0 && (
        <div
          style={{
            display: "flex",
            justifyContent: "flex-end",
            gap: 8,
            marginTop: 16,
            alignItems: "center",
          }}
        >
          {hasActiveFilter && (
            <button
              className="btn sm"
              onClick={clearFilters}
              type="button"
              style={{ marginRight: "auto" }}
            >
              清除筛选
            </button>
          )}
          <button
            className="btn sm"
            disabled={page <= 1}
            onClick={() => setPage((p) => Math.max(1, p - 1))}
            type="button"
          >
            上一页
          </button>
          <button
            className="btn sm"
            disabled={page >= totalPages}
            onClick={() => setPage((p) => p + 1)}
            type="button"
          >
            下一页
          </button>
        </div>
      )}
    </>
  );
}
