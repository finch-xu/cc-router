import { useCallback, useEffect, useRef, useState } from "react";
import { RefreshCw, CircleAlert, CircleQuestionMark } from "lucide-react";
import { useT, type TFunction } from "@/i18n";
import { SLOT_EFFORT_LEVELS } from "@/lib/modelSlots";
import { MODEL_RENDER_LIMIT, searchModels } from "@/lib/modelSearch";
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

export type ModelListMode = "auto" | "manual";
type Mode = ModelListMode;

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
  /** 刷新按钮文案, 缺省「刷新模型列表」。还没拉过列表的场景 (自定义订阅新建页) 用「获取」更贴切。 */
  refreshLabel?: string;
  exampleModels?: string[];
  disabled?: boolean;
  /** systemone 订阅: 只显示 Jev 槽一行, 隐藏 effort 与刷新按钮 */
  systemone?: boolean;
  /** 自定义 systemone 订阅: 没有示例模型可回退, Jev 槽必填 (换成不带「可选」的文案) */
  jevRequired?: boolean;
  /**
   * 订阅编辑页: 四个主槽的模型与思考强度交给 SlotScheduleEditor (同一份 value / efforts),
   * 自动 / 手动与刷新挂在卡片右上角 (`ModelListModeBar`), 这里只留兜底槽。新建向导不传 (四槽必填)。
   */
  hideMainSlots?: boolean;
  /** 受控的自动 / 手动 (编辑页由卡片右上角的开关驱动); 不传则组件自己管 */
  mode?: Mode;
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

/** System One 订阅只有这一行: 留空 = 透传客户端 model; 无 effort (Jev 协议没有思考强度)。 */
const JEV_ROW: SlotRow = {
  key: "jev",
  labelKey: "modelSlot.jev.label",
  hintKey: "modelSlot.jev.hint",
};

/** 自定义 System One 订阅的 Jev 行: 必填, 不提「示例模型」(自定义订阅没有)。 */
const JEV_REQUIRED_ROW: SlotRow = {
  key: "jev",
  labelKey: "modelSlot.jev.requiredLabel",
  hintKey: "modelSlot.jev.requiredHint",
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
  refreshLabel,
  exampleModels,
  disabled,
  systemone,
  jevRequired,
  hideMainSlots,
  mode: controlledMode,
}: Props) {
  const { t } = useT();
  const [ownMode, chooseMode] = useModelListMode(models, error);
  const [showEffortHelp, setShowEffortHelp] = useState(false);
  const controlled = controlledMode !== undefined;

  // systemone 没有模型列表可选, 恒为手动输入 (这样示例提示也会显示)
  const effectiveMode: Mode = systemone ? "manual" : (controlledMode ?? ownMode);

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

  // systemone 只有 Jev 一行, 没有主槽可隐藏
  const splitEffort = !!hideMainSlots && !systemone;

  function effortSelect(key: keyof SlotEfforts, labelKey: string, flex: string) {
    return (
      <SlotEffortSelect
        value={efforts[key]}
        onChange={(v) => updateEffort(key, v)}
        disabled={disabled || effortDisabled}
        disabledReason={effortDisabled ? effortDisabledReason : undefined}
        ariaLabel={t("slotEffort.aria", { slot: t(labelKey) })}
        flex={flex}
      />
    );
  }

  const effortHelpToggle = (
    <button
      type="button"
      className="help-toggle"
      aria-label={t("slotEffort.helpAria")}
      aria-expanded={showEffortHelp}
      onClick={() => setShowEffortHelp((v) => !v)}
    >
      <CircleQuestionMark size={13} />
    </button>
  );

  const effortHelpPanel = showEffortHelp && !systemone && (
    <div className="field-hint help-panel">
      {t("slotEffort.hint")}
      {effortDisabled && effortDisabledReason && (
        <>
          <br />
          {effortDisabledReason}
        </>
      )}
    </div>
  );

  return (
    <div>
      {!controlled && (
        <div style={{ marginBottom: 14 }}>
          <ModelListModeBar
            label={t("modelSlot.label")}
            mode={effectiveMode}
            onChoose={chooseMode}
            onRefresh={onRefresh}
            refreshLabel={refreshLabel}
            loading={loading}
            disabled={disabled}
            systemone={systemone}
          />
        </div>
      )}

      {error && (
        <div className="alert warn" style={{ marginBottom: 12 }}>
          <CircleAlert size={14} />
          <span>{t("modelSlot.errPrefix")}{error}{t("modelSlot.errSuffix")}</span>
        </div>
      )}

      {effectiveMode === "manual" && exampleModels && exampleModels.length > 0 && (
        <div className="field-hint" style={{ marginTop: 0, marginBottom: 10 }}>
          {t("modelSlot.examplePrefix")}{exampleModels.join(", ")}
        </div>
      )}

      {/* 列头: 与下方控件行同 flex 结构 (左 flex:1 / 右 104px) 保证对齐 */}
      {!splitEffort && (
        <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 8 }}>
          <div style={{ flex: 1, minWidth: 0, fontSize: 12, fontWeight: 500, color: "var(--ink-3)" }}>
            {t("modelSlot.colModel")}
          </div>
          {!systemone && (
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
              {effortHelpToggle}
            </div>
          )}
        </div>
      )}

      {!splitEffort && effortHelpPanel}

      <div style={{ display: "grid", gap: 14 }}>
        {(systemone
          ? [jevRequired ? JEV_REQUIRED_ROW : JEV_ROW]
          : splitEffort
            ? [FALLBACK_ROW]
            : [...SLOTS, FALLBACK_ROW]
        ).map(({ key, labelKey, hintKey }) => {
          const isOptionalSlot = key === "fallback" || (key === "jev" && !jevRequired);
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
                        noneLabel={isOptionalSlot ? t("modelSlot.fallback.none") : undefined}
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
                          <CircleAlert size={14} />
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
                        key === "jev" && !jevRequired
                          ? t("modelSlot.jev.none")
                          : isOptionalSlot
                            ? t("modelSlot.fallback.none")
                            : t("modelSlot.modelIdPh")
                      }
                      disabled={disabled}
                    />
                  )}
                </div>
                {/* 右: 思考档位。固定宽度让各行右边缘对齐; 兜底槽无 effort, 放占位块保持对齐
                    (思考强度单列成网格时兜底槽独占一行, 不需要占位) */}
                {systemone || splitEffort ? null : isOptionalSlot ? (
                  <div style={{ flex: "0 0 104px" }} />
                ) : (
                  effortSelect(key as keyof SlotEfforts, labelKey, "0 0 104px")
                )}
              </div>
            </div>
          );
        })}
      </div>

      {/* 长说明已收进列头的 (?) 面板; 底部只保留 Kiro 灰掉原因这种必须常显的信息 */}
      {!systemone && effortDisabled && effortDisabledReason && (
        <div className="field-hint" style={{ marginTop: 10 }}>
          {effortDisabledReason}
        </div>
      )}
    </div>
  );
}

