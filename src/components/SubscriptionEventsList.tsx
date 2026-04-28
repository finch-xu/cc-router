import { useMemo, useState } from "react";
import { ArrowRight, RefreshCw, ScrollText } from "lucide-react";
import { EmptyState } from "@/components/EmptyState";
import { Pagination } from "@/components/Pagination";
import { StatusBadge } from "@/components/StatusBadge";
import { useEvents } from "@/hooks/useEvents";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useT } from "@/i18n";
import { fmtTime } from "@/lib/format";
import type { EventDto, StateChangePayload } from "@/types";

const PAGE_SIZE = 20;

export function SubscriptionEventsList() {
  const { t } = useT();
  const [page, setPage] = useState(1);
  const query = useEvents(page, PAGE_SIZE, { kind: "subscription_state_change" });
  const subs = useSubscriptions();
  const subMap = useMemo(() => {
    const m = new Map<string, string>();
    subs.data?.forEach((s) => m.set(s.id, s.display_name));
    return m;
  }, [subs.data]);

  const total = query.data?.total ?? 0;
  const totalPages = Math.max(1, Math.ceil(total / PAGE_SIZE));
  const items = query.data?.items ?? [];

  return (
    <>
      <div className="page-actions">
        <div className="page-header" style={{ margin: 0 }}>
          <h1>{t("logs.tab.subscriptionEvents")}</h1>
          <div className="subtitle">{t("logs.subscriptionEvents.subtitle")}</div>
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
        <EmptyState icon={ScrollText} message={t("logs.subscriptionEvents.empty")} />
      )}

      {items.length > 0 && (
        <div className="card">
          <div style={{ display: "flex", flexDirection: "column" }}>
            {items.map((ev) => (
              <SubscriptionEventRow
                key={ev.id}
                ev={ev}
                subName={ev.subscription_id ? subMap.get(ev.subscription_id) : undefined}
              />
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
    </>
  );
}

function SubscriptionEventRow({ ev, subName }: { ev: EventDto; subName?: string }) {
  const payload = (ev.payload as StateChangePayload | null | undefined) ?? null;
  const fromState = payload?.from;
  const toState = payload?.to;

  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 12,
        padding: "10px 16px",
        borderBottom: "1px solid var(--line)",
        fontSize: 13,
      }}
    >
      <span className="mono muted" style={{ width: 150, fontSize: 12 }}>
        {fmtTime(ev.timestamp)}
      </span>
      <span className="strong" style={{ minWidth: 140 }}>
        {subName ?? ev.subscription_id ?? "—"}
      </span>
      <div style={{ display: "flex", alignItems: "center", gap: 6, flex: 1 }}>
        {fromState && <StatusBadge state={fromState} />}
        <ArrowRight size={12} style={{ color: "var(--ink-3)" }} />
        {toState && <StatusBadge state={toState} />}
      </div>
      {payload?.last_error && (
        <span
          className="mono"
          style={{ fontSize: 12, color: "var(--err)", maxWidth: 280, textAlign: "right" }}
          title={payload.last_error}
        >
          {payload.last_error}
        </span>
      )}
    </div>
  );
}
