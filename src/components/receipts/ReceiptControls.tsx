import { useMemo } from "react";
import { ChevronDown, FileImage, FileText, FileCode, RefreshCw } from "lucide-react";
import { useT } from "@/i18n";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import type { ReceiptRange, SubscriptionDto } from "@/types";
import type { ReceiptDisplayOptions } from "./ReceiptSlip";

/** 时间范围选择器渲染在页面标题行 (Receipts.tsx), 这里只导出选项表 */
export const RECEIPT_RANGES: { key: ReceiptRange; labelKey: string }[] = [
  { key: "last_24_hours", labelKey: "receipts.range.last24h" },
  { key: "last7_days", labelKey: "receipts.range.last7" },
  { key: "last30_days", labelKey: "receipts.range.last30" },
  { key: "last_year", labelKey: "receipts.range.lastYear" },
  { key: "all_time", labelKey: "receipts.range.all" },
];

interface Props {
  options: ReceiptDisplayOptions;
  onOptionsChange: (o: ReceiptDisplayOptions) => void;
  /** 选中的订阅 ID 集合; 空集合 = 全选(语义上「不过滤」) */
  selectedSubscriptionIds: Set<string>;
  onSelectedSubscriptionsChange: (s: Set<string>) => void;
  /** 选中的 provider id 集合; 空集合 = 全选 */
  selectedProviderIds: Set<string>;
  onSelectedProvidersChange: (s: Set<string>) => void;
  /** 勾选后从小票里剔除已删除订阅(subscription_display_name 为 nullish 的 sub_item)的用量 */
  excludeDeleted: boolean;
  onExcludeDeletedChange: (v: boolean) => void;
  isFetching: boolean;
  onRefresh: () => void;
  onExport: (kind: "png" | "pdf" | "html") => void;
  exportDisabled: boolean;
  exporting: boolean;
}

export function ReceiptControls({
  options,
  onOptionsChange,
  selectedSubscriptionIds,
  onSelectedSubscriptionsChange,
  selectedProviderIds,
  onSelectedProvidersChange,
  excludeDeleted,
  onExcludeDeletedChange,
  isFetching,
  onRefresh,
  onExport,
  exportDisabled,
  exporting,
}: Props) {
  const { t } = useT();
  const subs = useSubscriptions();

  const subsList: SubscriptionDto[] = subs.data ?? [];

  // 收集所有出现过的 provider — 来自订阅列表的 provider_id (含自定义 marker)
  const providerOptions = useMemo(() => {
    const map = new Map<string, string>();
    for (const s of subsList) {
      if (!map.has(s.provider_id)) {
        map.set(s.provider_id, s.provider_display_name);
      }
    }
    return Array.from(map, ([id, label]) => ({ id, label }));
  }, [subsList]);

  return (
    <div className="receipt-panel">
      {/* Section 1 — 导出 (置顶, 让用户一眼看到主操作); 刷新并进同一行 */}
      <Section title={t("receipts.controls.export.title")}>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}>
          <button
            className="btn"
            type="button"
            onClick={() => onExport("png")}
            disabled={exportDisabled || exporting}
          >
            <FileImage size={12} />
            {t("receipts.controls.export.png")}
          </button>
          <button
            className="btn"
            type="button"
            onClick={() => onExport("pdf")}
            disabled={exportDisabled || exporting}
          >
            <FileText size={12} />
            {t("receipts.controls.export.pdf")}
          </button>
          <button
            className="btn"
            type="button"
            onClick={() => onExport("html")}
            disabled={exportDisabled || exporting}
          >
            <FileCode size={12} />
            {t("receipts.controls.export.html")}
          </button>
          <button
            className="btn"
            type="button"
            onClick={onRefresh}
            disabled={isFetching}
            style={{ marginLeft: "auto" }}
          >
            <RefreshCw size={12} className={isFetching ? "spin" : undefined} />
            {t("common.refresh")}
          </button>
          {exporting && (
            <span className="receipt-field-desc" style={{ marginTop: 0, width: "100%" }}>
              {t("receipts.controls.export.exporting")}
            </span>
          )}
        </div>
      </Section>

      {/* Section 2 — 显示选项: 两个分段控件并排, 5 个勾选项双列 */}
      <Section title={t("receipts.controls.display.title")}>
        <div className="receipt-field-row">
          <div>
            <div className="receipt-field-label">
              {t("receipts.controls.display.groupMode.label")}
            </div>
            <div className="range-tabs" style={{ flexWrap: "wrap" }}>
              {(["virtual_model", "subscription", "totals_only"] as const).map((m) => (
                <button
                  key={m}
                  type="button"
                  className={"range-tab" + (options.groupMode === m ? " active" : "")}
                  onClick={() => onOptionsChange({ ...options, groupMode: m })}
                >
                  {t(`receipts.controls.display.groupMode.${m}`)}
                </button>
              ))}
            </div>
            <div className="receipt-field-desc">
              {t("receipts.controls.display.groupMode.desc")}
            </div>
          </div>
          <div>
            <div className="receipt-field-label">
              {t("receipts.controls.display.footerCode.label")}
            </div>
            <div className="range-tabs" style={{ flexWrap: "wrap" }}>
              {(["qr", "barcode"] as const).map((s) => (
                <button
                  key={s}
                  type="button"
                  className={"range-tab" + (options.footerCodeStyle === s ? " active" : "")}
                  onClick={() => onOptionsChange({ ...options, footerCodeStyle: s })}
                >
                  {t(`receipts.controls.display.footerCode.${s}`)}
                </button>
              ))}
            </div>
            <div className="receipt-field-desc">
              {t("receipts.controls.display.footerCode.desc")}
            </div>
          </div>
        </div>

        <div className="receipt-check-grid" style={{ marginTop: 12 }}>
          <CheckboxRow
            checked={options.colorMode === "color"}
            label={t("receipts.controls.display.colorMode")}
            desc={t("receipts.controls.display.colorModeDesc")}
            onChange={(v) => onOptionsChange({ ...options, colorMode: v ? "color" : "mono" })}
          />
          <CheckboxRow
            checked={options.showProviderLogo}
            label={t("receipts.controls.display.showProviderLogo")}
            desc={t("receipts.controls.display.showProviderLogoDesc")}
            onChange={(v) => onOptionsChange({ ...options, showProviderLogo: v })}
          />
          <CheckboxRow
            checked={options.showCacheTokens}
            label={t("receipts.controls.display.showCache")}
            desc={t("receipts.controls.display.showCacheDesc")}
            onChange={(v) => onOptionsChange({ ...options, showCacheTokens: v })}
          />
          <CheckboxRow
            checked={options.showRequestCounts}
            label={t("receipts.controls.display.showCounts")}
            desc={t("receipts.controls.display.showCountsDesc")}
            onChange={(v) => onOptionsChange({ ...options, showRequestCounts: v })}
          />
          <CheckboxRow
            checked={options.compactTokens}
            label={t("receipts.controls.display.compactTokens")}
            desc={t("receipts.controls.display.compactTokensDesc")}
            onChange={(v) => onOptionsChange({ ...options, compactTokens: v })}
          />
        </div>
      </Section>

      {/* Section 3 — 过滤: 两个下拉并排 */}
      <Section title={t("receipts.controls.filter.title")}>
        <div className="receipt-field-row">
          <FilterDropdown
            label={t("receipts.controls.filter.bySubscription")}
            allLabel={t("receipts.controls.filter.allSubscriptions")}
            options={subsList.map((s) => ({ id: s.id, label: s.display_name }))}
            selected={selectedSubscriptionIds}
            onChange={onSelectedSubscriptionsChange}
          />
          <FilterDropdown
            label={t("receipts.controls.filter.byProvider")}
            allLabel={t("receipts.controls.filter.allProviders")}
            options={providerOptions}
            selected={selectedProviderIds}
            onChange={onSelectedProvidersChange}
          />
        </div>
        <div style={{ marginTop: 10 }}>
          <CheckboxRow
            checked={excludeDeleted}
            label={t("receipts.controls.filter.excludeDeleted")}
            desc={t("receipts.controls.filter.excludeDeletedDesc")}
            onChange={onExcludeDeletedChange}
          />
        </div>
      </Section>
    </div>
  );
}