/**
 * 模型下拉: Radix Select + 面板顶部搜索框 (列表可能上百项)。
 * noneLabel: 空值是合法选择时给出 (兜底槽「不配置」、分时段默认模型「不设置」), 列表首项即它;
 * 不给时空值只是未选提示。SlotScheduleEditor 也用它。
 */
/**
 * 模型列表的自动 / 手动。null 表示还没初始化; 一旦用户主动点击切换, 不再被外部 data 反向覆盖。
 * 拉不到列表 (报错或空列表) 时初始为手动。
 */
export function useModelListMode(
  models: ModelInfo[] | null,
  error?: string | null,
): [ModelListMode, (next: ModelListMode) => void] {
  const [mode, setMode] = useState<ModelListMode | null>(null);
  const userChoseRef = useRef(false);

  useEffect(() => {
    if (userChoseRef.current) return;
    if (error || (models && models.length === 0)) {
      setMode("manual");
    } else if (models && models.length > 0) {
      setMode("auto");
    }
  }, [error, models]);

  const choose = useCallback((next: ModelListMode) => {
    userChoseRef.current = true;
    setMode(next);
  }, []);
  return [mode ?? "auto", choose];
}

/** 「模型列表 自动 / 手动」开关 + 刷新按钮。编辑页挂在槽位卡片右上角, 新建向导在 ModelSlotPicker 顶部。 */
export function ModelListModeBar({
  label,
  mode,
  onChoose,
  onRefresh,
  refreshLabel,
  loading,
  disabled,
  systemone,
}: {
  label: string;
  mode: ModelListMode;
  onChoose: (next: ModelListMode) => void;
  onRefresh?: () => void;
  refreshLabel?: string;
  loading?: boolean;
  disabled?: boolean;
  systemone?: boolean;
}) {
  const { t } = useT();
  return (
    <div className="model-list-bar">
      <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
        <span style={{ fontSize: 13, fontWeight: 500, color: "var(--ink-2)" }}>{label}</span>
        {!systemone && (
          <div className="radio-group">
            <button
              className={mode === "auto" ? "on" : ""}
              onClick={() => onChoose("auto")}
              disabled={disabled}
              type="button"
            >
              {t("modelSlot.modeAuto")}
            </button>
            <button
              className={mode === "manual" ? "on" : ""}
              onClick={() => onChoose("manual")}
              disabled={disabled}
              type="button"
            >
              {t("modelSlot.modeManual")}
            </button>
          </div>
        )}
      </div>
      {onRefresh && !systemone && (
        <button className="btn sm" onClick={onRefresh} disabled={disabled || loading} type="button">
          <RefreshCw size={12} className={loading ? "animate-spin" : undefined} />
          {refreshLabel ?? t("modelSlot.refresh")}
        </button>
      )}
    </div>
  );
}

