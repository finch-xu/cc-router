import { useEffect, useState } from "react";
import { Gauge, LoaderCircle } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useResetTotalQuotaUsage, useUpdateTokenQuotas } from "@/hooks/useSubscriptions";
import { useT } from "@/i18n";
import { cn } from "@/lib/utils";
import { fmtCompact, fmtTime, fmtTimeShort } from "@/lib/format";
import { formatTokenShorthand, parseTokenShorthand } from "@/lib/quota";
import type { QuotaPeriod, QuotaUsageDto, SubscriptionDto, TokenQuotas } from "@/types";
import { errorText } from "@/lib/errorText";
import { runtime } from "@/runtime";

interface Props {
  subscription: SubscriptionDto;
  onChanged?: () => void;
}

// 颜色与 src/routes/Statistics.tsx::TOKEN_BAR_SEGMENTS 保持一致 (同为 4 类 token 的配色),
// 那边的常量是路由内部私有的, 这里按值复制而不是新开一个共享 lib 模块。
export const QUOTA_SEGMENTS: Array<{
  key: keyof Pick<QuotaUsageDto, "input" | "output" | "cache_creation" | "cache_read">;
  labelKey: string;
  color: string;
}> = [
  { key: "input", labelKey: "quota.legend.input", color: "oklch(0.62 0.13 240)" },
  { key: "output", labelKey: "quota.legend.output", color: "oklch(0.50 0.16 240)" },
  { key: "cache_creation", labelKey: "quota.legend.cacheCreation", color: "oklch(0.70 0.05 270)" },
  { key: "cache_read", labelKey: "quota.legend.cacheRead", color: "var(--ink-4)" },
];

const PERIODS: QuotaPeriod[] = ["daily", "weekly", "monthly", "total"];

type Drafts = Record<QuotaPeriod, string>;

function draftsOf(q: TokenQuotas | undefined): Drafts {
  return {
    daily: formatTokenShorthand(q?.daily),
    weekly: formatTokenShorthand(q?.weekly),
    monthly: formatTokenShorthand(q?.monthly),
    total: formatTokenShorthand(q?.total),
  };
}

/**
 * 用量限额: 设置与进度合在一张卡片里。每个周期一行 = 限额输入 + 用量条 + 恢复时间
 * (累计总量那行是「重置累计计数」)。改输入时用量条按草稿值即时重画; 保存走 update_token_quotas。
 * 没设限额的周期也显示已用量 (后端四个周期都会返回 quota_usage)。
 */
