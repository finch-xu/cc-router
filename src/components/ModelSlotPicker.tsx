import { useEffect, useState } from "react";
import { RefreshCw, AlertCircle } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Alert, AlertDescription } from "@/components/ui/alert";
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
    <div className="space-y-3">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <span className="text-sm font-medium">模型槽位</span>
          <span className="text-xs text-muted-foreground">
            模式:
            <button
              className={`ml-2 underline-offset-2 ${mode === "auto" ? "underline font-medium" : ""}`}
              onClick={() => setMode("auto")}
              disabled={disabled}
              type="button"
            >
              自动
            </button>
            {" / "}
            <button
              className={`underline-offset-2 ${mode === "manual" ? "underline font-medium" : ""}`}
              onClick={() => setMode("manual")}
              disabled={disabled}
              type="button"
            >
              手动
            </button>
          </span>
        </div>
        {onRefresh && (
          <Button
            variant="outline"
            size="sm"
            onClick={onRefresh}
            disabled={disabled || loading}
            type="button"
          >
            <RefreshCw className={loading ? "h-3 w-3 animate-spin" : "h-3 w-3"} />
            刷新模型列表
          </Button>
        )}
      </div>

      {error && (
        <Alert variant="warning">
          <AlertCircle className="h-4 w-4" />
          <AlertDescription>
            无法获取模型列表：{error}。已切换到手动输入。
          </AlertDescription>
        </Alert>
      )}

      {mode === "manual" && exampleModels && exampleModels.length > 0 && (
        <div className="text-xs text-muted-foreground">
          示例：{exampleModels.join(", ")}
        </div>
      )}

      <div className="grid gap-3">
        {SLOTS.map(({ key, label, hint }) => (
          <div key={key} className="grid grid-cols-[120px_1fr] items-center gap-3">
            <Label htmlFor={`slot-${key}`}>
              <div className="text-sm">{label}</div>
              <div className="text-[10px] font-normal text-muted-foreground">{hint}</div>
            </Label>
            {mode === "auto" && models && models.length > 0 ? (
              <Select
                value={value[key] || undefined}
                onValueChange={(v) => update(key, v)}
                disabled={disabled}
              >
                <SelectTrigger id={`slot-${key}`}>
                  <SelectValue placeholder="请选择模型" />
                </SelectTrigger>
                <SelectContent>
                  {models.map((m) => (
                    <SelectItem key={m.id} value={m.id}>
                      {m.display_name || m.id}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            ) : (
              <Input
                id={`slot-${key}`}
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
