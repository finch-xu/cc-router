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

// 颜色与 src/routes/Statistics.tsx::TOKEN_BAR_SEGMENTS 保持一致 (同为 4 类 token 的配色),
// 那边的常量是路由内部私有的, 这里按值复制而不是新开一个共享 lib 模块。
const SEGMENTS: Array<{
  key: keyof Pick<QuotaUsageDto, "input" | "output" | "cache_creation" | "cache_read">;
  labelKey: string;
  color: string;
}> = [
  { key: "input", labelKey: "quota.legend.input", color: "oklch(0.62 0.13 240)" },
  { key: "output", labelKey: "quota.legend.output", color: "oklch(0.50 0.16 240)" },
  { key: "cache_creation", labelKey: "quota.legend.cacheCreation", color: "oklch(0.70 0.05 270)" },
  { key: "cache_read", labelKey: "quota.legend.cacheRead", color: "var(--ink-4)" },
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
                    q.exceeded && "text-destructive font-medium",
                  )}
                >
                  {q.exceeded
                    ? t("quota.exceededWithNumbers", {
                        used: fmtCompact(used),
                        limit: formatTokenShorthand(limit),
                      })
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
                    className="h-full shrink-0"
                    style={{
                      width: `${(Math.min(q[s.key], limit) / limit) * 100}%`,
                      background: s.color,
                    }}
                  />
                ))}
              </div>
              <div className="flex items-center justify-between text-xs text-muted-foreground">
                <span className="flex gap-3">
                  {SEGMENTS.map((s) => (
                    <span key={s.key} className="flex items-center gap-1">
                      <i
                        className="inline-block h-2 w-2 rounded-sm"
                        style={{ background: s.color }}
                      />
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
