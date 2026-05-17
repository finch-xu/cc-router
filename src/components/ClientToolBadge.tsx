import { useT } from "@/i18n";
import { CLIENT_TOOLS_BY_ID, UNKNOWN_CLIENT_ICON } from "@/lib/clientTools";
import type { ClientToolId } from "@/types";

interface Props {
  /** 后端返回的 client_tool 字段; undefined / 不在 SUPPORTED_TOOLS 内即视为未识别 */
  toolId?: string;
  /** 可选: 鼠标悬停展示完整 User-Agent (表格场景用; 详情页已有专门字段不需要传) */
  userAgent?: string;
  iconSize?: number;
}

export function ClientToolBadge({ toolId, userAgent, iconSize = 14 }: Props) {
  const { t } = useT();
  const meta = toolId ? CLIENT_TOOLS_BY_ID[toolId as ClientToolId] : undefined;
  const Icon = meta?.icon ?? UNKNOWN_CLIENT_ICON;
  const label = meta ? t(meta.i18nKey) : t("requestLogs.client.unknown");
  return (
    <span
      style={{ display: "inline-flex", alignItems: "center", gap: 6 }}
      title={userAgent}
    >
      <Icon size={iconSize} />
      <span style={{ fontSize: 12 }}>{label}</span>
    </span>
  );
}