/** 单个槽位的思考强度下拉。`onChange("")` = auto (透传客户端 effort)。 */
export function SlotEffortSelect({
  value,
  onChange,
  disabled,
  disabledReason,
  ariaLabel,
  flex,
  autoLabel,
}: {
  value: SlotEffort | undefined;
  onChange: (next: string) => void;
  disabled?: boolean;
  disabledReason?: string;
  ariaLabel: string;
  flex?: string;
  /** 「不强制」那一项的文案, 缺省「自动」(时段行用「跟随槽位」) */
  autoLabel?: string;
}) {
  const { t } = useT();
  return (
    <Select
      value={value ?? EFFORT_AUTO}
      onValueChange={(v) => onChange(v === EFFORT_AUTO ? "" : v)}
      disabled={disabled}
    >
      <SelectTrigger style={flex ? { flex } : undefined} aria-label={ariaLabel} title={disabledReason}>
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value={EFFORT_AUTO}>{autoLabel ?? t("slotEffort.auto")}</SelectItem>
        {SLOT_EFFORT_LEVELS.map((lv) => (
          <SelectItem key={lv} value={lv}>
            {lv}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

export function ModelSelect({
  id,
  noneLabel,
  current,
  models,
  showHistorical,
  maxCustomLength,
  disabled,
  onSelect,
  ariaLabel,
  t,
}: {
  id?: string;
  noneLabel?: string;
  current: string;
  models: ModelInfo[];
  showHistorical: boolean;
  maxCustomLength?: number;
  disabled?: boolean;
  onSelect: (v: string) => void;
  ariaLabel?: string;
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
  const typed = query.trim();
  // 模糊匹配 + 截到上限; 当前选中项永远保留 (卸载它会让 Radix 把焦点从搜索框抢回 listbox, 丢按键)
  const { shown, total } = searchModels(models, typed, current);
  // 列表里没有的名字 (网关第三方模型 / 刚上线的模型) 也能直接选, 不必切到手动模式
  const customName =
    typed && typed !== current && !models.some((m) => m.id === typed) ? typed : null;
  // 有真实匹配时「使用 …」放末尾: 放最前容易被顺手点中, 把上游并不存在的片段 (如 glm5) 填进槽位
  const customFirst = !shown.some((m) => m.id !== current);
  const customItem = customName && (
    // 固定 key: 输入时只换 value, 不反复卸载重挂
    <SelectItem key="__custom__" value={customName} className="font-mono">
      {t("modelSlot.useCustom", { name: customName })}
    </SelectItem>
  );
  return (
    <Select
      // 核心槽空值只是未选提示 → Radix placeholder; 兜底槽空值是合法选择 (= 透传) → sentinel 项
      value={current || (noneLabel !== undefined ? MODEL_NONE : "")}
      onValueChange={(v) => onSelect(noneLabel !== undefined && v === MODEL_NONE ? "" : v)}
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
      <SelectTrigger
        id={id}
        className="font-mono"
        style={{ flex: 1, minWidth: 0 }}
        aria-label={ariaLabel}
      >
        <SelectValue placeholder={noneLabel ?? t("modelSlot.placeholder")} />
      </SelectTrigger>
      <SelectContent
        header={
          <input
            ref={inputRef}
            className="input"
            style={{ fontSize: 12, padding: "6px 10px" }}
            placeholder={t("modelSlot.searchPlaceholder")}
            maxLength={maxCustomLength}
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
        {noneLabel !== undefined && <SelectItem value={MODEL_NONE}>{noneLabel}</SelectItem>}
        {showHistorical && (
          <SelectItem value={current} className="font-mono">
            {current}
            {t("modelSlot.historicalSuffix")}
          </SelectItem>
        )}
        {customFirst && customItem}
        {shown.map((m) => (
          <SelectItem key={m.id} value={m.id} className="font-mono">
            {m.display_name || m.id}
          </SelectItem>
        ))}
        {!customFirst && customItem}
        {total > MODEL_RENDER_LIMIT && (
          <div style={{ padding: "8px 10px", fontSize: 12, color: "var(--ink-4)" }}>
            {t("modelSlot.moreResults", { count: total - MODEL_RENDER_LIMIT })}
          </div>
        )}
        {shown.length === 0 && !customName && (
          <div style={{ padding: "8px 10px", fontSize: 12, color: "var(--ink-4)" }}>
            {t("modelSlot.searchNoResult")}
          </div>
        )}
      </SelectContent>
    </Select>
  );
}
