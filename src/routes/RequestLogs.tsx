import { useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { RefreshCw, ScrollText, X } from "lucide-react";
import { ClientToolBadge } from "@/components/ClientToolBadge";
import { EmptyState } from "@/components/EmptyState";
import { Pagination } from "@/components/Pagination";
import { ProviderLogo } from "@/components/ProviderLogo";
import { RequestDetailDialog } from "@/components/RequestDetailDialog";
import { useRequests } from "@/hooks/useRequests";
import { useProviders } from "@/hooks/useProviders";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useT } from "@/i18n";
import { fmtNum, fmtTime } from "@/lib/format";
import { VM_META } from "@/lib/virtualModels";
import { CLIENT_TOOLS } from "@/lib/clientTools";
import {
  CLIENT_TOOL_UNKNOWN_SENTINEL,
  type RequestLogDto,
  type RequestLogFilters,
  type RequestStatus,
  type VirtualModelName,
} from "@/types";

const PAGE_SIZE = 10;
const ALL = "__all__";
const EMPTY_ITEMS: RequestLogDto[] = [];

const STATUS_TONE: Record<RequestStatus, { tone: "ok" | "warn" | "err"; labelKey: string }> = {
  success: { tone: "ok",   labelKey: "requestLogs.status.success" },
  timeout: { tone: "warn", labelKey: "requestLogs.status.timeout" },
  error:   { tone: "err",  labelKey: "requestLogs.status.error" },
};