function Section({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div className="stats-section">
      <div className="stats-section-header">
        <div className="stats-section-title">{title}</div>
      </div>
      {children}
    </div>
  );
}

function CheckboxRow({
  checked,
  label,
  desc,
  onChange,
}: {
  checked: boolean;
  label: string;
  desc?: string;
  onChange: (v: boolean) => void;
}) {
  return (
    <label className="receipt-check">
      <input
        type="checkbox"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span className="receipt-check-text">
        <span className="receipt-check-label">{label}</span>
        {desc && <span className="receipt-check-desc">{desc}</span>}
      </span>
    </label>
  );
}

/**
 * 简易多选 dropdown — 用 details/summary 实现, 避免引入 popover 库。
 * selected 为空 = 「全部」(不应用过滤)。
 */
function FilterDropdown({
  label,
  allLabel,
  options,
  selected,
  onChange,
}: {
  label: string;
  allLabel: string;
  options: { id: string; label: string }[];
  selected: Set<string>;
  onChange: (s: Set<string>) => void;
}) {
  const summary = selected.size === 0 ? allLabel : `${selected.size} / ${options.length}`;
  const toggle = (id: string) => {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    onChange(next);
  };
  return (
    <details className="receipt-dropdown">
      <summary>
        <span>{label}</span>
        <span className="receipt-dropdown-value">
          <span>{summary}</span>
          <ChevronDown size={12} />
        </span>
      </summary>
      <div className="receipt-dropdown-body">
        {options.length === 0 && <div className="receipt-field-desc" style={{ marginTop: 0 }}>—</div>}
        {options.map((opt) => (
          <label key={opt.id}>
            <input
              type="checkbox"
              checked={selected.has(opt.id)}
              onChange={() => toggle(opt.id)}
            />
            <span>{opt.label}</span>
          </label>
        ))}
        {selected.size > 0 && (
          <button
            type="button"
            className="btn sm"
            onClick={() => onChange(new Set())}
            style={{ marginTop: 4, alignSelf: "flex-start" }}
          >
            {allLabel}
          </button>
        )}
      </div>
    </details>
  );
}
