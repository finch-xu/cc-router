import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { LoaderCircle, RefreshCw } from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { api } from "@/api/tauri";
import { useT } from "@/i18n";
import { errorText } from "@/lib/errorText";
import { SUBSCRIPTIONS_KEY, SUBSCRIPTION_DETAIL_KEY } from "@/hooks/useSubscriptions";
import type { SubscriptionDto } from "@/types";

/**
 * 厂商描述 (内置 yaml / 厂商插件) 更新后, 订阅的连接快照不会自动跟着变。
 * 这里提示用户, 并在确认前逐项列出旧值 / 新值 (尤其是域名变化: API Key 会发往新的地方)。
 */
export function RebuildConnectionBanner({ sub }: { sub: SubscriptionDto }) {
  const { t } = useT();
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const preview = useQuery({
    queryKey: ["subscription-rebuild-preview", sub.id, sub.updated_at],
    queryFn: () => api.previewSubscriptionRebuild(sub.id),
    enabled: open,
  });

  if (!sub.connection_outdated) return null;

  async function rebuild() {
    setBusy(true);
    setError(null);
    try {
      await api.rebuildSubscriptionConnection(sub.id);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: SUBSCRIPTIONS_KEY }),
        queryClient.invalidateQueries({ queryKey: SUBSCRIPTION_DETAIL_KEY }),
      ]);
      setOpen(false);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  const p = preview.data;
  return (
    <>
      <Alert>
        <RefreshCw className="h-4 w-4" />
        <AlertTitle>{t("subscriptionEdit.rebuild.title")}</AlertTitle>
        <AlertDescription>
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span>{t("subscriptionEdit.rebuild.desc")}</span>
            <Button size="sm" variant="outline" onClick={() => setOpen(true)}>
              {t("subscriptionEdit.rebuild.review")}
            </Button>
          </div>
        </AlertDescription>
      </Alert>

      <Dialog open={open} onOpenChange={(v) => !busy && setOpen(v)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("subscriptionEdit.rebuild.dialogTitle")}</DialogTitle>
            <DialogDescription>{t("subscriptionEdit.rebuild.dialogDesc")}</DialogDescription>
          </DialogHeader>
          {preview.isLoading && <LoaderCircle className="h-4 w-4 animate-spin" />}
          {preview.isError && <div className="text-sm text-destructive">{errorText(preview.error)}</div>}
          {p?.status === "up_to_date" && <div className="text-sm text-muted-foreground">{t("subscriptionEdit.rebuild.upToDate")}</div>}
          {p?.status === "blocked" && p.reason && (
            <div className="text-sm text-destructive">{t(`subscriptionEdit.rebuild.blocked.${p.reason}`)}</div>
          )}
          {p?.status === "changed" && (
            <div className="space-y-3 text-sm">
              {p.changes.map((c) => (
                <div key={c.field}>
                  <div className="font-medium">{t(`subscriptionEdit.rebuild.field.${c.field}`)}</div>
                  <div className="grid grid-cols-[48px_1fr] gap-x-2 gap-y-1 font-mono text-xs">
                    <span className="text-muted-foreground">{t("subscriptionEdit.rebuild.old")}</span>
                    <span className="whitespace-pre-wrap break-all line-through opacity-70">{c.old || "—"}</span>
                    <span className="text-muted-foreground">{t("subscriptionEdit.rebuild.new")}</span>
                    <span className="whitespace-pre-wrap break-all">{c.new || "—"}</span>
                  </div>
                </div>
              ))}
            </div>
          )}
          {error && <div className="text-sm text-destructive">{error}</div>}
          <DialogFooter>
            <Button variant="ghost" onClick={() => setOpen(false)} disabled={busy}>
              {t("common.cancel")}
            </Button>
            <Button onClick={rebuild} disabled={busy || p?.status !== "changed"}>
              {busy && <LoaderCircle className="h-4 w-4 animate-spin" />}
              {t("subscriptionEdit.rebuild.confirm")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
