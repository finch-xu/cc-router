import { useEffect, useState } from "react";
import { RefreshCw, AlertCircle } from "lucide-react";
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

const SLOTS: Array<{ key: keyof ModelSlots; label: string; hint: string }> = [
  { key: "opus", label: "Opus 槽", hint: "高级任务 / Plan Mode" },
  { key: "sonnet", label: "Sonnet 槽", hint: "主对话" },
  { key: "haiku", label: "Haiku 槽", hint: "小任务 / 工具调用" },
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
  const [mode, setMode] = useState<Mode>("auto");

  useEffect(() => {
    if (error || (models && models.length === 0)) {
      setMode("manual");
    } else if (models && models.length > 0) {
      setMode("auto");
    }
  }, [error, models]);

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
          <span style={{ fontSize: 13, fontWeight: 500, color: "var(--ink-2)" }}>模型槽位</span>
          <div className="radio-group">
            <button
              className={mode === "auto" ? "on" : ""}
              onClick={() => setMode("auto")}
              disabled={disabled}
              type="button"
            >
              自动
            </button>
            <button
              className={mode === "manual" ? "on" : ""}
              onClick={() => setMode("manual")}
              disabled={disabled}
              type="button"
            >
              手动
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
            刷新模型列表
          </button>
        )}
      </div>

      {error && (
        <div className="alert warn" style={{ marginBottom: 12 }}>
          <AlertCircle size={14} />
          <span>无法获取模型列表:{error}。已切换到手动输入。</span>
        </div>
      )}

      {mode === "manual" && exampleModels && exampleModels.length > 0 && (
        <div className="field-hint" style={{ marginTop: 0, marginBottom: 10 }}>
          示例:{exampleModels.join(", ")}
        </div>
      )}

      <div style={{ display: "grid", gap: 14 }}>
        {SLOTS.map(({ key, label, hint }) => (
          <div key={key}>
            <label className="field-label" htmlFor={`slot-${key}`}>
              {label}
              <span style={{ color: "var(--ink-4)", fontWeight: 400, marginLeft: 6 }}>
                {hint}
              </span>
            </label>
            {mode === "auto" && models && models.length > 0 ? (
              <select
                id={`slot-${key}`}
                className="select mono"
                value={value[key] || ""}
                onChange={(e) => update(key, e.target.value)}
                disabled={disabled}
              >
                <option value="" disabled>
                  请选择模型
                </option>
                {models.map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.display_name || m.id}
                  </option>
                ))}
              </select>
            ) : (
              <input
                id={`slot-${key}`}
                className="input mono"
                value={value[key]}
                onChange={(e) => update(key, e.target.value)}
                placeholder="填入厂商提供的模型 ID"
                disabled={disabled}
              />
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
