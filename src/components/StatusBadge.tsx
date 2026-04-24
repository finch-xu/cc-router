import { cn } from "@/lib/utils";
import type { SubscriptionState } from "@/types";

const STATE_META: Record<
  SubscriptionState,
  { label: string; dotClass: string; textClass: string }
> = {
  healthy: {
    label: "正常",
    dotClass: "bg-status-healthy",
    textClass: "text-status-healthy",
  },
  rate_limited: {
    label: "限流",
    dotClass: "bg-status-rate_limited",
    textClass: "text-status-rate_limited",
  },
  quota_exhausted: {
    label: "配额耗尽",
    dotClass: "bg-status-quota_exhausted",
    textClass: "text-status-quota_exhausted",
  },
  transient_error: {
    label: "临时错误",
    dotClass: "bg-status-transient_error",
    textClass: "text-status-transient_error",
  },
  auth_failed: {
    label: "凭证失效",
    dotClass: "bg-status-auth_failed",
    textClass: "text-status-auth_failed",
  },
  disabled: {
    label: "已禁用",
    dotClass: "bg-status-disabled",
    textClass: "text-status-disabled",
  },
};

interface Props {
  state: SubscriptionState;
  showLabel?: boolean;
  className?: string;
}

export function StatusBadge({ state, showLabel = true, className }: Props) {
  const meta = STATE_META[state];
  return (
    <span className={cn("inline-flex items-center gap-1.5", className)}>
      <span className={cn("h-2 w-2 rounded-full", meta.dotClass)} aria-hidden />
      {showLabel && (
        <span className={cn("text-xs font-medium", meta.textClass)}>{meta.label}</span>
      )}
    </span>
  );
}