export function RequestLogsPage() {
  const { t } = useT();
  const [searchParams, setSearchParams] = useSearchParams();
  const [page, setPage] = useState(1);
  const [vmFilter, setVmFilter] = useState<VirtualModelName | undefined>();
  const [providerFilter, setProviderFilter] = useState<string | undefined>();
  const [statusFilter, setStatusFilter] = useState<RequestStatus | undefined>();
  const [clientFilter, setClientFilter] = useState<string | undefined>();
  const subscriptionFilter = searchParams.get("subscription_id") ?? undefined;
  const [activeRequest, setActiveRequest] = useState<RequestLogDto | null>(null);
  const subs = useSubscriptions();
  const subFilterLabel = subscriptionFilter
    ? subs.data?.find((s) => s.id === subscriptionFilter)?.display_name ?? subscriptionFilter
    : undefined;

  const filters = useMemo<RequestLogFilters | undefined>(() => {
    if (!vmFilter && !providerFilter && !statusFilter && !clientFilter && !subscriptionFilter)
      return undefined;
    return {
      virtual_model_name: vmFilter,
      provider_id: providerFilter,
      status: statusFilter,
      client_tool: clientFilter,
      subscription_id: subscriptionFilter,
    };
  }, [vmFilter, providerFilter, statusFilter, clientFilter, subscriptionFilter]);

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
    setClientFilter(undefined);
    setSearchParams({});
    setPage(1);
  }

  function clearSubFilter() {
    const next = new URLSearchParams(searchParams);
    next.delete("subscription_id");
    setSearchParams(next);
    setPage(1);
  }

  return (
    <>
      <div className="page-actions">
        <div className="page-header" style={{ margin: 0 }}>
          <h1>{t("requestLogs.title")}</h1>
          <div className="subtitle">{t("requestLogs.subtitle")}</div>
        </div>
        <button
          className="btn"
          onClick={() => query.refetch()}
          disabled={query.isFetching}
          type="button"
        >
          <RefreshCw size={12} className={query.isFetching ? "spin" : undefined} />
          {t("requestLogs.refresh")}
        </button>
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
          <option value={ALL}>{t("requestLogs.filter.allVm")}</option>
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
          <option value={ALL}>{t("requestLogs.filter.allProvider")}</option>
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
          <option value={ALL}>{t("requestLogs.filter.allStatus")}</option>
          {(Object.keys(STATUS_TONE) as RequestStatus[]).map((s) => (
            <option key={s} value={s}>
              {t(STATUS_TONE[s].labelKey)}
            </option>
          ))}
        </select>

        <select
          className="select"
          value={clientFilter ?? ALL}
          onChange={(e) => {
            const v = e.target.value;
            setClientFilter(v === ALL ? undefined : v);
            setPage(1);
          }}
        >
          <option value={ALL}>{t("requestLogs.filter.allClient")}</option>
          {CLIENT_TOOLS.map((c) => (
            <option key={c.id} value={c.id}>
              {t(c.i18nKey)}
            </option>
          ))}
          <option value={CLIENT_TOOL_UNKNOWN_SENTINEL}>
            {t("requestLogs.filter.clientUnknown")}
          </option>
        </select>

        {subscriptionFilter && (
          <span className="pill accent" style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
            <span className="dot" />
            {subFilterLabel}
            <button
              type="button"
              onClick={clearSubFilter}
              style={{ background: "none", border: "none", color: "inherit", cursor: "pointer", padding: 0, marginLeft: 2, display: "inline-flex" }}
              aria-label="clear subscription filter"
            >
              <X size={12} />
            </button>
          </span>
        )}

        <span
          className="mono"
          style={{ marginLeft: "auto", fontSize: 12, color: "var(--ink-3)" }}
        >
          {total > 0
            ? t("requestLogs.summaryFormat", { total, page, pages: totalPages })
            : t("requestLogs.summaryEmpty")}
        </span>
      </div>

      {query.isLoading && <div className="field-hint">{t("common.loading")}</div>}

      {query.data && total === 0 && (
        <EmptyState
          icon={ScrollText}
          message={
            hasActiveFilter
              ? t("requestLogs.empty.filtered")
              : t("requestLogs.empty.none")
          }
          action={
            hasActiveFilter ? (
              <button className="btn sm" onClick={clearFilters} type="button">
                {t("requestLogs.clearFilter")}
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
                <th style={{ width: 150 }}>{t("requestLogs.col.time")}</th>
                <th style={{ width: 110 }}>{t("requestLogs.col.status")}</th>
                <th style={{ width: 130 }}>{t("requestLogs.col.client")}</th>
                <th style={{ width: 130 }}>{t("requestLogs.col.virtualModel")}</th>
                <th style={{ width: 200 }}>{t("requestLogs.col.realModel")}</th>
                <th>{t("requestLogs.col.responseModel")}</th>
                <th style={{ width: 140 }}>{t("requestLogs.col.provider")}</th>
                <th style={{ width: 70, textAlign: "right" }}>{t("requestLogs.col.tokensIn")}</th>
                <th style={{ width: 70, textAlign: "right" }}>{t("requestLogs.col.tokensOut")}</th>
                <th style={{ width: 90, textAlign: "right" }}>{t("requestLogs.col.latency")}</th>
              </tr>
            </thead>
            <tbody>
              {items.map((row) => {
                const lat = (row.total_latency_ms ?? 0) / 1000;
                const provider = providerOf(row.provider_id);
                const status = STATUS_TONE[row.status];
                const isError = row.status !== "success";
                return (
                  <tr
                    key={row.id}
                    onClick={() => setActiveRequest(row)}
                    style={{ cursor: "pointer" }}
                  >
                    <td className="mono muted">{fmtTime(row.timestamp)}</td>
                    <td>
                      <div style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
                        <span className={`pill ${status.tone}`}>
                          <span className="dot" />
                          {t(status.labelKey)}
                        </span>
                        {isError && row.http_status != null && (
                          <span className="pill tag mono">{row.http_status}</span>
                        )}
                        {row.is_streaming && (
                          <span className="pill tag mono" title={t("requestLogs.sse")}>
                            SSE
                          </span>
                        )}
                      </div>
                    </td>
                    <td>
                      <ClientToolBadge
                        toolId={row.client_tool}
                        userAgent={row.client_user_agent}
                      />
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

      <Pagination
        total={total}
        page={page}
        totalPages={totalPages}
        onChange={setPage}
        trailing={
          hasActiveFilter ? (
            <button
              className="btn sm"
              onClick={clearFilters}
              type="button"
              style={{ marginRight: "auto" }}
            >
              {t("requestLogs.clearFilter")}
            </button>
          ) : undefined
        }
      />

      <RequestDetailDialog
        request={activeRequest}
        onClose={() => setActiveRequest(null)}
      />
    </>
  );
}
