import { useEffect, useRef, useState } from "react";
import { RefreshCw, AlertCircle } from "lucide-react";
import { useT } from "@/i18n";
import type { ModelInfo, ModelSlots } from "@/types";

type Mode = "auto" | "manual";

interface Props {
  value: ModelSlots;
  onChange: (next: ModelSlots) => void;
  models: ModelInfo[] | null;
  loading?: boolean;
  error?: string | null;
  onRefresh?: () => void;
  exampleModels?: string[];
  disabled?: boolean;
}

const SLOTS: Array<{ key: keyof ModelSlots; labelKey: string; hintKey: string }> = [
  { key: "fable",  labelKey: "modelSlot.fable.label",  hintKey: "modelSlot.fable.hint" },
  { key: "opus",   labelKey: "modelSlot.opus.label",   hintKey: "modelSlot.opus.hint" },
  { key: "sonnet", labelKey: "modelSlot.sonnet.label", hintKey: "modelSlot.sonnet.hint" },
  { key: "haiku",  labelKey: "modelSlot.haiku.label",  hintKey: "modelSlot.haiku.hint" },
];

export function ModelSlotPicker({
  value,
  onChange,
  models,
  loading,
  error,
  onRefresh,
  exampleModels,
  disabled,
}: Props) {
  const { t } = useT();
  // null 表示还没初始化;一旦用户主动点击切换,userChose 置 true,不再被外部 data 反向覆盖。
  const [mode, setMode] = useState<Mode | null>(null);
  const userChoseRef = useRef(false);

  useEffect(() => {
    if (userChoseRef.current) return;
    if (error || (models && models.length === 0)) {
      setMode("manual");
    } else if (models && models.length > 0) {
      setMode("auto");
    }
  }, [error, models]);

  const effectiveMode: Mode = mode ?? "auto";

  function chooseMode(next: Mode) {
    userChoseRef.current = true;
    setMode(next);
  }

  function update(key: keyof ModelSlots, v: string) {
    onChange({ ...value, [key]: v });
  }

  return (
    <div>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          marginBottom: 14,
          gap: 12,
          flexWrap: "wrap",
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
          <span style={{ fontSize: 13, fontWeight: 500, color: "var(--ink-2)" }}>
            {t("modelSlot.label")}
          </span>
          <div className="radio-group">
            <button
              className={effectiveMode === "auto" ? "on" : ""}
              onClick={() => chooseMode("auto")}
              disabled={disabled}
              type="button"
            >
              {t("modelSlot.modeAuto")}
            </button>
            <button
              className={effectiveMode === "manual" ? "on" : ""}
              onClick={() => chooseMode("manual")}
              disabled={disabled}
              type="button"
            >
              {t("modelSlot.modeManual")}
            </button>
          </div>
        </div>
        {onRefresh && (
          <button
            className="btn sm"
            onClick={onRefresh}
            disabled={disabled || loading}
            type="button"
          >
            <RefreshCw size={12} className={loading ? "animate-spin" : undefined} />
            {t("modelSlot.refresh")}
          </button>
        )}
      </div>

      {error && (
        <div className="alert warn" style={{ marginBottom: 12 }}>
          <AlertCircle size={14} />
          <span>{t("modelSlot.errPrefix")}{error}{t("modelSlot.errSuffix")}</span>
        </div>
      )}

      {effectiveMode === "manual" && exampleModels && exampleModels.length > 0 && (
        <div className="field-hint" style={{ marginTop: 0, marginBottom: 10 }}>
          {t("modelSlot.examplePrefix")}{exampleModels.join(", ")}
        </div>
      )}

      <div style={{ display: "grid", gap: 14 }}>
        {SLOTS.map(({ key, labelKey, hintKey }) => {
          const current = value[key];
          const inList = !!models && models.some((m) => m.id === current);
          const showHistorical =
            effectiveMode === "auto" && !!models && models.length > 0 && !!current && !inList;
          return (
            <div key={key}>
              <label className="field-label" htmlFor={`slot-${key}`}>
                {t(labelKey)}
                <span style={{ color: "var(--ink-4)", fontWeight: 400, marginLeft: 6 }}>
                  {t(hintKey)}
                </span>
              </label>
              {effectiveMode === "auto" && models && models.length > 0 ? (
                <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                  <select
                    id={`slot-${key}`}
                    className="select mono"
                    style={{ flex: 1 }}
                    value={current || ""}
                    onChange={(e) => update(key, e.target.value)}
                    disabled={disabled}
                  >
                    <option value="" disabled>
                      {t("modelSlot.placeholder")}
                    </option>
                    {showHistorical && (
                      <option value={current}>{current}{t("modelSlot.historicalSuffix")}</option>
                    )}
                    {models.map((m) => (
                      <option key={m.id} value={m.id}>
                        {m.display_name || m.id}
                      </option>
                    ))}
                  </select>
                  {showHistorical && (
                    <span
                      title={t("modelSlot.historicalTitle", { model: current })}
                      style={{ color: "var(--warn, #d97706)", display: "inline-flex" }}
                    >
                      <AlertCircle size={14} />
                    </span>
                  )}
                </div>
              ) : (
                <input
                  id={`slot-${key}`}
                  className="input mono"
                  value={current}
                  onChange={(e) => update(key, e.target.value)}
                  placeholder={t("modelSlot.modelIdPh")}
                  disabled={disabled}
                />
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
