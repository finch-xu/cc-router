import { useEffect, useMemo, useState } from "react";
import * as TabsPrimitive from "@radix-ui/react-tabs";
import { useQuery } from "@tanstack/react-query";
import { Copy, Check } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ClientToolBadge } from "@/components/ClientToolBadge";
import { HeaderTable, type HeaderTag } from "@/components/request-detail/HeaderTable";
import { api } from "@/api/tauri";
import { isReturnedToClient } from "@/lib/headerSources";
import { ProviderLogo } from "@/components/ProviderLogo";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useT } from "@/i18n";
import { fmtNum, fmtTime, fmtRequestSeconds, fmtTokenSpeed } from "@/lib/format";
import { customProviderLabel, providerIconId } from "@/lib/providerLabels";
import { providerName as localProviderName } from "@/lib/providerText";
import type { HeaderPairs, OutboundHeaderRows, OutboundSource, RequestLogDto } from "@/types";
import { TOOL_NAME_TRUNCATED_MARKER } from "@/types";

interface Props {
  request: RequestLogDto | null;
  onClose: () => void;
}

/**
 * 「思考强度」一行的展示文案: 客户端请求 → 实际发往上游(来源) · 上游回显。
 * 四项全空 (老日志 / 该 provider 无 effort 概念) 返回 null, 调用方渲染 "—"。
 */
function effortSummary(
  request: RequestLogDto,
  t: (key: string) => string,
): string | null {
  const { client_effort, effective_effort, effort_source, upstream_effort } = request;
  if (!client_effort && !effective_effort && !effort_source && !upstream_effort) return null;

  const dash = "—";
  let text = `${t("requestLogs.detail.effortClient")} ${client_effort || dash} → ${t(
    "requestLogs.detail.effortEffective",
  )} ${effective_effort || dash}`;

  const sourceKey = `requestLogs.detail.effortSource.${effort_source ?? ""}`;
  const sourceLabel = effort_source ? t(sourceKey) : "";
  // t() 找不到 key 时会原样回落成 key 本身, 未知来源不显示括号
  if (sourceLabel && sourceLabel !== sourceKey) text += `（${sourceLabel}）`;

  text += upstream_effort
    ? ` · ${t("requestLogs.detail.effortUpstream")} ${upstream_effort}`
    : ` · ${t("requestLogs.detail.effortUpstreamNone")}`;
  return text;
}

