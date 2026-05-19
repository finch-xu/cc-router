import { useEffect, useMemo, useRef, useState } from "react";
import { CheckCircle2, ExternalLink, X } from "lucide-react";
import { open as openShell } from "@tauri-apps/plugin-shell";
import { downloadDir } from "@tauri-apps/api/path";
import { useT } from "@/i18n";
import { ReceiptSlip, type ReceiptDisplayOptions } from "@/components/receipts/ReceiptSlip";
import { ReceiptControls } from "@/components/receipts/ReceiptControls";
import { useReceipt } from "@/hooks/useReceipts";
import { exportPng, exportPdf, exportHtml } from "@/utils/exportReceipt";
import { ZERO_TOTALS, addTotals } from "@/lib/receipt-aggregations";
import type {
  ReceiptDto,
  ReceiptRange,
  ReceiptTotalsDto,
  ReceiptVirtualModelItemDto,
} from "@/types";

/** 空集合 = 全选(不过滤);否则 sub_items 按交集过滤,subtotal/grand_total 重算。 */
function applyFilters(
  dto: ReceiptDto,
  subFilter: Set<string>,
  providerFilter: Set<string>,
  excludeDeleted: boolean,
): ReceiptDto {
  const noSubFilter = subFilter.size === 0;
  const noProvFilter = providerFilter.size === 0;
  if (noSubFilter && noProvFilter && !excludeDeleted) return dto;

  let grandTotal: ReceiptTotalsDto = { ...ZERO_TOTALS };
  const items: ReceiptVirtualModelItemDto[] = dto.items.map((item) => {
    const sub_items = item.sub_items.filter(
      (s) =>
        (noSubFilter || subFilter.has(s.subscription_id)) &&
        (noProvFilter || providerFilter.has(s.provider_id)) &&
        (!excludeDeleted || s.subscription_display_name != null),
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
    colorMode: "color",
    showProviderLogo: true,
    compactTokens: true,
    groupMode: "virtual_model",
  });
  const [selectedSubs, setSelectedSubs] = useState<Set<string>>(new Set());
  const [selectedProviders, setSelectedProviders] = useState<Set<string>>(new Set());
  const [excludeDeleted, setExcludeDeleted] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [flash, setFlash] = useState<string | null>(null);
  const flashTimerRef = useRef<number | null>(null);

  const slipRef = useRef<HTMLDivElement>(null);
  const receipt = useReceipt(range);

  useEffect(
    () => () => {
      if (flashTimerRef.current !== null) window.clearTimeout(flashTimerRef.current);
    },
    [],
  );

  const showFlash = (msg: string) => {
    setFlash(msg);
    if (flashTimerRef.current !== null) window.clearTimeout(flashTimerRef.current);
    flashTimerRef.current = window.setTimeout(() => setFlash(null), 4500);
  };

  const openDownloadsFolder = async () => {
    try {
      const dir = await downloadDir();
      await openShell(dir);
    } catch (err) {
      console.warn("open downloads folder failed", err);
    }
  };

  const filteredDto = useMemo(() => {
    if (!receipt.data) return null;
    return applyFilters(receipt.data, selectedSubs, selectedProviders, excludeDeleted);
  }, [receipt.data, selectedSubs, selectedProviders, excludeDeleted]);

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
      showFlash(t("receipts.savedToDownloads"));
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
            excludeDeleted={excludeDeleted}
            onExcludeDeletedChange={setExcludeDeleted}
            isFetching={receipt.isFetching}
            onRefresh={() => receipt.refetch()}
            onExport={runExport}
            exportDisabled={exportDisabled}
            exporting={exporting}
          />
        </div>
      </div>

      {flash && (
        <div
          role="status"
          style={{
            position: "fixed",
            top: 20,
            right: 20,
            zIndex: 999,
            display: "flex",
            alignItems: "center",
            gap: 10,
            padding: "10px 12px 10px 14px",
            background: "var(--ok)",
            border: "1px solid var(--ok)",
            borderRadius: 6,
            boxShadow: "0 6px 20px rgba(0, 0, 0, 0.2)",
            fontSize: 13,
            maxWidth: 380,
            color: "white",
          }}
        >
          <CheckCircle2 size={15} style={{ flexShrink: 0 }} />
          <span style={{ fontWeight: 500 }}>{flash}</span>
          <button
            type="button"
            className="btn"
            onClick={() => void openDownloadsFolder()}
            style={{
              background: "transparent",
              border: "1px solid rgba(255, 255, 255, 0.55)",
              color: "white",
            }}
          >
            <ExternalLink size={12} /> {t("receipts.openDownloads")}
          </button>
          <button
            type="button"
            onClick={() => setFlash(null)}
            aria-label={t("common.close")}
            style={{
              background: "none",
              border: "none",
              cursor: "pointer",
              padding: 4,
              lineHeight: 0,
              color: "white",
              opacity: 0.85,
            }}
          >
            <X size={14} />
          </button>
        </div>
      )}
    </>
  );
}
