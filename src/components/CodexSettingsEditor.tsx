import { useMemo, useState } from "react";
import CodeMirror from "@uiw/react-codemirror";
import { json } from "@codemirror/lang-json";
import { StreamLanguage } from "@codemirror/language";
import { toml as tomlMode } from "@codemirror/legacy-modes/mode/toml";
import { useT } from "@/i18n";
import { useProxyEndpoint } from "@/hooks/useSettings";
import {
  useApplyCodexAuth,
  useApplyCodexConfig,
  useCodexAuth,
  useCodexAuthStatus,
  useCodexConfig,
  useCodexConfigStatus,
} from "@/hooks/useCodexSettings";
import {
  buildRecommendedCodexAuth,
  buildRecommendedCodexConfig,
  type CodexSnapshot,
} from "@/lib/recommendedCodexConfig";
import type { CodexSyncStatus } from "@/types";

type BadgeKey = CodexSyncStatus | "loading";

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

const TOML_EXT = StreamLanguage.define(tomlMode);
const JSON_EXT = json();

export function CodexSettingsEditor() {
  const { t } = useT();
  const { baseUrl, token } = useProxyEndpoint();
  // 同 queryKey 的 useQuery 在 react-query 内 dedupe, 顶层调用不会重复 fetch.
  // 我们需要 serverContent 来判定「Insert 内容与当前一致 → 不要置脏 draft」, 避免 Save 按钮亮起
  // 后让用户做一次无意义的写盘.
  const configRead = useCodexConfig();
  const authRead = useCodexAuth();

  const recommended: CodexSnapshot | null = useMemo(
    () => (baseUrl ? { baseUrl, token } : null),
    [baseUrl, token],
  );

  const statusLabels = useMemo(
    (): Record<BadgeKey, string> => ({
      in_sync: t("guide.codex.status.inSync"),
      needs_apply: t("guide.codex.status.needsApply"),
      never_applied: t("guide.codex.status.neverApplied"),
      file_missing: t("guide.codex.status.fileMissing"),
      parse_error: t("guide.codex.status.parseError"),
      loading: t("common.loading"),
    }),
    [t],
  );

  // 联动 Insert: 同时填充 config + auth 两个 draft.
  // 子组件通过 prop 拿 draft 与 setter, 顶层在这里做协调.
  const [configDraft, setConfigDraft] = useState<string | null>(null);
  const [authDraft, setAuthDraft] = useState<string | null>(null);

  const handleInsertBoth = () => {
    if (!recommended) {
      window.alert(t("guide.codex.toast.endpointLoading"));
      return;
    }
    // 已是 in_sync 状态点 Insert: 生成的内容与文件完全一致, 不再 setDraft 避免假 dirty.
    const cfg = buildRecommendedCodexConfig(recommended);
    if (cfg !== (configRead.data?.content ?? "")) setConfigDraft(cfg);
    const auth = buildRecommendedCodexAuth(recommended);
    if (auth !== (authRead.data?.content ?? "")) setAuthDraft(auth);
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
      {/* 顶部联动 Insert */}
      <div className="card section" style={{ marginBottom: 0 }}>
        <div
          className="card-head"
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            gap: 12,
          }}
        >
          <div style={{ minWidth: 0, flex: 1 }}>
            <div className="card-title">{t("guide.codex.editor.title")}</div>
            <span className="card-sub">{t("guide.codex.editor.subtitle")}</span>
          </div>
          <button
            type="button"
            className="btn"
            onClick={handleInsertBoth}
            disabled={!recommended}
          >
            {t("guide.codex.btn.insertBoth")}
          </button>
        </div>
      </div>

      <CodexConfigCard
        statusLabels={statusLabels}
        draft={configDraft}
        setDraft={setConfigDraft}
      />
      <CodexAuthCard
        statusLabels={statusLabels}
        draft={authDraft}
        setDraft={setAuthDraft}
      />
    </div>
  );
}

interface SubCardProps {
  statusLabels: Record<BadgeKey, string>;
  draft: string | null;
  setDraft: (v: string | null) => void;
}