/** tool_use_names JSON → [名称, 次数][] 保序去重; 末尾 "…" 标记单独返回 */
function parseToolNames(raw?: string | null): { chips: [string, number][]; truncated: boolean } {
  if (!raw) return { chips: [], truncated: false };
  let names: unknown;
  try {
    names = JSON.parse(raw);
  } catch {
    return { chips: [], truncated: false };
  }
  if (!Array.isArray(names)) return { chips: [], truncated: false };
  const counts = new Map<string, number>();
  let truncated = false;
  for (const n of names) {
    if (n === TOOL_NAME_TRUNCATED_MARKER) {
      truncated = true;
      continue;
    }
    const key = typeof n === "string" && n ? n : "(unnamed)";
    counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  return { chips: [...counts.entries()], truncated };
}

function hasToolInfo(r: RequestLogDto): boolean {
  return (
    r.stop_reason != null ||
    r.tools_offered_count != null ||
    r.tool_result_count != null ||
    r.tool_use_count != null ||
    r.tool_use_names != null
  );
}

export function RequestDetailDialog({ request, onClose }: Props) {
  const { t, locale } = useT();
  const [copied, setCopied] = useState(false);
  const subs = useSubscriptions();

  useEffect(() => {
    if (!request) setCopied(false);
  }, [request]);

  const open = request !== null;
  const isError = request && request.status !== "success";
  const [tab, setTab] = useState("overview");
  useEffect(() => {
    if (request) setTab("overview");
  }, [request?.id]);

  const headersQuery = useQuery({
    queryKey: ["requestHeaders", request?.id],
    queryFn: () => api.getRequestHeaders(request!.id),
    enabled: open,
    staleTime: Infinity,
  });
  const headers = headersQuery.data;

  const prettyBody = useMemo(() => {
    if (!request?.upstream_response_body) return null;
    try {
      const parsed = JSON.parse(request.upstream_response_body);
      return JSON.stringify(parsed, null, 2);
    } catch {
      return request.upstream_response_body;
    }
  }, [request]);

  // 订阅可能已被删除: 找不到时退回厂商名
  const sub = request ? subs.data?.find((s) => s.id === request.subscription_id) : undefined;
  const providerName = request
    ? (sub && localProviderName(sub, locale)) ?? customProviderLabel(request.provider_id, t) ?? request.provider_id
    : "";

  async function copyAll() {
    if (!request) return;
    const lines = [
      `request_id: ${request.id}`,
      `time: ${fmtTime(request.timestamp)}`,
      `virtual_model: ${request.virtual_model_name}`,
      `provider: ${customProviderLabel(request.provider_id, t) ?? request.provider_id}`,
      `subscription: ${sub?.display_name ?? request.subscription_id}`,
      `real_model: ${request.real_model_name}`,
      `effort: ${effortSummary(request, t) ?? "—"}`,
      `status: ${request.status}`,
      `total_latency: ${fmtRequestSeconds(request.total_latency_ms)}`,
      `first_content: ${fmtRequestSeconds(request.first_content_ms)}`,
      `output_speed: ${fmtTokenSpeed(request.output_tokens_per_second)}`,
      `http_status: ${request.http_status ?? "—"}`,
      `stop_reason: ${request.stop_reason ?? "—"}`,
      `tools_offered: ${request.tools_offered_count ?? "—"} · tool_results: ${request.tool_result_count ?? "—"} · tool_uses: ${request.tool_use_count ?? "—"}`,
      `tool_use_names: ${request.tool_use_names ?? "—"}`,
      `error_message: ${request.error_message ?? ""}`,
      "",
      "upstream_response_body:",
      prettyBody ?? "",
    ];
    const section = (name: string, pairs: HeaderPairs | OutboundHeaderRows | undefined) => {
      if (!pairs) return;
      lines.push("", `${name}:`, ...pairs.map(([k, v, src]) => (src ? `${k}: ${v}  [${src}]` : `${k}: ${v}`)));
    };
    section("client_request_headers", headers?.client_request);
    section("upstream_request_headers", headers?.upstream_request);
    section("upstream_response_headers", headers?.upstream_response);
    try {
      await navigator.clipboard.writeText(lines.join("\n"));
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // ignore
    }
  }

  const dash = <span className="muted">—</span>;
  const monoOr = (v?: string | null) => (v ? <span className="mono">{v}</span> : dash);

  // 发往上游一张表的来源标签: 后端组装时记录, 这里只映射文案与色调
  const outboundTag = ([, , src]: [string, string, OutboundSource]): HeaderTag => {
    const tone = src === "forwarded" ? "ok" : src === "cc_router" ? "muted" : "accent";
    return { label: t(`requestLogs.detail.headers.source.${src}`), tone };
  };
  // 回传白名单只作用于 Anthropic 透传路径; 翻译类订阅不显示该标签
  const showReturned = sub?.auth_type === "api_key";
  const responseTag = ([name]: [string, string]): HeaderTag | null =>
    showReturned
      ? isReturnedToClient(name)
        ? { label: t("requestLogs.detail.headers.returned"), tone: "ok" }
        : { label: t("requestLogs.detail.headers.notReturned"), tone: "muted" }
      : null;
  const count = (pairs: HeaderPairs | OutboundHeaderRows | undefined) =>
    headersQuery.isLoading ? "…" : pairs ? String(pairs.length) : "—";
  // 某组为 null 的原因: 加载失败 > 整行都没记 (没开记录 / 老日志) > 记了入站但这次尝试没发出或没收到响应
  const recorded = headers?.client_request != null;
  const emptyText = (kind: "client" | "request" | "response") =>
    headersQuery.isError
      ? t("requestLogs.detail.headers.loadFailed")
      : !recorded || kind === "client"
        ? t("requestLogs.detail.headers.notRecorded")
        : kind === "request" || headers?.upstream_request == null
          ? t("requestLogs.detail.headers.notSent")
          : t("requestLogs.detail.headers.noResponse");
  const toolsVisible = request ? hasToolInfo(request) : false;

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent className="cc-dialog rd rd-split">
        <DialogHeader className="rd-dialog-head">
          <DialogTitle>{t("requestLogs.detail.title")}</DialogTitle>
        </DialogHeader>
        {request && (
          <>
            {/* 抬头: 状态 / HTTP / 流式 + 时间与请求 ID, 右侧复制全部 (不随右栏滚动) */}
            <div className="rd-head">
              <div className="rd-head-tags">
                <span className={`pill ${toneOf(request.status)}`}>
                  <span className="dot" />
                  {t(`requestLogs.status.${request.status}`)}
                </span>
                {request.http_status != null && (
                  <span className="pill tag mono" title={t("requestLogs.detail.httpStatus")}>
                    HTTP {request.http_status}
                  </span>
                )}
                <span className="pill tag">
                  {request.is_streaming ? t("requestLogs.detail.streaming") : t("requestLogs.detail.nonStreaming")}
                </span>
                <span className="rd-head-meta">
                  <span className="mono">{fmtTime(request.timestamp)}</span>
                  <span className="rd-sep">·</span>
                  <span className="mono rd-id" title={request.id}>{request.id}</span>
                </span>
              </div>
              <button className="btn sm" type="button" onClick={copyAll}>
                {copied ? <Check size={12} /> : <Copy size={12} />}
                {copied ? t("common.copied") : t("requestLogs.detail.copy")}
              </button>
            </div>

            <TabsPrimitive.Root
              value={tab}
              onValueChange={setTab}
              orientation="vertical"
              className="rd-split-body"
            >
              <TabsPrimitive.List className="rd-nav" aria-label={t("requestLogs.detail.title")}>
                <TabsPrimitive.Trigger value="overview" className="rd-nav-item">
                  {t("requestLogs.detail.nav.overview")}
                </TabsPrimitive.Trigger>
                <TabsPrimitive.Trigger value="request" className="rd-nav-item">
                  {t("requestLogs.detail.nav.request")}
                </TabsPrimitive.Trigger>
                {toolsVisible && (
                  <TabsPrimitive.Trigger value="tools" className="rd-nav-item">
                    {t("requestLogs.detail.tools.title")}
                  </TabsPrimitive.Trigger>
                )}
                <div className="rd-nav-group" role="presentation" aria-hidden="true">{t("requestLogs.detail.nav.headersGroup")}</div>
                <TabsPrimitive.Trigger value="clientHeaders" className="rd-nav-item">
                  <span>{t("requestLogs.detail.nav.clientHeaders")}</span>
                  <span className="rd-nav-n">{count(headers?.client_request)}</span>
                </TabsPrimitive.Trigger>
                <TabsPrimitive.Trigger value="upstreamHeaders" className="rd-nav-item">
                  <span>{t("requestLogs.detail.nav.upstreamHeaders")}</span>
                  <span className="rd-nav-n">{count(headers?.upstream_request)}</span>
                </TabsPrimitive.Trigger>
                <TabsPrimitive.Trigger value="responseHeaders" className="rd-nav-item">
                  <span>{t("requestLogs.detail.nav.responseHeaders")}</span>
                  <span className="rd-nav-n">{count(headers?.upstream_response)}</span>
                </TabsPrimitive.Trigger>
                <div className="rd-nav-group" role="presentation" aria-hidden="true">{t("requestLogs.detail.nav.responseGroup")}</div>
                <TabsPrimitive.Trigger value="body" className="rd-nav-item">
                  {t("requestLogs.detail.nav.body")}
                </TabsPrimitive.Trigger>
              </TabsPrimitive.List>

              <div className="rd-pane">
                <TabsPrimitive.Content value="overview" className="rd-body">
                  <table className="rd-table rd-overview-table">
                    <thead>
                      <tr>
                        <th>{t("requestLogs.detail.headers.item")}</th>
                        <th>{t("requestLogs.detail.headers.value")}</th>
                      </tr>
                    </thead>
                    <tbody>
                      <Row k={t("requestLogs.detail.clientTool")} v={<ClientToolBadge toolId={request.client_tool} userAgent={request.client_user_agent} />} />
                      <Row k={t("requestLogs.detail.clientVersion")} v={monoOr(request.client_version)} />
                      <Row k={t("requestLogs.detail.entryKind")} v={monoOr(request.entry_kind ? `/v1/${request.entry_kind}` : null)} />
                      <Row k={t("requestLogs.detail.virtualModel")} v={monoOr(request.virtual_model_name)} />
                      <Row k={t("requestLogs.detail.subscription")} v={sub?.display_name ?? providerName} />
                      <Row k={t("requestLogs.detail.provider")} v={
                        <span className="rd-provider-value">
                          <ProviderLogo iconId={sub?.provider_icon ?? providerIconId(request.provider_id)} size={20} iconSize={13} />
                          <span>{providerName}</span>
                        </span>
                      } />
                      <Row k={t("requestLogs.detail.realModel")} v={monoOr(request.real_model_name)} />
                      <Row k={t("requestLogs.detail.responseModel")} v={monoOr(request.response_model_name)} />
                      <Row k={t("requestLogs.detail.effort")} v={effortSummary(request, t) ?? dash} />
                      <Row k={t("requestLogs.col.speed")} hint={t("requestLogs.speedHint")} v={fmtTokenSpeed(request.output_tokens_per_second)} />
                      <Row k={t("requestLogs.col.firstContent")} hint={t("requestLogs.firstContentHint")} v={fmtRequestSeconds(request.first_content_ms)} />
                      <Row k={t("requestLogs.detail.latency")} v={fmtRequestSeconds(request.total_latency_ms)} />
                      <Row k={t("stats.daily.tokenTooltipInput")} v={fmtNum(request.input_tokens)} />
                      <Row k={t("stats.daily.tokenTooltipOutput")} v={fmtNum(request.output_tokens)} />
                      <Row k={t("stats.daily.tokenTooltipCacheRead")} v={fmtNum(request.cache_read_tokens)} />
                      <Row k={t("stats.daily.tokenTooltipCacheCreate")} v={fmtNum(request.cache_creation_tokens)} />
                      {isError && request.error_message && (
                        <Row k={t("requestLogs.detail.errorMessage")} v={<span className="rd-overview-error">{request.error_message}</span>} />
                      )}
                    </tbody>
                  </table>
                </TabsPrimitive.Content>

                <TabsPrimitive.Content value="request" className="rd-body">
                  <table className="rd-table">
                    <thead>
                      <tr>
                        <th>{t("requestLogs.detail.headers.item")}</th>
                        <th>{t("requestLogs.detail.headers.value")}</th>
                      </tr>
                    </thead>
                    <tbody>
                      <Row k={t("requestLogs.detail.effort")} v={effortSummary(request, t) ?? dash} />
                      <Row k={t("requestLogs.detail.stopReason")} v={monoOr(request.stop_reason)} />
                      <Row k={t("requestLogs.detail.entryKind")} v={monoOr(request.entry_kind ? `/v1/${request.entry_kind}` : null)} />
                      <Row k={t("requestLogs.detail.httpVersion")} v={monoOr(request.downstream_http_version)} />
                      <Row k={t("requestLogs.detail.clientVersion")} v={monoOr(request.client_version)} />
                      <Row k={t("requestLogs.detail.clientIp")} v={monoOr(request.client_ip)} />
                      <Row k={t("requestLogs.detail.userAgent")} v={monoOr(request.client_user_agent)} />
                    </tbody>
                  </table>
                </TabsPrimitive.Content>

                {toolsVisible && (() => {
                  const { chips, truncated } = parseToolNames(request.tool_use_names);
                  const num = (v?: number | null) => (v != null ? <span className="mono tnum">{v}</span> : dash);
                  return (
                    <TabsPrimitive.Content value="tools" className="rd-body">
                      <div className="rd-tool-counts">
                        <span>{t("requestLogs.detail.tools.offered")} {num(request.tools_offered_count)}</span>
                        <span>{t("requestLogs.detail.tools.results")} {num(request.tool_result_count)}</span>
                        <span>{t("requestLogs.detail.tools.used")} {num(request.tool_use_count)}</span>
                      </div>
                      {chips.length > 0 ? (
                        <div style={{ display: "flex", flexWrap: "wrap", gap: 6 }}>
                          {chips.map(([name, n]) => (
                            <span key={name} className="pill tag mono" title={name}>
                              {name}
                              {n > 1 && <span className="muted"> ×{n}</span>}
                            </span>
                          ))}
                          {truncated && <span className="field-hint">{t("requestLogs.detail.tools.truncated")}</span>}
                        </div>
                      ) : (
                        request.tool_use_count === 0 && (
                          <div className="field-hint">{t("requestLogs.detail.tools.none")}</div>
                        )
                      )}
                    </TabsPrimitive.Content>
                  );
                })()}

                <TabsPrimitive.Content value="clientHeaders" className="rd-body">
                  <HeaderTable
                    title={t("requestLogs.detail.nav.clientHeaders")}
                    subtitle={t("requestLogs.detail.headers.clientSub")}
                    pairs={headers?.client_request}
                    loading={headersQuery.isLoading}
                    emptyText={emptyText("client")}
                  />
                </TabsPrimitive.Content>
                <TabsPrimitive.Content value="upstreamHeaders" className="rd-body">
                  <HeaderTable
                    title={t("requestLogs.detail.nav.upstreamHeaders")}
                    subtitle={t("requestLogs.detail.headers.upstreamSub")}
                    pairs={headers?.upstream_request}
                    loading={headersQuery.isLoading}
                    emptyText={emptyText("request")}
                    tagFor={outboundTag}
                  />
                  {headers?.upstream_request && headers.upstream_request.length > 0 && (
                    <p className="rd-note">{t("requestLogs.detail.headers.sourceHint")}</p>
                  )}
                </TabsPrimitive.Content>
                <TabsPrimitive.Content value="responseHeaders" className="rd-body">
                  <HeaderTable
                    title={t("requestLogs.detail.nav.responseHeaders")}
                    subtitle={t("requestLogs.detail.headers.responseSub")}
                    pairs={headers?.upstream_response}
                    loading={headersQuery.isLoading}
                    emptyText={emptyText("response")}
                    tagFor={responseTag}
                  />
                </TabsPrimitive.Content>

                <TabsPrimitive.Content value="body" className="rd-body">
                  {isError && request.error_message && (
                    <div className="alert err rd-error">
                      <div className="mono" style={{ fontSize: 12.5, wordBreak: "break-word" }}>
                        {request.error_message}
                      </div>
                    </div>
                  )}
                  {prettyBody ? (
                    <pre className="mono rd-pre">{prettyBody}</pre>
                  ) : (
                    <div className="rd-empty">{t("requestLogs.detail.noBody")}</div>
                  )}
                </TabsPrimitive.Content>
              </div>
            </TabsPrimitive.Root>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}

function Row({ k, v, hint }: { k: string; v: React.ReactNode; hint?: string }) {
  return (
    <tr>
      <td className="rd-table-k" title={hint}>{k}</td>
      <td className="rd-table-v">{v}</td>
    </tr>
  );
}

function toneOf(status: string): "ok" | "warn" | "err" {
  if (status === "success") return "ok";
  if (status === "timeout") return "warn";
  return "err";
}
