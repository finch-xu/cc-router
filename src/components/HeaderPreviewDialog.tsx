import { useQuery } from "@tanstack/react-query";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { HeaderTable, type HeaderTag } from "@/components/request-detail/HeaderTable";
import { api } from "@/api/tauri";
import { useT } from "@/i18n";
import type { AuthHeaderFormat, DropReason, HeaderPolicy, OutboundSource } from "@/types";

/**
 * 订阅请求头设置的「发送效果预览」: 后端用与真实请求同一个组装函数,
 * 按当前草稿 (未保存也算) 处理一份示例 Claude Code 请求头。
 */
export function HeaderPreviewDialog({
  open,
  onClose,
  subscriptionId,
  policy,
  headers,
  auth,
}: {
  open: boolean;
  onClose: () => void;
  subscriptionId: string;
  policy: HeaderPolicy;
  /** 用户可编辑的那张表的草稿: 内置订阅是自定义请求头, 自定义订阅是连接区的请求头 */
  headers: Record<string, string>;
  /** 自定义订阅的鉴权头草稿 (内置订阅不传) */
  auth?: { name: string; format: AuthHeaderFormat };
}) {
  const { t } = useT();
  const query = useQuery({
    queryKey: ["header-preview", subscriptionId, policy, headers, auth],
    queryFn: () => api.previewOutboundHeaders(subscriptionId, policy, headers, auth),
    enabled: open,
    staleTime: 0,
  });

  const sent = query.data?.sent.map((h) => [h.name, h.value, h.source] as [string, string, OutboundSource]);
  const dropped = query.data?.dropped.map((h) => [h.name, h.value, h.reason] as [string, string, DropReason]);
  const sourceTag = ([, , src]: [string, string, OutboundSource]): HeaderTag => ({
    label: t(`requestLogs.detail.headers.source.${src}`),
    tone: src === "forwarded" ? "ok" : src === "cc_router" ? "muted" : "accent",
  });
  const reasonTag = ([, , reason]: [string, string, DropReason]): HeaderTag => ({
    label: t(`subscriptionEdit.headerPreview.reason.${reason}`),
    tone: reason === "overridden" ? "accent" : "muted",
  });
  const errorText = query.isError ? String((query.error as { message?: string })?.message ?? query.error) : "";

  return (
    <Dialog open={open} onOpenChange={(v) => !v && onClose()}>
      <DialogContent className="cc-dialog hpv">
        <DialogHeader>
          <DialogTitle>{t("subscriptionEdit.headerPreview.title")}</DialogTitle>
        </DialogHeader>
        <p className="rd-note hpv-note">{t("subscriptionEdit.headerPreview.sampleNote")}</p>
        <div className="hpv-body">
          <HeaderTable
            title={t("subscriptionEdit.headerPreview.sent")}
            pairs={sent ?? null}
            loading={query.isLoading}
            tagFor={sourceTag}
            emptyText={errorText}
          />
          {!query.isError && (
            <HeaderTable
              title={t("subscriptionEdit.headerPreview.dropped")}
              pairs={dropped ?? null}
              loading={query.isLoading}
              tagFor={reasonTag}
              emptyText=""
            />
          )}
          <section>
            <h3 className="rd-pane-title">
              <span>{t("subscriptionEdit.headerPreview.rules")}</span>
            </h3>
            <p className="rd-note">{t("subscriptionEdit.headerPreview.priority")}</p>
            <p className="rd-note">{t("subscriptionEdit.headerPolicy.hardDeny")}</p>
          </section>
        </div>
      </DialogContent>
    </Dialog>
  );
}
