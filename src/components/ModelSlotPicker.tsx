import { useEffect, useRef, useState } from "react";
import { RefreshCw, AlertCircle, HelpCircle } from "lucide-react";
import { useT, type TFunction } from "@/i18n";
import { SLOT_EFFORT_LEVELS } from "@/lib/modelSlots";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { ModelInfo, ModelSlots, SlotEffort, SlotEfforts } from "@/types";

/** Radix SelectItem 不允许 value=""; 空值语义各自映射成 sentinel, 写回 state 前还原。 */
const EFFORT_AUTO = "__auto__";
const MODEL_NONE = "__none__";

type Mode = "auto" | "manual";

interface Props {
  value: ModelSlots;
  onChange: (next: ModelSlots) => void;
  /** 每槽位 effort 覆盖。字段缺失 = auto (透传客户端 effort)。 */
  efforts: SlotEfforts;
  onEffortsChange: (next: SlotEfforts) => void;
  /** true 时 effort 下拉灰掉 (Kiro: CodeWhisperer 协议没有 reasoning 字段)。 */
  effortDisabled?: boolean;
  /** 灰掉的原因, 显示在下方说明处并挂 title。 */
  effortDisabledReason?: string;
  models: ModelInfo[] | null;
  loading?: boolean;
  error?: string | null;
  onRefresh?: () => void;
  exampleModels?: string[];
  disabled?: boolean;
}

type SlotRow = { key: keyof ModelSlots; labelKey: string; hintKey: string };

const SLOTS: SlotRow[] = [
  { key: "fable",  labelKey: "modelSlot.fable.label",  hintKey: "modelSlot.fable.hint" },
  { key: "opus",   labelKey: "modelSlot.opus.label",   hintKey: "modelSlot.opus.hint" },
  { key: "sonnet", labelKey: "modelSlot.sonnet.label", hintKey: "modelSlot.sonnet.hint" },
  { key: "haiku",  labelKey: "modelSlot.haiku.label",  hintKey: "modelSlot.haiku.hint" },
];

/**
 * 兜底槽 (可选, fallback 虚拟模型专用): 留空 = 透传未知 model。
 * 与四个核心槽同循环渲染保持控件一致, 但不参与必填校验、无 effort 下拉。
 */
const FALLBACK_ROW: SlotRow = {
  key: "fallback",
  labelKey: "modelSlot.fallback.label",
  hintKey: "modelSlot.fallback.hint",
};

