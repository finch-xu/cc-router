import { useEffect, useMemo, useRef, useState } from "react";
import { CircleCheck, ExternalLink, X } from "lucide-react";
import { runtime } from "@/runtime";
import { useT } from "@/i18n";
import { useSearchParams } from "react-router";
import { ReceiptSlip, type ReceiptDisplayOptions, type ReceiptTheme } from "@/components/receipts/ReceiptSlip";
import { ReceiptControls, RECEIPT_RANGES } from "@/components/receipts/ReceiptControls";
import {
  ReceiptPrintOverlay,
  type PrintExportStatus,
} from "@/components/receipts/ReceiptPrintOverlay";
import { useReceipt } from "@/hooks/useReceipts";
import { useSettings, useUpdateSettings } from "@/hooks/useSettings";
import { exportPng, exportPdf, exportHtml } from "@/utils/exportReceipt";
import { ZERO_TOTALS, addTotals } from "@/lib/receipt-aggregations";
import { customProviderLabel } from "@/lib/providerLabels";
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

/**
 * 订阅被删除后, 后端 COALESCE 兜底会把 provider_display_name 落成裸 provider_id;
 * 自定义订阅此时会显示 "custom-openai" 这类内部 id —— 统一替换成 i18n 友好名。
 * 仅当 display_name 与 id 相同 (即兜底路径) 时替换, 不动用户自起的名字。
 */
function localizeCustomProviders(dto: ReceiptDto, t: (key: string) => string): ReceiptDto {
  return {
    ...dto,
    items: dto.items.map((item) => ({
      ...item,
      sub_items: item.sub_items.map((s) => {
        if (s.provider_display_name !== s.provider_id) return s;
        const label = customProviderLabel(s.provider_id, t);
        return label ? { ...s, provider_display_name: label } : s;
      }),
    })),
  };
}

export function ReceiptsPage() {
  const { t } = useT();

  const [range, setRange] = useState<ReceiptRange>("all_time");
  // 插件页「去用这张小票」带 ?theme=plugin:<id> 过来
  const [searchParams] = useSearchParams();
  const themeParam = searchParams.get("theme");
  const [options, setOptions] = useState<ReceiptDisplayOptions>({
    showCacheTokens: true,
    showRequestCounts: true,
    theme: themeParam && /^plugin:[a-z0-9][a-z0-9-]{1,39}$/.test(themeParam) ? (themeParam as ReceiptTheme) : "color",
    showProviderLogo: true,
    compactTokens: true,
    groupMode: "virtual_model",
    // 默认条形码: 二维码分享到部分平台会被检测并屏蔽
    footerCodeStyle: "barcode",
  });
  const [selectedSubs, setSelectedSubs] = useState<Set<string>>(new Set());
  const [selectedProviders, setSelectedProviders] = useState<Set<string>>(new Set());
  const [excludeDeleted, setExcludeDeleted] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [flash, setFlash] = useState<string | null>(null);
  const flashTimerRef = useRef<number | null>(null);
  // 出票动画: 导出时叠在页面上; 关掉动画不影响导出, 届时改回右上角提示
  const [printJob, setPrintJob] = useState<{
    kind: "png" | "pdf" | "html";
    status: PrintExportStatus;
  } | null>(null);
  const printOpenRef = useRef(false);
  const settings = useSettings();
  const updateSettings = useUpdateSettings();
  // 保存在途时先显示用户刚点的值, 不等设置重新拉取
  const printAnimation = updateSettings.isPending
    ? (updateSettings.variables?.receipt_print_animation ?? true)
    : (settings.data?.receipt_print_animation ?? true);

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
      await runtime.openDownloadsDir();
    } catch (err) {
      console.warn("open downloads folder failed", err);
    }
  };

  const closePrint = () => {
    printOpenRef.current = false;
    setPrintJob(null);
  };

  const filteredDto = useMemo(() => {
    if (!receipt.data) return null;
    return localizeCustomProviders(
      applyFilters(receipt.data, selectedSubs, selectedProviders, excludeDeleted),
      t,
    );
  }, [receipt.data, selectedSubs, selectedProviders, excludeDeleted, t]);

  const exportDisabled = !filteredDto;

  const runExport = async (kind: "png" | "pdf" | "html") => {
    const el = slipRef.current;
    if (!el || !filteredDto) return;
    const { slip_no: slip, range: r } = filteredDto;
    setExporting(true);
    if (printAnimation) {
      printOpenRef.current = true;
      setPrintJob({ kind, status: "pending" });
    }
    try {
      if (kind === "png") {
        await exportPng(el, slip, r);
      } else if (kind === "pdf") {
        await exportPdf(el, slip, r);
      } else {
        await exportHtml(el, slip, r, options.theme);
      }
      if (printOpenRef.current) {
        setPrintJob((job) => job && { ...job, status: "ok" });
      } else {
        showFlash(t("receipts.savedToDownloads"));
      }
    } catch (err) {
      console.error("export failed", err);
      closePrint();
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
        {/* 时间范围决定整页数据, 属于页面级控制 —— 放标题行, 不跟小票的渲染选项混在一起 */}
        <div className="range-tabs" style={{ marginBottom: 0, flexShrink: 0 }}>
          {RECEIPT_RANGES.map((r) => (
            <button
              key={r.key}
              type="button"
              className={"range-tab" + (range === r.key ? " active" : "")}
              onClick={() => setRange(r.key)}
            >
              {t(r.labelKey)}
            </button>
          ))}
        </div>
      </div>

      <div className="receipt-layout">
        {/* 左 — 小票 (固定 360px, 就是小票的物理宽度) */}
        <div className="receipt-slip-col">
          {filteredDto ? (
            <ReceiptSlip ref={slipRef} dto={filteredDto} options={options} />
          ) : (
            <div className="receipt-empty">
              {receipt.isLoading ? t("common.loading") : t("receipts.empty")}
            </div>
          )}
          {receipt.isError && (
            <div className="alert err" style={{ maxWidth: 360 }}>
              {t("receipts.loadError")}
            </div>
          )}
        </div>

        {/* 右 — 控制台。根节点自带 .receipt-panel (flex:1), 直接吃满剩余宽度;
            原先外面套一层 maxWidth:460 的 div, 右边会白留 200px 空档 */}
        <ReceiptControls
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
          printAnimation={printAnimation}
          onPrintAnimationChange={(v) => updateSettings.mutate({ receipt_print_animation: v })}
        />
      </div>

      {filteredDto && (
        <ReceiptPrintOverlay
          open={printJob !== null}
          dto={filteredDto}
          options={options}
          kind={printJob?.kind ?? "png"}
          exportStatus={printJob?.status ?? "pending"}
          onClose={closePrint}
        />
      )}

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
          <CircleCheck size={15} style={{ flexShrink: 0 }} />
          <span style={{ fontWeight: 500 }}>{flash}</span>
          {runtime.kind === "desktop" && (
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
          )}
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
