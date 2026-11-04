import { useEffect, useMemo, useState } from "react";
import * as TabsPrimitive from "@radix-ui/react-tabs";
import { useQuery } from "@tanstack/react-query";
import { Copy, Check, ArrowRight } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ClientToolBadge } from "@/components/ClientToolBadge";
import { HeaderTable, type HeaderTag } from "@/components/request-detail/HeaderTable";
import { api } from "@/api/tauri";
import { isReturnedToClient, outboundSource } from "@/lib/headerSources";
import { ProviderLogo } from "@/components/ProviderLogo";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useT } from "@/i18n";
import { fmtNum, fmtTime } from "@/lib/format";
import { customProviderLabel, providerIconId } from "@/lib/providerLabels";
import { providerName as localProviderName } from "@/lib/providerText";
import type { HeaderPairs, RequestLogDto } from "@/types";
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
      `http_status: ${request.http_status ?? "—"}`,
      `stop_reason: ${request.stop_reason ?? "—"}`,
      `tools_offered: ${request.tools_offered_count ?? "—"} · tool_results: ${request.tool_result_count ?? "—"} · tool_uses: ${request.tool_use_count ?? "—"}`,
      `tool_use_names: ${request.tool_use_names ?? "—"}`,
      `error_message: ${request.error_message ?? ""}`,
      "",
      "upstream_response_body:",
      prettyBody ?? "",
    ];
    const section = (name: string, pairs: HeaderPairs | undefined) => {
      if (!pairs) return;
      lines.push("", `${name}:`, ...pairs.map(([k, v]) => `${k}: ${v}`));
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

  // 发往上游 / 上游响应两张表的标签 (规则见 lib/headerSources.ts)
  const clientNames = new Set((headers?.client_request ?? []).map(([k]) => k.toLowerCase()));
  const outboundTag = (name: string): HeaderTag => {
    const src = outboundSource(name, clientNames, sub?.auth_header_name);
    const tone = src === "forwarded" ? "ok" : src === "ccRouter" ? "muted" : "accent";
    return { label: t(`requestLogs.detail.headers.source.${src}`), tone };
  };
  // 回传白名单只作用于 Anthropic 透传路径; 翻译类订阅不显示该标签
  const showReturned = sub?.auth_type === "api_key";
  const responseTag = (name: string): HeaderTag | null =>
    showReturned
      ? isReturnedToClient(name)
        ? { label: t("requestLogs.detail.headers.returned"), tone: "ok" }
        : { label: t("requestLogs.detail.headers.notReturned"), tone: "muted" }
      : null;
  const count = (pairs: HeaderPairs | undefined) =>
    headersQuery.isLoading ? "…" : pairs ? String(pairs.length) : "—";
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
                <div className="rd-nav-group">{t("requestLogs.detail.nav.headersGroup")}</div>
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
                <div className="rd-nav-group">{t("requestLogs.detail.nav.responseGroup")}</div>
                <TabsPrimitive.Trigger value="body" className="rd-nav-item">
                  {t("requestLogs.detail.nav.body")}
                </TabsPrimitive.Trigger>
              </TabsPrimitive.List>

              <div className="rd-pane">
                <TabsPrimitive.Content value="overview" className="rd-body">
                  {/* 路由路径: 客户端 → 虚拟模型 → 订阅 → 真实模型 */}
                  <section className="rd-route" aria-label={t("requestLogs.detail.section.route")}>
                    <div className="rd-node">
                      <div className="rd-node-label">{t("requestLogs.detail.clientTool")}</div>
                      <div className="rd-node-value">
                        <ClientToolBadge toolId={request.client_tool} userAgent={request.client_user_agent} />
                      </div>
                      <div className="rd-node-sub mono">{request.client_version ?? "—"}</div>
                    </div>
                    <ArrowRight className="rd-arrow" size={16} aria-hidden />
                    <div className="rd-node">
                      <div className="rd-node-label">{t("requestLogs.detail.virtualModel")}</div>
                      <div className="rd-node-value mono">{request.virtual_model_name}</div>
                      <div className="rd-node-sub mono">
                        {request.entry_kind ? `/v1/${request.entry_kind}` : "—"}
                      </div>
                    </div>
                    <ArrowRight className="rd-arrow" size={16} aria-hidden />
                    <div className="rd-node">
                      <div className="rd-node-label">{t("requestLogs.detail.subscription")}</div>
                      <div className="rd-node-value rd-node-sub-row">
                        <ProviderLogo
                          iconId={sub?.provider_icon ?? providerIconId(request.provider_id)}
                          size={20}
                          iconSize={13}
                        />
                        <span className="rd-ellipsis">{sub?.display_name ?? providerName}</span>
                      </div>
                      <div className="rd-node-sub">{providerName}</div>
                    </div>
                    <ArrowRight className="rd-arrow" size={16} aria-hidden />
                    <div className="rd-node accent">
                      <div className="rd-node-label">{t("requestLogs.detail.realModel")}</div>
                      <div className="rd-node-value mono strong rd-ellipsis" title={request.real_model_name}>
                        {request.real_model_name}
                      </div>
                      <div className="rd-node-sub mono rd-ellipsis" title={request.response_model_name}>
                        {request.response_model_name && request.response_model_name !== request.real_model_name
                          ? `${t("requestLogs.detail.responseModel")} ${request.response_model_name}`
                          : effortSummaryShort(request, t) ?? "—"}
                      </div>
                    </div>
                  </section>

                  <section className="rd-stats" aria-label={t("requestLogs.detail.section.usage")}>
                    <Stat label={t("requestLogs.detail.latency")}
                      value={request.total_latency_ms != null ? `${(request.total_latency_ms / 1000).toFixed(2)}s` : "—"} />
                    <Stat label={t("stats.daily.tokenTooltipInput")} value={fmtNum(request.input_tokens)} />
                    <Stat label={t("stats.daily.tokenTooltipOutput")} value={fmtNum(request.output_tokens)} strong />
                    <Stat label={t("stats.daily.tokenTooltipCacheRead")} value={fmtNum(request.cache_read_tokens)} />
                    <Stat label={t("stats.daily.tokenTooltipCacheCreate")} value={fmtNum(request.cache_creation_tokens)} />
                  </section>

                  {isError && request.error_message && (
                    <div className="alert err rd-error">
                      <div>
                        <div className="rd-card-title" style={{ marginBottom: 4 }}>
                          {t("requestLogs.detail.errorMessage")}
                        </div>
                        <div className="mono" style={{ fontSize: 12.5, wordBreak: "break-word" }}>
                          {request.error_message}
                        </div>
                      </div>
                    </div>
                  )}
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
                  />
                </TabsPrimitive.Content>
                <TabsPrimitive.Content value="upstreamHeaders" className="rd-body">
                  <HeaderTable
                    title={t("requestLogs.detail.nav.upstreamHeaders")}
                    subtitle={t("requestLogs.detail.headers.upstreamSub")}
                    pairs={headers?.upstream_request}
                    loading={headersQuery.isLoading}
                    tagFor={outboundTag}
                  />
                  <p className="field-hint">{t("requestLogs.detail.headers.sourceHint")}</p>
                </TabsPrimitive.Content>
                <TabsPrimitive.Content value="responseHeaders" className="rd-body">
                  <HeaderTable
                    title={t("requestLogs.detail.nav.responseHeaders")}
                    subtitle={t("requestLogs.detail.headers.responseSub")}
                    pairs={headers?.upstream_response}
                    loading={headersQuery.isLoading}
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
                    <div className="field-hint">{t("requestLogs.detail.noBody")}</div>
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

/** 路由卡「真实模型」下方的一行小字: 实际发往上游的思考强度 (没有则不显示) */
function effortSummaryShort(r: RequestLogDto, t: (key: string) => string): string | null {
  return r.effective_effort ? `${t("requestLogs.detail.effort")} ${r.effective_effort}` : null;
}

function Stat({ label, value, strong }: { label: string; value: string; strong?: boolean }) {
  return (
    <div className={strong ? "rd-stat strong" : "rd-stat"}>
      <div className="rd-stat-label">{label}</div>
      <div className="rd-stat-value mono tnum">{value}</div>
    </div>
  );
}

function Row({ k, v }: { k: string; v: React.ReactNode }) {
  return (
    <tr>
      <td className="rd-table-k">{k}</td>
      <td className="rd-table-v">{v}</td>
    </tr>
  );
}

function toneOf(status: string): "ok" | "warn" | "err" {
  if (status === "success") return "ok";
  if (status === "timeout") return "warn";
  return "err";
}