export function ModelSlotPicker({
  value,
  onChange,
  efforts,
  onEffortsChange,
  effortDisabled,
  effortDisabledReason,
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
  const [showEffortHelp, setShowEffortHelp] = useState(false);

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

  /** "" 是 auto 的 sentinel, 归一化成字段缺失, 保证 patch 里不出现空串。 */
  function updateEffort(key: keyof SlotEfforts, v: string) {
    const next = { ...efforts };
    if (v) {
      next[key] = v as SlotEffort;
    } else {
      delete next[key];
    }
    onEffortsChange(next);
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

      {/* 列头: 与下方控件行同 flex 结构 (左 flex:1 / 右 104px) 保证对齐 */}
      <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 8 }}>
        <div style={{ flex: 1, minWidth: 0, fontSize: 12, fontWeight: 500, color: "var(--ink-3)" }}>
          {t("modelSlot.colModel")}
        </div>
        <div
          style={{
            flex: "0 0 104px",
            display: "flex",
            alignItems: "center",
            gap: 4,
            fontSize: 12,
            fontWeight: 500,
            color: "var(--ink-3)",
          }}
        >
          {t("slotEffort.colLabel")}
          <button
            type="button"
            className="help-toggle"
            aria-label={t("slotEffort.helpAria")}
            aria-expanded={showEffortHelp}
            onClick={() => setShowEffortHelp((v) => !v)}
          >
            <HelpCircle size={13} />
          </button>
        </div>
      </div>

      {showEffortHelp && (
        <div className="field-hint help-panel">
          {t("slotEffort.hint")}
          {effortDisabled && effortDisabledReason && (
            <>
              <br />
              {effortDisabledReason}
            </>
          )}
        </div>
      )}

      <div style={{ display: "grid", gap: 14 }}>
        {[...SLOTS, FALLBACK_ROW].map(({ key, labelKey, hintKey }) => {
          const isFallbackSlot = key === "fallback";
          const current = value[key] ?? "";
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
              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                {/* 左: 模型控件。minWidth:0 防止长模型名把右侧 effort 下拉挤掉 */}
                <div
                  style={{
                    flex: 1,
                    minWidth: 0,
                    display: "flex",
                    alignItems: "center",
                    gap: 8,
                  }}
                >
                  {effectiveMode === "auto" && models && models.length > 0 ? (
                    <>
                      <ModelSelect
                        id={`slot-${key}`}
                        isFallbackSlot={isFallbackSlot}
                        current={current}
                        models={models}
                        showHistorical={showHistorical}
                        disabled={disabled}
                        onSelect={(v) => update(key, v)}
                        t={t}
                      />
                      {showHistorical && (
                        <span
                          title={t("modelSlot.historicalTitle", { model: current })}
                          style={{ color: "var(--warn, #d97706)", display: "inline-flex" }}
                        >
                          <AlertCircle size={14} />
                        </span>
                      )}
                    </>
                  ) : (
                    <input
                      id={`slot-${key}`}
                      className="input mono"
                      style={{ flex: 1, minWidth: 0 }}
                      value={current}
                      onChange={(e) => update(key, e.target.value)}
                      placeholder={
                        isFallbackSlot ? t("modelSlot.fallback.none") : t("modelSlot.modelIdPh")
                      }
                      disabled={disabled}
                    />
                  )}
                </div>
                {/* 右: 思考档位。固定宽度让各行右边缘对齐; 兜底槽无 effort, 放占位块保持对齐 */}
                {isFallbackSlot ? (
                  <div style={{ flex: "0 0 104px" }} />
                ) : (
                  <Select
                    value={efforts[key as keyof SlotEfforts] ?? EFFORT_AUTO}
                    onValueChange={(v) =>
                      updateEffort(key as keyof SlotEfforts, v === EFFORT_AUTO ? "" : v)
                    }
                    disabled={disabled || effortDisabled}
                  >
                    <SelectTrigger
                      style={{ flex: "0 0 104px" }}
                      aria-label={t("slotEffort.aria", { slot: t(labelKey) })}
                      title={effortDisabled ? effortDisabledReason : undefined}
                    >
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={EFFORT_AUTO}>{t("slotEffort.auto")}</SelectItem>
                      {SLOT_EFFORT_LEVELS.map((lv) => (
                        <SelectItem key={lv} value={lv}>
                          {lv}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                )}
              </div>
            </div>
          );
        })}
      </div>

      {/* 长说明已收进列头的 (?) 面板; 底部只保留 Kiro 灰掉原因这种必须常显的信息 */}
      {effortDisabled && effortDisabledReason && (
        <div className="field-hint" style={{ marginTop: 10 }}>
          {effortDisabledReason}
        </div>
      )}
    </div>
  );
}

/** 模型下拉: Radix Select + 面板顶部搜索框 (列表可能上百项)。 */
function ModelSelect({
  id,
  isFallbackSlot,
  current,
  models,
  showHistorical,
  disabled,
  onSelect,
  t,
}: {
  id: string;
  isFallbackSlot: boolean;
  current: string;
  models: ModelInfo[];
  showHistorical: boolean;
  disabled?: boolean;
  onSelect: (v: string) => void;
  t: TFunction;
}) {
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  // 过滤会卸载列表项 (含选中项), Radix FocusScope 随之把焦点抢回 listbox,
  // 导致只能输入一个字符 — 每次 query 变化后把焦点还给搜索框。
  useEffect(() => {
    if (!open || !query) return;
    const timer = setTimeout(() => inputRef.current?.focus(), 0);
    return () => clearTimeout(timer);
  }, [open, query]);
  const q = query.trim().toLowerCase();
  // 当前选中项永远保留: 它一旦被过滤卸载, Radix 会把焦点从搜索框抢回 listbox (丢按键)
  const filtered = q
    ? models.filter(
        (m) =>
          m.id === current ||
          m.id.toLowerCase().includes(q) ||
          (m.display_name ?? "").toLowerCase().includes(q),
      )
    : models;
  return (
    <Select
      // 核心槽空值只是未选提示 → Radix placeholder; 兜底槽空值是合法选择 (= 透传) → sentinel 项
      value={current || (isFallbackSlot ? MODEL_NONE : undefined)}
      onValueChange={(v) => onSelect(v === MODEL_NONE ? "" : v)}
      disabled={disabled}
      onOpenChange={(next) => {
        setOpen(next);
        if (next) {
          setQuery("");
          // Radix 打开后会把焦点交给选中项, 稍后再抢回给搜索框
          setTimeout(() => inputRef.current?.focus(), 0);
        }
      }}
    >
      <SelectTrigger id={id} className="font-mono" style={{ flex: 1, minWidth: 0 }}>
        <SelectValue
          placeholder={
            isFallbackSlot ? t("modelSlot.fallback.none") : t("modelSlot.placeholder")
          }
        />
      </SelectTrigger>
      <SelectContent
        header={
          <input
            ref={inputRef}
            className="input"
            style={{ fontSize: 12, padding: "6px 10px" }}
            placeholder={t("modelSlot.searchPlaceholder")}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            // 阻断 Radix Select 的 typeahead / 方向键抢焦点; Esc 放行用于关闭面板
            onKeyDown={(e) => {
              if (e.key !== "Escape") e.stopPropagation();
            }}
          />
        }
      >
        {/* sentinel / 历史项可能正是当前选中项, 不随搜索卸载 (卸载选中项会触发 Radix 抢焦点) */}
        {isFallbackSlot && (
          <SelectItem value={MODEL_NONE}>{t("modelSlot.fallback.none")}</SelectItem>
        )}
        {showHistorical && (
          <SelectItem value={current} className="font-mono">
            {current}
            {t("modelSlot.historicalSuffix")}
          </SelectItem>
        )}
        {filtered.map((m) => (
          <SelectItem key={m.id} value={m.id} className="font-mono">
            {m.display_name || m.id}
          </SelectItem>
        ))}
        {filtered.length === 0 && (
          <div style={{ padding: "8px 10px", fontSize: 12, color: "var(--ink-4)" }}>
            {t("modelSlot.searchNoResult")}
          </div>
        )}
      </SelectContent>
    </Select>
  );
}
