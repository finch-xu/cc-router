import { useEffect, useRef, useState } from "react";
import { RefreshCw, Wallet } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Spinner } from "@/components/Spinner";
import { api } from "@/api/tauri";
import { useT, type TFunction } from "@/i18n";
import { cn } from "@/lib/utils";
import { fmtRelativeTime } from "@/lib/format";
import type {
  BalanceEntry,
  BalanceSeverity,
  SubscriptionDto,
} from "@/types";

interface Props {
  subscription: SubscriptionDto;
  /** Hook into refresh success — page-level invalidate keeps list badges in sync. */
  onChanged?: () => void;
}

interface SeverityMeta {
  /** text color for the main number */
  text: string;
  /** background pill for the list-page badge */
  badge: string;
  /** i18n key for the inline label under the number; missing for `normal` */
  labelKey?: string;
}

const SEVERITY_META: Record<BalanceSeverity, SeverityMeta> = {
  normal: {
    text: "",
    badge: "bg-muted text-muted-foreground",
  },
  low: {
    text: "text-yellow-600",
    badge: "bg-yellow-500/10 text-yellow-700 dark:text-yellow-500",
    labelKey: "subscriptionBalance.severityLow",
  },
  critical: {
    text: "text-destructive",
    badge: "bg-destructive/10 text-destructive",
    labelKey: "subscriptionBalance.severityCritical",
  },
};

export function SubscriptionBalanceCard({ subscription, onChanged }: Props) {
  const { t } = useT();
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const autoFetched = useRef(false);

  const cache = subscription.balance_cache;

  // Auto-fetch once per subscription when no cache exists yet. Stale-while-revalidate
  // is left to the user's explicit "refresh" button — avoids hammering provider quotas.
  useEffect(() => {
    autoFetched.current = false;
  }, [subscription.id]);

  useEffect(() => {
    if (
      subscription.balance_supported &&
      !cache &&
      !loading &&
      !autoFetched.current
    ) {
      autoFetched.current = true;
      void doRefresh();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [subscription.id, subscription.balance_supported, !!cache]);

  if (!subscription.balance_supported) return null;

  async function doRefresh() {
    setLoading(true);
    setError(null);
    try {
      const result = await api.refreshSubscriptionBalance(subscription.id);
      if (result.kind === "success") {
        onChanged?.();
      } else if (result.kind === "failed") {
        setError(result.reason);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  const snapshot = cache?.snapshot;
  const accountUnavailable = snapshot?.is_available === false;

  return (
    <Card>
      <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-3">
        <CardTitle className="flex items-center gap-2 text-base">
          <Wallet className="h-4 w-4" />
          {t("subscriptionBalance.title")}
        </CardTitle>
        <Button variant="outline" size="sm" onClick={doRefresh} disabled={loading}>
          {loading ? <Spinner size={14} /> : <RefreshCw className="h-3.5 w-3.5" />}
          {loading
            ? t("subscriptionBalance.refreshing")
            : t("subscriptionBalance.refresh")}
        </Button>
      </CardHeader>
      <CardContent className="space-y-2">
        {!snapshot && !error && !loading && (
          <div className="text-xs text-muted-foreground">
            {t("subscriptionBalance.neverFetched")}
          </div>
        )}

        {accountUnavailable && (
          <div className="text-xs text-destructive">
            {t("subscriptionBalance.accountUnavailable")}
          </div>
        )}

        {snapshot?.entries.map((entry, idx) => (
          <BalanceEntryRow
            key={`${entry.unit}-${idx}`}
            entry={entry}
            t={t}
            accountUnavailable={accountUnavailable}
          />
        ))}

        {error && (
          <div className="text-xs text-destructive">
            {t("subscriptionBalance.errPrefix")}
            {error}
          </div>
        )}

        {cache && (
          <div className="text-[11px] text-muted-foreground">
            {t("subscriptionBalance.lastUpdatedPrefix")}
            {fmtRelativeTime(cache.fetched_at, t)}
          </div>
        )}
      </CardContent>
    </Card>
  );
}

function BalanceEntryRow({
  entry,
  t,
  accountUnavailable,
}: {
  entry: BalanceEntry;
  t: TFunction;
  accountUnavailable: boolean;
}) {
  const effective = effectiveSeverity(entry.severity, accountUnavailable);
  const meta = SEVERITY_META[effective];
  const prefix = currencyPrefix(entry.unit);

  return (
    <div className="flex items-baseline justify-between gap-3">
      <span className="text-sm text-muted-foreground">{entry.label}</span>
      <div className="flex flex-col items-end">
        <span className={cn("font-mono text-lg font-semibold tabular-nums", meta.text)}>
          {prefix}
          {entry.value_text}
          {!prefix && (
            <span className="ml-1 text-xs text-muted-foreground">{entry.unit}</span>
          )}
        </span>
        {entry.hint && (
          <span className="text-[11px] text-muted-foreground">{entry.hint}</span>
        )}
        {meta.labelKey && (
          <span className={cn("text-[11px]", meta.text)}>{t(meta.labelKey)}</span>
        )}
      </div>
    </div>
  );
}

/**
 * Compact badge for the subscriptions list. Renders the first entry only;
 * returns null when no cache is available (the list page never auto-fetches).
 */
export function BalanceBadge({ subscription }: { subscription: SubscriptionDto }) {
  if (!subscription.balance_supported || !subscription.balance_cache) return null;

  const snapshot = subscription.balance_cache.snapshot;
  const entry = snapshot.entries[0];
  if (!entry) return null;

  const effective = effectiveSeverity(entry.severity, snapshot.is_available === false);
  const meta = SEVERITY_META[effective];

  return (
    <span
      className={cn(
        "inline-flex items-center rounded px-1.5 py-0.5 font-mono text-[10.5px] tabular-nums",
        meta.badge,
      )}
      title={entry.hint}
    >
      {currencyPrefix(entry.unit) || `${entry.unit} `}
      {entry.value_text}
    </span>
  );
}

function effectiveSeverity(
  raw: BalanceSeverity,
  accountUnavailable: boolean,
): BalanceSeverity {
  return accountUnavailable ? "critical" : raw;
}

function currencyPrefix(unit: string): string {
  switch (unit) {
    case "CNY":
    case "JPY":
      return "¥";
    case "USD":
      return "$";
    case "EUR":
      return "€";
    default:
      return "";
  }
}
