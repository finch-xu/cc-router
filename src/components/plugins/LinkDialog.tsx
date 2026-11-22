import { useEffect, useState } from "react";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Spinner } from "@/components/Spinner";
import { api } from "@/api/tauri";
import { useT } from "@/i18n";
import { errorText } from "@/lib/errorText";
import type { StagedPlugin } from "@/types";

/** 「从链接安装」: 粘贴 GitHub 链接 → 后端下载到暂存区 → 交给安装确认框 */
export function LinkDialog({
  open,
  onOpenChange,
  onStaged,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  onStaged: (s: StagedPlugin) => void;
}) {
  const { t } = useT();
  const [link, setLink] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) {
      setLink("");
      setError(null);
    }
  }, [open]);

  async function submit() {
    if (!link.trim()) return;
    setBusy(true);
    setError(null);
    try {
      const staged = await api.stagePluginFromGithub(link.trim());
      onOpenChange(false);
      onStaged(staged);
    } catch (e) {
      setError(t("plugins.fetchFailed", { error: errorText(e) }));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={(v) => !busy && onOpenChange(v)}>
      <DialogContent className="cc-dialog">
        <DialogHeader>
          <DialogTitle>{t("plugins.link.title")}</DialogTitle>
          <DialogDescription>{t("plugins.link.desc")}</DialogDescription>
        </DialogHeader>
        <input
          className="input mono"
          autoFocus
          spellCheck={false}
          placeholder={t("plugins.link.placeholder")}
          value={link}
          onChange={(e) => setLink(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && submit()}
        />
        {error && <div className="pg-alert err">{error}</div>}
        <DialogFooter>
          <button className="btn" type="button" onClick={() => onOpenChange(false)} disabled={busy}>
            {t("common.cancel")}
          </button>
          <button className="btn primary" type="button" onClick={submit} disabled={busy || !link.trim()}>
            {busy && <Spinner />}
            {t("plugins.btn.fetch")}
          </button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
