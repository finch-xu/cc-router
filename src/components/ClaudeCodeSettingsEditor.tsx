import { useMemo, useState } from "react";
import CodeMirror from "@uiw/react-codemirror";
import { json } from "@codemirror/lang-json";
import { useT } from "@/i18n";
import { useProxyEndpoint } from "@/hooks/useSettings";
import {
  useApplyClaudeCodeSettings,
  useClaudeCodeSettings,
  useClaudeCodeStatus,
} from "@/hooks/useClaudeCodeSettings";
import {
  mergeRecommendedEnv,
  type ClaudeCodeEnvSnapshot,
} from "@/lib/recommendedClaudeCodeEnv";
import type { ClaudeCodeSyncStatus } from "@/types";

type BadgeKey = ClaudeCodeSyncStatus | "loading";

function statusTone(s: BadgeKey): "ok" | "warn" | "err" | "" {
  switch (s) {
    case "in_sync":
      return "ok";
    case "needs_apply":
      return "warn";
    case "parse_error":
      return "err";
    default:
      return "";
  }
}

export function ClaudeCodeSettingsEditor() {
  const { t } = useT();
  const { baseUrl, token } = useProxyEndpoint();
  const read = useClaudeCodeSettings();
  const status = useClaudeCodeStatus();
  const apply = useApplyClaudeCodeSettings();

  // 编辑器以「服务端拉到的内容」为基线; 用户开始编辑后 draft 不再是 null.
  const serverContent = read.data?.content ?? "";
  const [draft, setDraft] = useState<string | null>(null);
  const effective = draft ?? serverContent;
  const isDirty = draft !== null && draft !== serverContent;

  const parseError = useMemo(() => {
    if (!effective.trim()) return null;
    try {
      JSON.parse(effective);
      return null;
    } catch (e) {
      return (e as Error).message;
    }
  }, [effective]);

  /**
   * recommended.baseUrl 来自后端 ProxyStatus.base_url, 包含正确的 scheme (http/https) 和真实端口.
   * undefined 时 (status query 尚未返回) Insert 按钮会被 disable, 避免把错误的 URL 写进文件.
   */
  const recommended: ClaudeCodeEnvSnapshot | null = useMemo(
    () => (baseUrl ? { baseUrl, token } : null),
    [baseUrl, token],
  );

  // statusKey: 区分「加载中」与「文件不存在」, 避免初次进 tab 闪烁错误徽章.
  const statusKey: BadgeKey = status.data
    ? status.data.status
    : status.isError
      ? "parse_error"
      : "loading";

  const statusLabels: Record<BadgeKey, string> = useMemo(
    () => ({
      in_sync: t("guide.editor.status.inSync"),
      needs_apply: t("guide.editor.status.needsApply"),
      never_applied: t("guide.editor.status.neverApplied"),
      file_missing: t("guide.editor.status.fileMissing"),
      parse_error: t("guide.editor.status.parseError"),
      loading: t("common.loading"),
    }),
    [t],
  );

  const handleReload = async () => {
    if (isDirty && !window.confirm(t("guide.editor.confirm.discardDirty"))) return;
    setDraft(null);
    await read.refetch();
  };

  const handleInsertRecommended = () => {
    if (!recommended) {
      window.alert(t("guide.editor.toast.endpointLoading"));
      return;
    }
    const merged = mergeRecommendedEnv(effective, recommended);
    if (merged === null) {
      window.alert(t("guide.editor.toast.invalidJson"));
      return;
    }
    setDraft(merged);
  };

  const handleSave = async () => {
    if (parseError) {
      window.alert(t("guide.editor.toast.saveFail", { reason: parseError }));
      return;
    }
    // 文件不存在时, 即使 effective 为空也得给 Rust 一个合法 JSON object — 写空骨架.
    const payload = effective.trim() ? effective : "{}\n";
    try {
      const outcome = await apply.mutateAsync(payload);
      let msg = t("guide.editor.toast.saveOk", { path: outcome.path });
      if (outcome.backup_path) {
        msg += "\n\n" + t("guide.editor.toast.backupMade", {
          path: outcome.backup_path,
        });
      }
      window.alert(msg);
      setDraft(null);
    } catch (e) {
      window.alert(t("guide.editor.toast.saveFail", { reason: String(e) }));
    }
  };

  // Save 按钮: 文件不存在时不要求 isDirty (允许用户直接创建空骨架).
  const saveDisabled =
    !!parseError ||
    apply.isPending ||
    (!isDirty && statusKey !== "file_missing");

  return (
    <div className="card section">
      <div
        className="card-head"
        style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12 }}
      >
        <div style={{ minWidth: 0, flex: 1 }}>
          <div className="card-title">{t("guide.editor.title")}</div>
          <span
            className="card-sub mono"
            style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", display: "block" }}
          >
            {read.data?.path ?? "~/.claude/settings.json"}
          </span>
        </div>
        <span
          className={"pill " + statusTone(statusKey)}
          style={{ display: "inline-flex", alignItems: "center", gap: 4, flexShrink: 0 }}
        >
          <span className="dot" />
          {statusLabels[statusKey]}
        </span>
      </div>
      <div className="card-body">
        <div className="field-hint" style={{ marginBottom: 10 }}>
          {t("guide.editor.subtitle")}
        </div>
        <div
          style={{
            border: "1px solid var(--line-2)",
            borderRadius: 6,
            overflow: "hidden",
          }}
        >
          <CodeMirror
            value={effective}
            height="320px"
            extensions={[json()]}
            onChange={(val) => setDraft(val)}
            basicSetup={{
              lineNumbers: true,
              foldGutter: true,
              highlightActiveLine: false,
            }}
          />
        </div>
        {parseError && (
          <div
            className="field-hint"
            style={{ color: "oklch(0.55 0.18 28)", marginTop: 6 }}
          >
            {t("guide.editor.parseHint")}: {parseError}
          </div>
        )}
        <div style={{ display: "flex", gap: 8, marginTop: 12, flexWrap: "wrap" }}>
          <button
            type="button"
            className="btn"
            onClick={handleReload}
            disabled={read.isFetching}
          >
            {t("guide.editor.btn.reload")}
          </button>
          <button
            type="button"
            className="btn"
            onClick={handleInsertRecommended}
            disabled={!recommended}
          >
            {t("guide.editor.btn.insert")}
          </button>
          <button
            type="button"
            className="btn primary"
            onClick={handleSave}
            disabled={saveDisabled}
          >
            {apply.isPending ? t("common.loading") : t("guide.editor.btn.save")}
          </button>
        </div>
      </div>
    </div>
  );
}
