import { Badge } from "@/components/ui/badge";
import { useT } from "@/i18n";
import type { Compatibility } from "@/types";

const META: Record<
  Compatibility,
  { labelKey: string; variant: "default" | "secondary" | "destructive" | "outline" }
> = {
  verified: { labelKey: "providerBadge.verified", variant: "default" },
  partial: { labelKey: "providerBadge.partial", variant: "secondary" },
  untested: { labelKey: "providerBadge.untested", variant: "outline" },
};

export function ProviderBadge({ compatibility }: { compatibility: Compatibility }) {
  const { t } = useT();
  const meta = META[compatibility];
  return <Badge variant={meta.variant}>{t(meta.labelKey)}</Badge>;
}
