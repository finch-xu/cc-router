import { useState } from "react";
import { Check, Copy, Eye, Lock, Plus, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { HeaderPreviewDialog } from "@/components/HeaderPreviewDialog";
import { Input } from "@/components/ui/input";
import { useT } from "@/i18n";
import { validateHeaderPolicyNames } from "@/lib/connectionValidation";
import { runtime } from "@/runtime";
import type { HeaderPolicy, HeaderPolicyMode } from "@/types";

const MODES: HeaderPolicyMode[] = ["none", "claude_code", "allowlist", "denylist", "all"];

/** 白名单模式下可一键勾选的预设项 (与后端 claude_code 预设一致)。 */
const PRESET_NAMES = ["anthropic-*", "x-claude-code-*", "x-stainless-*", "x-app", "user-agent"];

const AUTO_MODE_ENV = "CLAUDE_CODE_AUTO_MODE_SERVER=0";

/**
 * 订阅的客户端请求头透传策略 (受控)。只在 Anthropic 透传订阅显示。
 * 不影响订阅自己配置的请求头: 那些总是发送。
 */
export function HeaderPolicyEditor({
  value,
  onChange,
  subscriptionId,
  draftHeaders,
}: {
  value: HeaderPolicy;
  onChange: (p: HeaderPolicy) => void;
  subscriptionId: string;
  /** 用户可编辑的那张请求头表的草稿, 预览按它计算 */
  draftHeaders: Record<string, string>;
}) {
  const { t } = useT();
  const [draft, setDraft] = useState("");
  const [draftError, setDraftError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [previewOpen, setPreviewOpen] = useState(false);

  const names = value.names ?? [];
  const hasName = (n: string) => names.some((x) => x.toLowerCase() === n.toLowerCase());
  const setNames = (next: string[]) => onChange({ ...value, names: next });
  const toggleName = (n: string) =>
    setNames(hasName(n) ? names.filter((x) => x.toLowerCase() !== n.toLowerCase()) : [...names, n]);

  function addDraft() {
    const n = draft.trim();
    if (!n) return;
    const err = validateHeaderPolicyNames([n]);
    if (err) return setDraftError(t(err));
    if (!hasName(n)) setNames([...names, n]);
    setDraft("");
    setDraftError(null);
  }

  async function copyEnv() {
    try {
      await runtime.copyText(AUTO_MODE_ENV);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // ignore
    }
  }

  const showList = value.mode === "allowlist" || value.mode === "denylist";
  const customNames = value.mode === "allowlist" ? names.filter((n) => !PRESET_NAMES.includes(n)) : names;

  return (
    <div className="space-y-2">
      <div className="hp-options" role="radiogroup" aria-label={t("subscriptionEdit.headerPolicy.label")}>
        {MODES.map((m) => (
          <label key={m} className={value.mode === m ? "hp-option on" : "hp-option"}>
            <input
              type="radio"
              name="header-policy"
              checked={value.mode === m}
              // 切模式时名单保留, 切回来不丢 (只在 allowlist / denylist 下生效)
              onChange={() => onChange({ ...value, mode: m })}
            />
            <span>
              <span className="hp-option-title">{t(`subscriptionEdit.headerPolicy.mode.${m}`)}</span>
              <span className="hp-option-desc">{t(`subscriptionEdit.headerPolicy.modeDesc.${m}`)}</span>
            </span>
          </label>
        ))}
      </div>

      {showList && (
        <div className="hp-names">
          {value.mode === "allowlist" &&
            PRESET_NAMES.map((n) => (
              <label key={n} className="hp-chip">
                <input type="checkbox" checked={hasName(n)} onChange={() => toggleName(n)} />
                <span className="mono">{n}</span>
              </label>
            ))}
          {customNames.map((n) => (
            <span key={n} className="hp-chip">
              <span className="mono">{n}</span>
              <button
                type="button"
                className="hp-chip-x"
                aria-label={t("subscriptionEdit.headerPolicy.removeName", { name: n })}
                onClick={() => toggleName(n)}
              >
                <X size={12} />
              </button>
            </span>
          ))}
          <div className="flex items-center gap-2 w-full">
            <Input
              className="h-8 mono"
              value={draft}
              placeholder={t("subscriptionEdit.headerPolicy.namePh")}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  addDraft();
                }
              }}
            />
            <Button type="button" variant="outline" size="sm" onClick={addDraft}>
              <Plus size={14} />
              {t("subscriptionEdit.headerPolicy.addName")}
            </Button>
          </div>
          {draftError && <p className="text-xs text-destructive w-full">{draftError}</p>}
        </div>
      )}

      <p className="text-xs text-muted-foreground flex items-start gap-1.5">
        <Lock size={12} className="mt-0.5 shrink-0" aria-hidden />
        {t("subscriptionEdit.headerPolicy.hardDeny")}
      </p>
      <p className="text-xs text-muted-foreground">{t("subscriptionEdit.headerPolicy.mergeHint")}</p>
      <Button type="button" variant="outline" size="sm" className="hpv-open" onClick={() => setPreviewOpen(true)}>
        <Eye size={14} />
        {t("subscriptionEdit.headerPreview.open")}
      </Button>
      <HeaderPreviewDialog
        open={previewOpen}
        onClose={() => setPreviewOpen(false)}
        subscriptionId={subscriptionId}
        policy={value}
        headers={draftHeaders}
      />

      <div className="alert warn hp-auto-mode">
        <div className="space-y-1.5">
          <div>{t("subscriptionEdit.headerPolicy.autoModeNote")}</div>
          <div className="flex items-center gap-2">
            <code className="mono">{AUTO_MODE_ENV}</code>
            <Button type="button" variant="outline" size="sm" onClick={copyEnv}>
              {copied ? <Check size={12} /> : <Copy size={12} />}
              {copied ? t("common.copied") : t("common.copy")}
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
