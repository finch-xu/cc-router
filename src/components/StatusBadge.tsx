import { cn } from "@/lib/utils";
import type { SubscriptionState } from "@/types";

export type StatusTone = "ok" | "warn" | "err" | "accent" | "neutral";

interface Meta {
  label: string;
  tone: StatusTone;
}

const STATE_META: Record<SubscriptionState, Meta> = {
  healthy:         { label: "正常",     tone: "ok" },
  rate_limited:    { label: "限流",     tone: "warn" },
  quota_exhausted: { label: "配额耗尽", tone: "warn" },
  transient_error: { label: "临时错误", tone: "warn" },
  auth_failed:     { label: "凭证失效", tone: "err" },
  disabled:        { label: "已禁用",   tone: "neutral" },
};

export function stateTone(state: SubscriptionState): StatusTone {
  return STATE_META[state].tone;
}

export function stateLabel(state: SubscriptionState): string {
  return STATE_META[state].label;
}

interface Props {
  state: SubscriptionState;
  className?: string;
}

export function StatusBadge({ state, className }: Props) {
  const meta = STATE_META[state];
  return (
    <span className={cn("pill", meta.tone !== "neutral" && meta.tone, className)}>
      <span className="dot" />
      {meta.label}
    </span>
  );
}

/** 仅渲染一个状态点(用于行内紧凑场景),无 pill 包裹 */
export function StatusDot({ state, className }: Props) {
  const meta = STATE_META[state];
  return (
    <span
      className={cn("status-dot", `status-dot-${meta.tone}`, className)}
      aria-label={meta.label}
    />
  );
}
