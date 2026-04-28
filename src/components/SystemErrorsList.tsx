import { useMemo, useState } from "react";
import { AlertTriangle, RefreshCw } from "lucide-react";
import { EmptyState } from "@/components/EmptyState";
import { Pagination } from "@/components/Pagination";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useEvents } from "@/hooks/useEvents";
import { useT } from "@/i18n";
import { fmtTime } from "@/lib/format";
import type { EventDto, EventSeverity } from "@/types";

const SEVERITY_TONE: Record<EventSeverity, "ok" | "warn" | "err"> = {
  info: "ok",
  warn: "warn",
  error: "err",
};

const PAGE_SIZE = 20;

export function SystemErrorsList() {
  const { t } = useT();
  const [page, setPage] = useState(1);
  const query = useEvents(page, PAGE_SIZE, { kind: "system_error" });
  const [active, setActive] = useState<EventDto | null>(null);

  const total = query.data?.total ?? 0;
  const totalPages = Math.max(1, Math.ceil(total / PAGE_SIZE));
  const items = query.data?.items ?? [];

  return (
    <>
      <div className="page-actions">
        <div className="page-header" style={{ margin: 0 }}>
          <h1>{t("logs.tab.systemErrors")}</h1>
          <div className="subtitle">{t("logs.systemErrors.subtitle")}</div>
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

      {query.isLoading && <div className="field-hint">{t("common.loading")}</div>}

      {query.data && total === 0 && (
        <EmptyState icon={AlertTriangle} message={t("logs.systemErrors.empty")} />
      )}

      {items.length > 0 && (
        <div className="card">
          <div style={{ display: "flex", flexDirection: "column" }}>
            {items.map((ev) => (
              <SystemErrorRow key={ev.id} ev={ev} onOpen={() => setActive(ev)} />
            ))}
          </div>
        </div>
      )}

      <Pagination
        total={total}
        page={page}
        totalPages={totalPages}
        onChange={setPage}
      />

      <SystemErrorDialog ev={active} onClose={() => setActive(null)} />
    </>
  );
}

function SystemErrorRow({ ev, onOpen }: { ev: EventDto; onOpen: () => void }) {
  return (
    <div
      onClick={onOpen}
      style={{
        display: "flex",
        alignItems: "center",
        gap: 12,
        padding: "10px 16px",
        borderBottom: "1px solid var(--line)",
        fontSize: 13,
        cursor: "pointer",
      }}
    >
      <span className="mono muted" style={{ width: 150, fontSize: 12 }}>
        {fmtTime(ev.timestamp)}
      </span>
      <span className={`pill ${SEVERITY_TONE[ev.severity]}`}>
        <span className="dot" />
        {ev.severity}
      </span>
      <span style={{ flex: 1 }}>{ev.summary}</span>
    </div>
  );
}

function SystemErrorDialog({ ev, onClose }: { ev: EventDto | null; onClose: () => void }) {
  const { t } = useT();
  const open = ev !== null;

  const prettyPayload = useMemo(() => {
    if (ev?.payload == null) return null;
    return JSON.stringify(ev.payload, null, 2);
  }, [ev]);

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent
        className="cc-dialog"
        style={{ maxWidth: 720, width: "92vw", maxHeight: "85vh", overflow: "auto" }}
      >
        <DialogHeader>
          <DialogTitle>{t("logs.systemErrors.detailTitle")}</DialogTitle>
        </DialogHeader>
        {ev && (
          <div style={{ display: "flex", flexDirection: "column", gap: 12, fontSize: 13 }}>
            <div>
              <div style={{ color: "var(--ink-3)", fontSize: 12.5, marginBottom: 4 }}>
                {t("logs.systemErrors.summary")}
              </div>
              <div className="mono" style={{ color: "var(--err)" }}>{ev.summary}</div>
            </div>
            <div className="mono muted" style={{ fontSize: 12 }}>
              {fmtTime(ev.timestamp)} · {ev.severity}
            </div>
            {prettyPayload && (
              <div>
                <div style={{ color: "var(--ink-3)", fontSize: 12.5, marginBottom: 4 }}>
                  {t("logs.systemErrors.payload")}
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
                  {prettyPayload}
                </pre>
              </div>
            )}
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
