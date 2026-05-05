import { useMemo, useRef, useState } from "react";
import { useT } from "@/i18n";
import { ReceiptSlip, type ReceiptDisplayOptions } from "@/components/receipts/ReceiptSlip";
import { ReceiptControls } from "@/components/receipts/ReceiptControls";
import { useReceipt } from "@/hooks/useReceipts";
import { exportPng, exportPdf, exportHtml } from "@/utils/exportReceipt";
import type {
  ReceiptDto,
  ReceiptRange,
  ReceiptTotalsDto,
  ReceiptVirtualModelItemDto,
} from "@/types";

const ZERO_TOTALS: ReceiptTotalsDto = {
  request_count: 0,
  input_tokens: 0,
  output_tokens: 0,
  cache_creation_tokens: 0,
  cache_read_tokens: 0,
};

function addTotals(a: ReceiptTotalsDto, b: ReceiptTotalsDto): ReceiptTotalsDto {
  return {
    request_count: a.request_count + b.request_count,
    input_tokens: a.input_tokens + b.input_tokens,
    output_tokens: a.output_tokens + b.output_tokens,
    cache_creation_tokens: a.cache_creation_tokens + b.cache_creation_tokens,
    cache_read_tokens: a.cache_read_tokens + b.cache_read_tokens,
  };
}

/** 空集合 = 全选(不过滤);否则 sub_items 按交集过滤,subtotal/grand_total 重算。 */
function applyFilters(
  dto: ReceiptDto,
  subFilter: Set<string>,
  providerFilter: Set<string>,
): ReceiptDto {
  const noSubFilter = subFilter.size === 0;
  const noProvFilter = providerFilter.size === 0;
  if (noSubFilter && noProvFilter) return dto;

  let grandTotal: ReceiptTotalsDto = { ...ZERO_TOTALS };
  const items: ReceiptVirtualModelItemDto[] = dto.items.map((item) => {
    const sub_items = item.sub_items.filter(
      (s) =>
        (noSubFilter || subFilter.has(s.subscription_id)) &&
        (noProvFilter || providerFilter.has(s.provider_id)),
    );
    const subtotal = sub_items.reduce((acc, s) => addTotals(acc, s.totals), { ...ZERO_TOTALS });
    grandTotal = addTotals(grandTotal, subtotal);
    return {
      virtual_model_name: item.virtual_model_name,
      subtotal,
      sub_items,
    };
  });

  return {
    ...dto,
    items,
    grand_total: grandTotal,
  };
}

export function ReceiptsPage() {
  const { t } = useT();

  const [range, setRange] = useState<ReceiptRange>("last7_days");
  const [options, setOptions] = useState<ReceiptDisplayOptions>({
    showCacheTokens: true,
    showRequestCounts: true,
    colorMode: "mono",
    showProviderLogo: false,
  });
  const [selectedSubs, setSelectedSubs] = useState<Set<string>>(new Set());
  const [selectedProviders, setSelectedProviders] = useState<Set<string>>(new Set());
  const [exporting, setExporting] = useState(false);

  const slipRef = useRef<HTMLDivElement>(null);
  const receipt = useReceipt(range);

  const filteredDto = useMemo(() => {
    if (!receipt.data) return null;
    return applyFilters(receipt.data, selectedSubs, selectedProviders);
  }, [receipt.data, selectedSubs, selectedProviders]);

  const exportDisabled = !filteredDto;

  const runExport = async (kind: "png" | "pdf" | "html") => {
    const el = slipRef.current;
    if (!el || !filteredDto) return;
    const { slip_no: slip, range: r } = filteredDto;
    setExporting(true);
    try {
      if (kind === "png") {
        await exportPng(el, slip, r);
      } else if (kind === "pdf") {
        await exportPdf(el, slip, r);
      } else {
        exportHtml(el, slip, r);
      }
    } catch (err) {
      console.error("export failed", err);
      alert(t("receipts.exportFailed"));
    } finally {
      setExporting(false);
    }
  };

  return (
    <>
      <div className="page-actions">
        <div className="page-header" style={{ margin: 0 }}>
          <h1>{t("receipts.title")}</h1>
          <div className="subtitle">{t("receipts.subtitle")}</div>
        </div>
      </div>

      <div
        style={{
          display: "flex",
          gap: 24,
          alignItems: "flex-start",
          flexWrap: "wrap",
        }}
      >
        {/* 左 — 小票 */}
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            alignItems: "center",
            gap: 8,
          }}
        >
          {filteredDto ? (
            <ReceiptSlip ref={slipRef} dto={filteredDto} options={options} />
          ) : (
            <div
              style={{
                width: 360,
                padding: 40,
                textAlign: "center",
                color: "var(--muted)",
                border: "1px dashed var(--border)",
                borderRadius: 4,
              }}
            >
              {receipt.isLoading ? t("common.loading") : t("receipts.empty")}
            </div>
          )}
          {receipt.isError && (
            <div className="alert alert-error" style={{ maxWidth: 360 }}>
              {t("receipts.loadError")}
            </div>
          )}
        </div>

        {/* 右 — 控制台 */}
        <div style={{ flex: "1 1 320px", maxWidth: 460, minWidth: 280 }}>
          <ReceiptControls
            range={range}
            onRangeChange={setRange}
            options={options}
            onOptionsChange={setOptions}
            selectedSubscriptionIds={selectedSubs}
            onSelectedSubscriptionsChange={setSelectedSubs}
            selectedProviderIds={selectedProviders}
            onSelectedProvidersChange={setSelectedProviders}
            isFetching={receipt.isFetching}
            onRefresh={() => receipt.refetch()}
            onExport={runExport}
            exportDisabled={exportDisabled}
            exporting={exporting}
          />
        </div>
      </div>
    </>
  );
}
