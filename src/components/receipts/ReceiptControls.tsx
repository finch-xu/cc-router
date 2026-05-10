import { useMemo } from "react";
import { ChevronDown, FileImage, FileText, FileCode, RefreshCw } from "lucide-react";
import { useT } from "@/i18n";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import type { ReceiptRange, SubscriptionDto } from "@/types";
import type { ReceiptDisplayOptions } from "./ReceiptSlip";

const RANGES: { key: ReceiptRange; labelKey: string }[] = [
  { key: "last_24_hours", labelKey: "receipts.range.last24h" },
  { key: "last7_days", labelKey: "receipts.range.last7" },
  { key: "last30_days", labelKey: "receipts.range.last30" },
  { key: "last_year", labelKey: "receipts.range.lastYear" },
  { key: "all_time", labelKey: "receipts.range.all" },
];

interface Props {
  range: ReceiptRange;
  onRangeChange: (r: ReceiptRange) => void;
  options: ReceiptDisplayOptions;
  onOptionsChange: (o: ReceiptDisplayOptions) => void;
  /** 选中的订阅 ID 集合; 空集合 = 全选(语义上「不过滤」) */
  selectedSubscriptionIds: Set<string>;
  onSelectedSubscriptionsChange: (s: Set<string>) => void;
  /** 选中的 provider id 集合; 空集合 = 全选 */
  selectedProviderIds: Set<string>;
  onSelectedProvidersChange: (s: Set<string>) => void;
  isFetching: boolean;
  onRefresh: () => void;
  onExport: (kind: "png" | "pdf" | "html") => void;
  exportDisabled: boolean;
  exporting: boolean;
}

export function ReceiptControls({
  range,
  onRangeChange,
  options,
  onOptionsChange,
  selectedSubscriptionIds,
  onSelectedSubscriptionsChange,
  selectedProviderIds,
  onSelectedProvidersChange,
  isFetching,
  onRefresh,
  onExport,
  exportDisabled,
  exporting,
}: Props) {
  const { t } = useT();
  const subs = useSubscriptions();

  const subsList: SubscriptionDto[] = subs.data ?? [];

  // 收集所有出现过的 provider — 来自订阅列表的 provider_id (非 __custom__)
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
    <div style={{ display: "flex", flexDirection: "column", gap: 24 }}>
      {/* Section 1 — 导出 (置顶, 让用户一眼看到主操作) */}
      <Section title={t("receipts.controls.export.title")}>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
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
          {exporting && (
            <span className="field-hint" style={{ alignSelf: "center", fontSize: 11 }}>
              {t("receipts.controls.export.exporting")}
            </span>
          )}
        </div>
        <div style={{ marginTop: 6 }}>
          <button
            className="btn"
            type="button"
            onClick={onRefresh}
            disabled={isFetching}
          >
            <RefreshCw size={12} className={isFetching ? "spin" : undefined} />
            {t("common.refresh")}
          </button>
        </div>
      </Section>

      {/* Section 2 — 时间范围 */}
      <Section title={t("receipts.controls.range.title")}>
        <div className="range-tabs" style={{ flexWrap: "wrap" }}>
          {RANGES.map((r) => (
            <button
              key={r.key}
              type="button"
              className={"range-tab" + (range === r.key ? " active" : "")}
              onClick={() => onRangeChange(r.key)}
            >
              {t(r.labelKey)}
            </button>
          ))}
        </div>
      </Section>

      {/* Section 3 — 显示选项 */}
      <Section title={t("receipts.controls.display.title")}>
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
      </Section>

      {/* Section 4 — 过滤 */}
      <Section title={t("receipts.controls.filter.title")}>
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
      <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
        {children}
      </div>
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
    <label
      style={{
        display: "flex",
        gap: 8,
        cursor: "pointer",
        alignItems: "flex-start",
      }}
    >
      <input
        type="checkbox"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
        style={{ marginTop: 3 }}
      />
      <span>
        <div>{label}</div>
        {desc && <div className="field-hint" style={{ fontSize: 11 }}>{desc}</div>}
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
    <details style={{ border: "1px solid var(--border)", borderRadius: 4 }}>
      <summary
        style={{
          padding: "6px 10px",
          cursor: "pointer",
          listStyle: "none",
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          fontSize: 12,
        }}
      >
        <span>{label}</span>
        <span style={{ display: "flex", alignItems: "center", gap: 6, opacity: 0.7 }}>
          <span>{summary}</span>
          <ChevronDown size={12} />
        </span>
      </summary>
      <div
        style={{
          padding: "6px 10px 8px",
          borderTop: "1px solid var(--border)",
          display: "flex",
          flexDirection: "column",
          gap: 4,
          maxHeight: 220,
          overflowY: "auto",
        }}
      >
        {options.length === 0 && (
          <div className="field-hint" style={{ fontSize: 11 }}>—</div>
        )}
        {options.map((opt) => (
          <label
            key={opt.id}
            style={{ display: "flex", gap: 6, alignItems: "center", cursor: "pointer" }}
          >
            <input
              type="checkbox"
              checked={selected.has(opt.id)}
              onChange={() => toggle(opt.id)}
            />
            <span style={{ fontSize: 12 }}>{opt.label}</span>
          </label>
        ))}
        {selected.size > 0 && (
          <button
            type="button"
            className="btn"
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
