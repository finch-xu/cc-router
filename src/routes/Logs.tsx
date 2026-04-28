import { useState } from "react";
import { useT } from "@/i18n";
import { RequestLogsPage } from "@/routes/RequestLogs";
import { SubscriptionEventsList } from "@/components/SubscriptionEventsList";
import { SystemErrorsList } from "@/components/SystemErrorsList";

type Tab = "requests" | "subscriptionEvents" | "systemErrors";

export function LogsPage() {
  const { t } = useT();
  const [tab, setTab] = useState<Tab>("requests");

  return (
    <>
      <div
        style={{
          display: "flex",
          gap: 4,
          borderBottom: "1px solid var(--line)",
          marginBottom: 16,
        }}
      >
        <TabButton active={tab === "requests"} onClick={() => setTab("requests")}>
          {t("logs.tab.requests")}
        </TabButton>
        <TabButton
          active={tab === "subscriptionEvents"}
          onClick={() => setTab("subscriptionEvents")}
        >
          {t("logs.tab.subscriptionEvents")}
        </TabButton>
        <TabButton
          active={tab === "systemErrors"}
          onClick={() => setTab("systemErrors")}
        >
          {t("logs.tab.systemErrors")}
        </TabButton>
      </div>

      {tab === "requests" && <RequestLogsPage />}
      {tab === "subscriptionEvents" && <SubscriptionEventsList />}
      {tab === "systemErrors" && <SystemErrorsList />}
    </>
  );
}

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      style={{
        padding: "8px 16px",
        background: "transparent",
        border: "none",
        borderBottom: active ? "2px solid var(--ink)" : "2px solid transparent",
        marginBottom: -1,
        color: active ? "var(--ink)" : "var(--ink-3)",
        fontSize: 13,
        fontWeight: active ? 600 : 400,
        cursor: "pointer",
      }}
    >
      {children}
    </button>
  );
}
