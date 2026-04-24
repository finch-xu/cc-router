import { Badge } from "@/components/ui/badge";
import type { Compatibility } from "@/types";

const META: Record<Compatibility, { label: string; variant: "default" | "secondary" | "destructive" | "outline" }> = {
  verified: { label: "已验证", variant: "default" },
  partial: { label: "部分兼容", variant: "secondary" },
  untested: { label: "未测试", variant: "outline" },
};

export function ProviderBadge({ compatibility }: { compatibility: Compatibility }) {
  const meta = META[compatibility];
  return <Badge variant={meta.variant}>{meta.label}</Badge>;
}