function CodexConfigCard({ statusLabels, draft, setDraft }: SubCardProps) {
  const { t } = useT();
  const read = useCodexConfig();
  const status = useCodexConfigStatus();
  const apply = useApplyCodexConfig();

  const serverContent = read.data?.content ?? "";
  const effective = draft ?? serverContent;
  const isDirty = draft !== null && draft !== serverContent;

  const statusKey: BadgeKey = status.data
    ? status.data.status
    : status.isError
      ? "parse_error"
      : "loading";

  const handleReload = async () => {
    if (isDirty && !window.confirm(t("guide.codex.confirm.discardDirty"))) return;
    setDraft(null);
    await read.refetch();
  };

  const handleSave = async () => {
    // TOML 合法性靠后端 toml_edit 兜底; 前端不重复 parse, 失败时显示 Rust 报错.
    const payload = effective.trim() ? effective : "";
    if (!payload) {
      window.alert(t("guide.codex.toast.invalidToml"));
      return;
    }
    try {
      const outcome = await apply.mutateAsync(payload);
      let msg = t("guide.codex.toast.saveOk", { path: outcome.path });
      if (outcome.backup_path) {
        msg += "\n\n" + t("guide.codex.toast.backupMade", { path: outcome.backup_path });
      }
      window.alert(msg);
      setDraft(null);
    } catch (e) {
      window.alert(t("guide.codex.toast.saveFail", { reason: String(e) }));
    }
  };

  const saveDisabled =
    apply.isPending || (!isDirty && statusKey !== "file_missing");

  return (
    <div className="card section">
      <div
        className="card-head"
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          gap: 12,
        }}
      >
        <div style={{ minWidth: 0, flex: 1 }}>
          <div className="card-title">{t("guide.codex.config.title")}</div>
          <span
            className="card-sub mono"
            style={{
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
              display: "block",
            }}
          >
            {read.data?.path ?? "~/.codex/config.toml"}
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
        <div
          style={{
            border: "1px solid var(--line-2)",
            borderRadius: 6,
            overflow: "hidden",
          }}
        >
          <CodeMirror
            value={effective}
            height="240px"
            extensions={[TOML_EXT]}
            onChange={(val) => setDraft(val)}
            basicSetup={{
              lineNumbers: true,
              foldGutter: true,
              highlightActiveLine: false,
            }}
          />
        </div>
        <div style={{ display: "flex", gap: 8, marginTop: 12, flexWrap: "wrap" }}>
          <button
            type="button"
            className="btn"
            onClick={handleReload}
            disabled={read.isFetching}
          >
            {t("guide.codex.btn.reload")}
          </button>
          <button
            type="button"
            className="btn primary"
            onClick={handleSave}
            disabled={saveDisabled}
          >
            {apply.isPending ? t("common.loading") : t("guide.codex.btn.save")}
          </button>
        </div>
      </div>
    </div>
  );
}

function CodexAuthCard({ statusLabels, draft, setDraft }: SubCardProps) {
  const { t } = useT();
  const read = useCodexAuth();
  const status = useCodexAuthStatus();
  const apply = useApplyCodexAuth();

  const serverContent = read.data?.content ?? "";
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

  const statusKey: BadgeKey = status.data
    ? status.data.status
    : status.isError
      ? "parse_error"
      : "loading";

  const hasChatGptOauth = status.data?.has_chatgpt_oauth ?? false;

  const handleReload = async () => {
    if (isDirty && !window.confirm(t("guide.codex.confirm.discardDirty"))) return;
    setDraft(null);
    await read.refetch();
  };

  const handleSave = async () => {
    if (parseError) {
      window.alert(t("guide.codex.toast.saveFail", { reason: parseError }));
      return;
    }
    // 检测到 OAuth 凭据时, 写入前给用户最后一次确认 (会备份但破坏现有 ChatGPT 登录).
    if (hasChatGptOauth && !window.confirm(t("guide.codex.confirm.overwriteOauth"))) {
      return;
    }
    const payload = effective.trim() ? effective : "{}\n";
    try {
      const outcome = await apply.mutateAsync(payload);
      let msg = t("guide.codex.toast.saveOk", { path: outcome.path });
      if (outcome.backup_path) {
        msg += "\n\n" + t("guide.codex.toast.backupMade", { path: outcome.backup_path });
      }
      window.alert(msg);
      setDraft(null);
    } catch (e) {
      window.alert(t("guide.codex.toast.saveFail", { reason: String(e) }));
    }
  };

  const saveDisabled =
    !!parseError ||
    apply.isPending ||
    (!isDirty && statusKey !== "file_missing");

  return (
    <div className="card section">
      <div
        className="card-head"
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          gap: 12,
        }}
      >
        <div style={{ minWidth: 0, flex: 1 }}>
          <div className="card-title">{t("guide.codex.auth.title")}</div>
          <span
            className="card-sub mono"
            style={{
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
              display: "block",
            }}
          >
            {read.data?.path ?? "~/.codex/auth.json"}
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
        {hasChatGptOauth && (
          <div
            className="field-hint"
            style={{
              color: "oklch(0.55 0.18 65)",
              marginBottom: 10,
              padding: "8px 10px",
              border: "1px solid oklch(0.78 0.10 65)",
              borderRadius: 6,
            }}
          >
            {t("guide.codex.auth.oauthDetected")}
          </div>
        )}
        <div
          style={{
            border: "1px solid var(--line-2)",
            borderRadius: 6,
            overflow: "hidden",
          }}
        >
          <CodeMirror
            value={effective}
            height="240px"
            extensions={[JSON_EXT]}
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
            {t("guide.codex.parseHint")}: {parseError}
          </div>
        )}
        <div style={{ display: "flex", gap: 8, marginTop: 12, flexWrap: "wrap" }}>
          <button
            type="button"
            className="btn"
            onClick={handleReload}
            disabled={read.isFetching}
          >
            {t("guide.codex.btn.reload")}
          </button>
          <button
            type="button"
            className="btn primary"
            onClick={handleSave}
            disabled={saveDisabled}
          >
            {apply.isPending ? t("common.loading") : t("guide.codex.btn.save")}
          </button>
        </div>
      </div>
    </div>
  );
}
