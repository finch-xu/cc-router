import { cn } from "@/lib/utils";
import { useT, type TFunction } from "@/i18n";
import type { SubscriptionState } from "@/types";

export type StatusTone = "ok" | "warn" | "err" | "accent" | "neutral";

interface Meta {
  labelKey: string;
  tone: StatusTone;
}

const STATE_META: Record<SubscriptionState, Meta> = {
  healthy:         { labelKey: "subscriptionState.healthy",         tone: "ok" },
  rate_limited:    { labelKey: "subscriptionState.rate_limited",    tone: "warn" },
  quota_exhausted: { labelKey: "subscriptionState.quota_exhausted", tone: "warn" },
  transient_error: { labelKey: "subscriptionState.transient_error", tone: "warn" },
  auth_failed:     { labelKey: "subscriptionState.auth_failed",     tone: "err" },
  disabled:        { labelKey: "subscriptionState.disabled",        tone: "neutral" },
};

export function stateTone(state: SubscriptionState): StatusTone {
  return STATE_META[state].tone;
}

/** Translate a state to the localized label. Pass the t function from useT(). */
export function stateLabel(state: SubscriptionState, t: TFunction): string {
  return t(STATE_META[state].labelKey);
}

interface Props {
  state: SubscriptionState;
  className?: string;
}

export function StatusBadge({ state, className }: Props) {
  const { t } = useT();
  const meta = STATE_META[state];
  return (
    <span className={cn("pill", meta.tone !== "neutral" && meta.tone, className)}>
      <span className="dot" />
      {t(meta.labelKey)}
    </span>
  );
}

/** 仅渲染一个状态点(用于行内紧凑场景),无 pill 包裹 */
export function StatusDot({ state, className }: Props) {
  const { t } = useT();
  const meta = STATE_META[state];
  return (
    <span
      className={cn("status-dot", `status-dot-${meta.tone}`, className)}
      aria-label={t(meta.labelKey)}
    />
  );
}
