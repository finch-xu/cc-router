import { useState } from "react";
import { Gauge } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useResetTotalQuotaUsage } from "@/hooks/useSubscriptions";
import { useT } from "@/i18n";
import { cn } from "@/lib/utils";
import { fmtCompact, fmtTime, fmtTimeShort } from "@/lib/format";
import { formatTokenShorthand } from "@/lib/quota";
import type { QuotaUsageDto, SubscriptionDto } from "@/types";

interface Props {
  subscription: SubscriptionDto;
  onChanged?: () => void;
}

const SEGMENTS: Array<{
  key: keyof Pick<QuotaUsageDto, "input" | "output" | "cache_creation" | "cache_read">;
  labelKey: string;
  className: string;
}> = [
  { key: "input", labelKey: "quota.legend.input", className: "bg-sky-500" },
  { key: "output", labelKey: "quota.legend.output", className: "bg-emerald-500" },
  { key: "cache_creation", labelKey: "quota.legend.cacheCreation", className: "bg-amber-500" },
  { key: "cache_read", labelKey: "quota.legend.cacheRead", className: "bg-violet-500" },
];

export function SubscriptionQuotaCard({ subscription, onChanged }: Props) {
  const { t } = useT();
  const resetMut = useResetTotalQuotaUsage();
  const [resetError, setResetError] = useState<string | null>(null);
  const rows = subscription.quota_usage.filter((q) => q.limit != null);

  async function resetTotal() {
    if (!window.confirm(t("quota.resetTotalConfirm"))) return;
    setResetError(null);
    try {
      await resetMut.mutateAsync(subscription.id);
      onChanged?.();
    } catch (e) {
      setResetError(String(e));
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <Gauge className="h-4 w-4" /> {t("quota.title")}
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        {rows.length === 0 && (
          <p className="text-sm text-muted-foreground">{t("quota.noLimits")}</p>
        )}
        {rows.map((q) => {
          const limit = q.limit!;
          const used = q.input + q.output + q.cache_creation + q.cache_read;
          const ratio = Math.min(used / limit, 1);
          const warn = ratio >= 0.8;
          return (
            <div key={q.period} className="space-y-1">
              <div className="flex items-center justify-between text-sm">
                <span className="font-medium">{t(`quota.period.${q.period}`)}</span>
                <span
                  className={cn(
                    "text-muted-foreground",
                    q.exceeded && "text-red-600 font-medium",
                  )}
                >
                  {q.exceeded
                    ? t("quota.exceeded")
                    : t("quota.usedOf", { used: fmtCompact(used), limit: formatTokenShorthand(limit) })}
                </span>
              </div>
              <div
                className={cn(
                  "flex h-3 w-full overflow-hidden rounded-full bg-muted",
                  warn && !q.exceeded && "ring-1 ring-amber-500",
                  q.exceeded && "ring-1 ring-red-500",
                )}
                title={SEGMENTS.map((s) => `${t(s.labelKey)} ${fmtCompact(q[s.key])}`).join(" · ")}
              >
                {SEGMENTS.map((s) => (
                  <div
                    key={s.key}
                    className={cn("h-full shrink-0", s.className)}
                    style={{ width: `${(Math.min(q[s.key], limit) / limit) * 100}%` }}
                  />
                ))}
              </div>
              <div className="flex items-center justify-between text-xs text-muted-foreground">
                <span className="flex gap-3">
                  {SEGMENTS.map((s) => (
                    <span key={s.key} className="flex items-center gap-1">
                      <i className={cn("inline-block h-2 w-2 rounded-sm", s.className)} />
                      {t(s.labelKey)} {fmtCompact(q[s.key])}
                    </span>
                  ))}
                </span>
                {q.period_end_ms != null ? (
                  <span>
                    {t("quota.resetAt", {
                      time: q.period === "daily" ? fmtTimeShort(q.period_end_ms) : fmtTime(q.period_end_ms),
                    })}
                  </span>
                ) : (
                  <div className="flex flex-col items-end gap-1">
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={resetMut.isPending}
                      onClick={resetTotal}
                    >
                      {t("quota.resetTotal")}
                    </Button>
                    {resetError && <span className="text-destructive">{resetError}</span>}
                  </div>
                )}
              </div>
            </div>
          );
        })}
      </CardContent>
    </Card>
  );
}