export function SubscriptionQuotaCard({ subscription, onChanged }: Props) {
  const { t } = useT();
  const updateMut = useUpdateTokenQuotas();
  const resetMut = useResetTotalQuotaUsage();
  const [drafts, setDrafts] = useState<Drafts>(() => draftsOf(subscription.token_quotas));
  const [saveError, setSaveError] = useState<string | null>(null);
  const [resetError, setResetError] = useState<string | null>(null);

  // 换了一个订阅才重置草稿; 同一订阅的后台刷新不覆盖用户正在改的值
  useEffect(() => {
    setDrafts(draftsOf(subscription.token_quotas));
    setSaveError(null);
    setResetError(null);
  }, [subscription.id]);

  const saved = draftsOf(subscription.token_quotas);
  const parsed = Object.fromEntries(
    PERIODS.map((p) => [p, parseTokenShorthand(drafts[p])]),
  ) as Record<QuotaPeriod, number | null | undefined>;
  const anyInvalid = PERIODS.some((p) => parsed[p] === null);
  const dirty = PERIODS.some((p) => drafts[p].trim() !== saved[p].trim());

  async function save() {
    if (!dirty || anyInvalid) return;
    setSaveError(null);
    const out: TokenQuotas = {};
    for (const p of PERIODS) {
      const v = parsed[p];
      if (typeof v === "number") out[p] = v;
    }
    try {
      const dto = await updateMut.mutateAsync({ id: subscription.id, quotas: out });
      setDrafts(draftsOf(dto.token_quotas));
      onChanged?.();
    } catch (e) {
      setSaveError(errorText(e));
    }
  }

  async function resetTotal() {
    if (!(await runtime.confirm(t("quota.resetTotalConfirm")))) return;
    setResetError(null);
    try {
      await resetMut.mutateAsync(subscription.id);
      onChanged?.();
    } catch (e) {
      setResetError(errorText(e));
    }
  }

  const usageOf = (p: QuotaPeriod) => subscription.quota_usage.find((q) => q.period === p);

  return (
    <Card>
      <CardHeader className="flex-row flex-wrap items-start justify-between gap-x-6 gap-y-3 space-y-0">
        <div className="min-w-0 flex-[1_1_360px] space-y-1.5">
          <CardTitle className="flex items-center gap-2">
            <Gauge className="h-4 w-4" /> {t("quota.title")}
          </CardTitle>
          <p className="text-sm text-muted-foreground">{t("quota.desc")}</p>
        </div>
        <div className="flex flex-wrap gap-3 pt-1 text-xs text-muted-foreground">
          {QUOTA_SEGMENTS.map((s) => (
            <span key={s.key} className="flex items-center gap-1">
              <i className="inline-block h-2.5 w-2.5 rounded-sm" style={{ background: s.color }} />
              {t(s.labelKey)}
            </span>
          ))}
        </div>
      </CardHeader>
      <CardContent>
        <div className="quota-rows">
          {PERIODS.map((p) => {
            const q = usageOf(p);
            const used = q ? q.input + q.output + q.cache_creation + q.cache_read : 0;
            const limit = parsed[p];
            const invalid = limit === null;
            const has = typeof limit === "number";
            const ratio = has ? Math.min(used / limit, 1) : 0;
            const exceeded = has && used >= limit;
            const warn = has && !exceeded && ratio >= 0.8;
            const changed = drafts[p].trim() !== saved[p].trim();
            const tip = q
              ? QUOTA_SEGMENTS.map((s) => `${t(s.labelKey)} ${fmtCompact(q[s.key])}`).join(" · ")
              : undefined;
            const inputId = `quota-${p}`;
            return (
              <div key={p} className="quota-row">
                <label htmlFor={inputId} className="text-sm font-medium">
                  {t(`quota.period.${p}`)}
                </label>
                <div className="flex items-center gap-1.5">
                  <input
                    id={inputId}
                    className={cn("input mono quota-input", changed && "changed", invalid && "invalid")}
                    value={drafts[p]}
                    placeholder={t("quota.unlimited")}
                    aria-invalid={invalid || undefined}
                    onChange={(e) => setDrafts({ ...drafts, [p]: e.target.value })}
                    disabled={updateMut.isPending}
                  />
                  <span className="text-xs text-muted-foreground">token</span>
                </div>
                <div className="min-w-0">
                  {has ? (
                    <div
                      className={cn("quota-track", warn && "warn", exceeded && "exceeded")}
                      title={tip}
                      role="img"
                      aria-label={t("quota.usedOfPct", {
                        used: fmtCompact(used),
                        limit: formatTokenShorthand(limit),
                        pct: Math.round((used / limit) * 100),
                      })}
                    >
                      {q &&
                        QUOTA_SEGMENTS.map((s) => (
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
                  ) : (
                    <div className="quota-track empty" title={tip} />
                  )}
                  <div
                    className={cn(
                      "mt-1.5 text-xs",
                      invalid
                        ? "text-destructive"
                        : exceeded
                          ? "font-semibold text-destructive"
                          : warn
                            ? "text-(--warn)"
                            : "text-muted-foreground",
                    )}
                  >
                    {invalid
                      ? t("quota.invalid")
                      : has
                        ? t("quota.usedOfPct", {
                            used: fmtCompact(used),
                            limit: formatTokenShorthand(limit),
                            pct: Math.round((used / limit) * 100),
                          })
                        : t("quota.usedUnlimited", { used: fmtCompact(used) })}
                  </div>
                </div>
                <div className="flex flex-wrap items-center justify-end gap-2 text-right text-xs text-muted-foreground">
                  {exceeded && <span className="pill err">{t("quota.exceeded")}</span>}
                  {q?.period_end_ms != null ? (
                    <span className="whitespace-nowrap">
                      {t("quota.resetAt", {
                        time: p === "daily" ? fmtTimeShort(q.period_end_ms) : fmtTime(q.period_end_ms),
                      })}
                    </span>
                  ) : (
                    p === "total" && (
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={resetMut.isPending}
                        onClick={resetTotal}
                      >
                        {t("quota.resetTotal")}
                      </Button>
                    )
                  )}
                  {p === "total" && resetError && (
                    <span className="basis-full text-destructive">{resetError}</span>
                  )}
                </div>
              </div>
            );
          })}
        </div>
        <div className="mt-4 flex flex-wrap items-center gap-x-4 gap-y-2">
          <span
            className={cn("mr-auto text-xs", anyInvalid ? "text-destructive" : "text-muted-foreground")}
          >
            {saveError ?? (anyInvalid ? t("quota.hintInvalid") : t("quota.hint"))}
          </span>
          {dirty && <span className="hand text-(--accent-ink)">{t("quota.dirty")}</span>}
          <Button size="sm" onClick={save} disabled={!dirty || anyInvalid || updateMut.isPending}>
            {updateMut.isPending && <LoaderCircle className="h-4 w-4 animate-spin" />}
            {t("quota.save")}
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}
