import { useEffect, useMemo, useState } from "react";
import { Copy, Check } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ClientToolBadge } from "@/components/ClientToolBadge";
import { useT } from "@/i18n";
import { fmtTime } from "@/lib/format";
import type { RequestLogDto } from "@/types";

interface Props {
  request: RequestLogDto | null;
  onClose: () => void;
}

export function RequestDetailDialog({ request, onClose }: Props) {
  const { t } = useT();
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!request) setCopied(false);
  }, [request]);

  const open = request !== null;
  const isError = request && request.status !== "success";

  const prettyBody = useMemo(() => {
    if (!request?.upstream_response_body) return null;
    try {
      const parsed = JSON.parse(request.upstream_response_body);
      return JSON.stringify(parsed, null, 2);
    } catch {
      return request.upstream_response_body;
    }
  }, [request]);

  async function copyAll() {
    if (!request) return;
    const lines = [
      `request_id: ${request.id}`,
      `time: ${fmtTime(request.timestamp)}`,
      `virtual_model: ${request.virtual_model_name}`,
      `provider: ${request.provider_id}`,
      `real_model: ${request.real_model_name}`,
      `status: ${request.status}`,
      `http_status: ${request.http_status ?? "—"}`,
      `error_message: ${request.error_message ?? ""}`,
      "",
      "upstream_response_body:",
      prettyBody ?? "",
    ];
    try {
      await navigator.clipboard.writeText(lines.join("\n"));
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // ignore
    }
  }

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent
        className="cc-dialog"
        style={{ maxWidth: 720, width: "92vw", maxHeight: "85vh", overflow: "auto" }}
      >
        <DialogHeader>
          <DialogTitle>{t("requestLogs.detail.title")}</DialogTitle>
        </DialogHeader>
        {request && (
          <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
            <div
              style={{
                display: "grid",
                gridTemplateColumns: "auto 1fr",
                gap: "6px 12px",
                fontSize: 12.5,
              }}
            >
              <KV k={t("requestLogs.detail.time")} v={fmtTime(request.timestamp)} mono />
              <KV k={t("requestLogs.detail.requestId")} v={request.id} mono />
              <KV
                k={t("requestLogs.detail.status")}
                v={
                  <span className={`pill ${toneOf(request.status)}`}>
                    <span className="dot" />
                    {t(`requestLogs.status.${request.status}`)}
                  </span>
                }
              />
              <KV
                k={t("requestLogs.detail.httpStatus")}
                v={
                  request.http_status != null ? (
                    <span className="mono">{request.http_status}</span>
                  ) : (
                    <span className="muted">—</span>
                  )
                }
              />
              <KV
                k={t("requestLogs.detail.virtualModel")}
                v={<span className="mono">{request.virtual_model_name}</span>}
              />
              <KV
                k={t("requestLogs.detail.realModel")}
                v={<span className="mono strong">{request.real_model_name}</span>}
              />
              <KV
                k={t("requestLogs.detail.provider")}
                v={<span className="mono">{request.provider_id}</span>}
              />
              <KV
                k={t("requestLogs.detail.latency")}
                v={
                  request.total_latency_ms != null
                    ? `${(request.total_latency_ms / 1000).toFixed(2)}s`
                    : "-"
                }
              />
              <KV
                k={t("requestLogs.detail.clientTool")}
                v={<ClientToolBadge toolId={request.client_tool} />}
              />
              <KV
                k={t("requestLogs.detail.clientVersion")}
                v={
                  request.client_version ? (
                    <span className="mono">{request.client_version}</span>
                  ) : (
                    <span className="muted">—</span>
                  )
                }
              />
              <KV
                k={t("requestLogs.detail.clientIp")}
                v={
                  request.client_ip ? (
                    <span className="mono">{request.client_ip}</span>
                  ) : (
                    <span className="muted">—</span>
                  )
                }
              />
              <KV
                k={t("requestLogs.detail.userAgent")}
                v={
                  request.client_user_agent ? (
                    <span
                      className="mono"
                      style={{ fontSize: 11.5, wordBreak: "break-all" }}
                    >
                      {request.client_user_agent}
                    </span>
                  ) : (
                    <span className="muted">—</span>
                  )
                }
              />
            </div>

            {isError && request.error_message && (
              <div>
                <div style={{ color: "var(--ink-3)", fontSize: 12.5, marginBottom: 4 }}>
                  {t("requestLogs.detail.errorMessage")}
                </div>
                <div className="mono" style={{ color: "var(--err)", fontSize: 13 }}>
                  {request.error_message}
                </div>
              </div>
            )}

            {prettyBody && (
              <div>
                <div
                  style={{
                    display: "flex",
                    alignItems: "center",
                    justifyContent: "space-between",
                  }}
                >
                  <div style={{ color: "var(--ink-3)", fontSize: 12.5 }}>
                    {t("requestLogs.detail.upstreamBody")}
                  </div>
                  <button
                    className="btn sm"
                    type="button"
                    onClick={copyAll}
                    style={{ display: "inline-flex", alignItems: "center", gap: 4 }}
                  >
                    {copied ? <Check size={12} /> : <Copy size={12} />}
                    {copied ? t("common.copied") : t("requestLogs.detail.copy")}
                  </button>
                </div>
                <pre
                  className="mono"
                  style={{
                    background: "var(--surface-2)",
                    border: "1px solid var(--line)",
                    borderRadius: 6,
                    padding: 12,
                    margin: 0,
                    fontSize: 12,
                    maxHeight: 320,
                    overflow: "auto",
                    whiteSpace: "pre-wrap",
                    wordBreak: "break-word",
                  }}
                >
                  {prettyBody}
                </pre>
              </div>
            )}

            {!isError && !prettyBody && (
              <div className="field-hint">{t("requestLogs.detail.noBody")}</div>
            )}
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}

function KV({
  k,
  v,
  mono,
}: {
  k: string;
  v: React.ReactNode;
  mono?: boolean;
}) {
  return (
    <>
      <div style={{ color: "var(--ink-3)", whiteSpace: "nowrap" }}>{k}</div>
      <div className={mono ? "mono" : undefined} style={{ wordBreak: "break-word" }}>
        {v}
      </div>
    </>
  );
}

function toneOf(status: string): "ok" | "warn" | "err" {
  if (status === "success") return "ok";
  if (status === "timeout") return "warn";
  return "err";
}
