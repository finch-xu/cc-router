import { forwardRef, useMemo } from "react";
import { QRCodeSVG } from "qrcode.react";
import { useT } from "@/i18n";
import { fmtNum, fmtCompact } from "@/lib/format";
import { ProviderIcon } from "@/components/ProviderIcon";
import {
  groupBySubscription,
  totalsBySubscription,
  totalsByRealModel,
  totalTokens,
  type SubscriptionGroup,
  type TotalsRow,
} from "@/lib/receipt-aggregations";
import type {
  ReceiptDto,
  ReceiptSubItemDto,
  ReceiptTotalsDto,
  ReceiptVirtualModelItemDto,
} from "@/types";
import { version as VERSION } from "../../../package.json";
import logoUrl from "@/assets/logo.png";

const SITE_URL = "https://ccrouter.app";
const SITE_LABEL = "ccrouter.app";
const REPO_LABEL = "github.com/finch-xu/cc-router";

export type ReceiptColorMode = "mono" | "color";

/**
 * 小票主体的聚合视图模式:
 * - virtual_model: 现状,按 4 个虚拟模型分组 (opus/sonnet/haiku/fallback)
 * - subscription:  按订阅分组,订阅下展开各真实模型用量
 * - totals_only:   上下两段并列,「按订阅总量」+「按真实模型总量」, 无嵌套
 */
export type ReceiptGroupMode = "virtual_model" | "subscription" | "totals_only";

export interface ReceiptDisplayOptions {
  /** 显示 cache_creation / cache_read 两行;关掉只展示 in/out */
  showCacheTokens: boolean;
  /** 在每行子项展示请求次数;关掉只展示 token */
  showRequestCounts: boolean;
  /** 默认 mono 黑白(打印小票感)/ color 米色纸彩色 */
  colorMode: ReceiptColorMode;
  /** 子项行展示 provider 品牌 logo (来自 lobehub/icons), 默认关 */
  showProviderLogo: boolean;
  /** 把 token 数字压缩成 K/M 紧凑形式 (2 位小数), 默认开启; 仅作用于 token, 不影响请求次数 */
  compactTokens: boolean;
  /** 小票主体聚合视图模式; 默认 virtual_model 保持原行为 */
  groupMode: ReceiptGroupMode;
}

interface Props {
  dto: ReceiptDto;
  options: ReceiptDisplayOptions;
}

interface Palette {
  bg: string;
  fg: string;
  /** 次要文字颜色 (元信息、subtitle) */
  muted: string;
  /** 虚线分隔色 */
  dashed: string;
  /** 双线总计分隔色 */
  double: string;
  border: string;
  /** QR / 强调色 */
  accent: string;
  /** logo 是否走 grayscale filter (mono 模式让原色 logo 也变灰阶) */
  logoFilter: string;
}

const PALETTE: Record<ReceiptColorMode, Palette> = {
  mono: {
    bg: "#ffffff",
    fg: "#111111",
    muted: "#666666",
    dashed: "#bbbbbb",
    double: "#444444",
    border: "#dddddd",
    accent: "#111111",
    logoFilter: "grayscale(1) contrast(1.05)",
  },
  color: {
    bg: "#faf7f0",
    fg: "#222222",
    muted: "rgba(34,34,34,0.7)",
    dashed: "#b8ad94",
    double: "#8a7f63",
    border: "#e5dfd0",
    accent: "#3a2f1c",
    logoFilter: "none",
  },
};

function formatPeriod(startMs: number, endMs: number): string {
  const fmt = (ms: number) => {
    const d = new Date(ms);
    const Y = d.getUTCFullYear();
    const M = String(d.getUTCMonth() + 1).padStart(2, "0");
    const D = String(d.getUTCDate()).padStart(2, "0");
    return `${Y}-${M}-${D}`;
  };
  return `${fmt(startMs)} → ${fmt(endMs)}`;
}

function formatIssued(ms: number): string {
  const d = new Date(ms);
  const Y = d.getUTCFullYear();
  const M = String(d.getUTCMonth() + 1).padStart(2, "0");
  const D = String(d.getUTCDate()).padStart(2, "0");
  const h = String(d.getUTCHours()).padStart(2, "0");
  const m = String(d.getUTCMinutes()).padStart(2, "0");
  return `${Y}-${M}-${D} ${h}:${m} UTC`;
}

const VM_DISPLAY: Record<string, string> = {
  "model-fable": "MODEL-FABLE",
  "model-opus": "MODEL-OPUS",
  "model-sonnet": "MODEL-SONNET",
  "model-haiku": "MODEL-HAIKU",
};

export const ReceiptSlip = forwardRef<HTMLDivElement, Props>(function ReceiptSlip(
  { dto, options },
  ref,
) {
  const { t } = useT();
  const palette = PALETTE[options.colorMode];

  const periodLabel = formatPeriod(dto.range_start_ms, dto.range_end_ms);
  const issuedLabel = formatIssued(dto.generated_at_ms);
  const grandTokens = totalTokens(dto.grand_total);
  const fmtToken = options.compactTokens ? fmtCompact : fmtNum;

  return (
    <div
      ref={ref}
      style={{
        width: 360,
        padding: "20px 18px 24px",
        background: palette.bg,
        color: palette.fg,
        fontFamily:
          '"SF Mono", Menlo, Monaco, Consolas, "Courier New", monospace',
        fontSize: 12,
        lineHeight: 1.55,
        fontVariantNumeric: "tabular-nums",
        boxShadow:
          options.colorMode === "color"
            ? "0 1px 2px rgba(0,0,0,0.04), 0 8px 24px -8px rgba(0,0,0,0.08)"
            : "0 0 0 1px " + palette.border,
        border: "1px solid " + palette.border,
        borderRadius: 4,
      }}
    >
      {/* Header — logo + 标题 */}
      <div style={{ textAlign: "center", marginBottom: 6 }}>
        <div style={{ display: "flex", justifyContent: "center", marginBottom: 6 }}>
          <img
            src={logoUrl}
            alt="cc-router"
            width={40}
            height={40}
            style={{
              width: 40,
              height: 40,
              filter: palette.logoFilter,
            }}
          />
        </div>
        <div style={{ fontWeight: 700, fontSize: 16, letterSpacing: 2 }}>
          CC-ROUTER
        </div>
        <div style={{ fontWeight: 700, fontSize: 11, letterSpacing: 4, color: palette.muted }}>
          {t("receipts.slip.header")}
        </div>
        <div style={{ fontSize: 10, color: palette.muted, marginTop: 2 }}>
          {t("receipts.slip.tagline")}
        </div>
      </div>

      <Divider palette={palette} />

      <Row labelMuted label={t("receipts.slip.period")} value={periodLabel} palette={palette} />
      <Row labelMuted label={t("receipts.slip.issued")} value={issuedLabel} palette={palette} />
      <Row
        labelMuted
        label={t("receipts.slip.slipNo")}
        value={`RCPT-${dto.slip_no}`}
        palette={palette}
      />

      <Divider palette={palette} />

      {/* Items: 按 groupMode 选择聚合视图 */}
      {options.groupMode === "virtual_model" &&
        dto.items.map((item, idx) => (
          <VmItemBlock
            key={item.virtual_model_name}
            item={item}
            options={options}
            palette={palette}
            isLast={idx === dto.items.length - 1}
          />
        ))}
      {options.groupMode === "subscription" && (
        <SubscriptionView dto={dto} options={options} palette={palette} />
      )}
      {options.groupMode === "totals_only" && (
        <TotalsView dto={dto} options={options} palette={palette} />
      )}

      {/* Grand total */}
      <DoubleDivider palette={palette} />
      <Row
        bold
        big
        label={t("receipts.slip.totalRequests")}
        value={fmtNum(dto.grand_total.request_count)}
        palette={palette}
      />
      <Row
        bold
        big
        label={t("receipts.slip.totalTokens")}
        value={fmtToken(grandTokens)}
        palette={palette}
      />
      <Row
        indent
        label={`├ ${t("receipts.slip.input")}`}
        value={fmtToken(dto.grand_total.input_tokens)}
        palette={palette}
      />
      <Row
        indent
        label={`├ ${t("receipts.slip.output")}`}
        value={fmtToken(dto.grand_total.output_tokens)}
        palette={palette}
      />
      {options.showCacheTokens && (
        <>
          <Row
            indent
            label={`├ ${t("receipts.slip.cacheCreate")}`}
            value={fmtToken(dto.grand_total.cache_creation_tokens)}
            palette={palette}
          />
          <Row
            indent
            label={`└ ${t("receipts.slip.cacheRead")}`}
            value={fmtToken(dto.grand_total.cache_read_tokens)}
            palette={palette}
          />
        </>
      )}

      <Divider palette={palette} />

      {/* Footer */}
      <div style={{ textAlign: "center", fontSize: 10, color: palette.muted, marginTop: 4 }}>
        {t("receipts.slip.thanks")}
      </div>

      {/* QR — 扫码进官网 */}
      <div style={{ display: "flex", justifyContent: "center", marginTop: 10 }}>
        <div
          style={{
            background: palette.bg,
            padding: 4,
            border: "1px solid " + palette.border,
            borderRadius: 2,
          }}
        >
          <QRCodeSVG
            value={SITE_URL}
            size={68}
            bgColor={palette.bg}
            fgColor={palette.accent}
            level="M"
            marginSize={0}
          />
        </div>
      </div>

      <div
        style={{
          textAlign: "center",
          fontSize: 9,
          color: palette.muted,
          marginTop: 8,
          letterSpacing: 0.5,
        }}
      >
        {SITE_LABEL}
      </div>
      <div
        style={{
          textAlign: "center",
          fontSize: 8,
          color: palette.muted,
          marginTop: 2,
          opacity: 0.6,
          letterSpacing: 0.5,
        }}
      >
        {REPO_LABEL}
      </div>
      <div
        style={{
          textAlign: "center",
          fontSize: 9,
          color: palette.muted,
          marginTop: 2,
          opacity: 0.7,
          letterSpacing: 1,
        }}
      >
        v{VERSION}
      </div>
    </div>
  );
});

function VmItemBlock({
  item,
  options,
  palette,
  isLast,
}: {
  item: ReceiptVirtualModelItemDto;
  options: ReceiptDisplayOptions;
  palette: Palette;
  isLast: boolean;
}) {
  const { t } = useT();
  const subtotalTokens = totalTokens(item.subtotal);
  const display = VM_DISPLAY[item.virtual_model_name] ?? item.virtual_model_name.toUpperCase();
  const isEmpty = item.sub_items.length === 0;
  const fmtToken = options.compactTokens ? fmtCompact : fmtNum;

  return (
    <div style={{ marginTop: 4 }}>
      <Row
        bold
        label={`▸ ${display}`}
        value={
          options.showRequestCounts
            ? `${fmtNum(item.subtotal.request_count)}×`
            : fmtToken(subtotalTokens)
        }
        palette={palette}
      />
      {isEmpty ? (
        <div style={{ paddingLeft: 16, fontSize: 10, color: palette.muted, marginTop: 2 }}>
          {t("receipts.slip.empty")}
        </div>
      ) : (
        <>
          {item.sub_items.map((sub, i) => (
            <SubItemRows
              key={`${sub.subscription_id}|${sub.real_model_name}`}
              sub={sub}
              options={options}
              palette={palette}
              isLastSub={i === item.sub_items.length - 1}
            />
          ))}
          <Row
            label={`  ${t("receipts.slip.subtotal")}`}
            value={fmtToken(subtotalTokens)}
            small
            palette={palette}
          />
        </>
      )}
      {!isLast && <Divider compact palette={palette} />}
    </div>
  );
}

function SubItemRows({
  sub,
  options,
  palette,
  isLastSub,
}: {
  sub: ReceiptSubItemDto;
  options: ReceiptDisplayOptions;
  palette: Palette;
  isLastSub: boolean;
}) {
  const { t } = useT();
  const branch = isLastSub ? "└" : "├";
  const subscriptionLabel =
    sub.subscription_display_name ?? t("receipts.slip.deletedSub");
  return (
    <div style={{ marginTop: 3 }}>
      <div style={{ paddingLeft: 8, fontWeight: 600 }}>
        {branch} {sub.real_model_name}
      </div>
      <div
        style={{
          paddingLeft: 16,
          fontSize: 10,
          color: palette.muted,
          display: "flex",
          alignItems: "center",
          gap: 4,
          flexWrap: "wrap",
        }}
      >
        {options.showProviderLogo && (
          <span style={{ filter: palette.logoFilter, color: palette.fg }} aria-hidden>
            <ProviderIcon
              iconId={sub.provider_id}
              size={12}
              monochrome={options.colorMode === "mono"}
            />
          </span>
        )}
        <span>
          {sub.provider_display_name} / {subscriptionLabel}
        </span>
        {options.showRequestCounts && (
          <span style={{ marginLeft: 2 }}>
            · {fmtNum(sub.totals.request_count)}×
          </span>
        )}
      </div>
      <TokenLine
        label="in"
        value={sub.totals.input_tokens}
        label2="out"
        value2={sub.totals.output_tokens}
        palette={palette}
        compact={options.compactTokens}
      />
      {options.showCacheTokens &&
        (sub.totals.cache_creation_tokens > 0 || sub.totals.cache_read_tokens > 0) && (
          <TokenLine
            label="c+"
            value={sub.totals.cache_creation_tokens}
            label2="c-"
            value2={sub.totals.cache_read_tokens}
            palette={palette}
            compact={options.compactTokens}
          />
        )}
    </div>
  );
}

function TokenLine({
  label,
  value,
  label2,
  value2,
  palette,
  compact,
}: {
  label: string;
  value: number;
  label2: string;
  value2: number;
  palette: Palette;
  compact: boolean;
}) {
  const fmtToken = compact ? fmtCompact : fmtNum;
  return (
    <div
      style={{
        paddingLeft: 16,
        display: "grid",
        gridTemplateColumns: "auto 1fr auto 1fr",
        columnGap: 6,
        fontSize: 11,
      }}
    >
      <span style={{ color: palette.muted }}>{label}</span>
      <span style={{ textAlign: "right" }}>{fmtToken(value)}</span>
      <span style={{ color: palette.muted, paddingLeft: 6 }}>{label2}</span>
      <span style={{ textAlign: "right" }}>{fmtToken(value2)}</span>
    </div>
  );
}

function Row({
  label,
  value,
  bold,
  big,
  small,
  indent,
  labelMuted,
  palette,
}: {
  label: string;
  value: string;
  bold?: boolean;
  big?: boolean;
  small?: boolean;
  indent?: boolean;
  /** label 走 muted 颜色, value 仍是 fg — 用于 meta 行 (Period / Issued / Slip No) */
  labelMuted?: boolean;
  palette: Palette;
}) {
  const baseFontSize = big ? 13 : small ? 10 : labelMuted ? 11 : 12;
  return (
    <div
      style={{
        display: "flex",
        justifyContent: "space-between",
        fontWeight: bold ? 700 : 400,
        fontSize: baseFontSize,
        color: small ? palette.muted : palette.fg,
        paddingLeft: indent ? 8 : 0,
        marginTop: big ? 2 : 0,
      }}
    >
      <span style={labelMuted ? { color: palette.muted } : undefined}>{label}</span>
      <span>{value}</span>
    </div>
  );
}

function Divider({ compact, palette }: { compact?: boolean; palette: Palette }) {
  return (
    <div
      style={{
        borderTop: "1px dashed " + palette.dashed,
        marginTop: compact ? 6 : 8,
        marginBottom: compact ? 6 : 8,
      }}
    />
  );
}

function DoubleDivider({ palette }: { palette: Palette }) {
  return (
    <div
      style={{
        borderTop: "2px double " + palette.double,
        marginTop: 10,
        marginBottom: 8,
      }}
    />
  );
}

// =============================================================================
// 视图 2: 按订阅分组 (subscription mode)
// =============================================================================

function SubscriptionView({
  dto,
  options,
  palette,
}: {
  dto: ReceiptDto;
  options: ReceiptDisplayOptions;
  palette: Palette;
}) {
  const { t } = useT();
  const groups = useMemo(() => groupBySubscription(dto), [dto]);
  if (groups.length === 0) {
    return (
      <div style={{ paddingLeft: 8, fontSize: 11, color: palette.muted }}>
        {t("receipts.slip.empty")}
      </div>
    );
  }
  return (
    <>
      {groups.map((g, idx) => (
        <SubscriptionBlock
          key={g.subscription_id}
          group={g}
          options={options}
          palette={palette}
          isLast={idx === groups.length - 1}
        />
      ))}
    </>
  );
}

function SubscriptionBlock({
  group,
  options,
  palette,
  isLast,
}: {
  group: SubscriptionGroup;
  options: ReceiptDisplayOptions;
  palette: Palette;
  isLast: boolean;
}) {
  const { t } = useT();
  const subtotalTokens = totalTokens(group.subtotal);
  const fmtToken = options.compactTokens ? fmtCompact : fmtNum;
  const subscriptionLabel =
    group.subscription_display_name ?? t("receipts.slip.deletedSub");

  return (
    <div style={{ marginTop: 4 }}>
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          fontWeight: 700,
          fontSize: 12,
          gap: 6,
        }}
      >
        <span
          style={{
            display: "flex",
            alignItems: "center",
            gap: 4,
            minWidth: 0,
            overflow: "hidden",
          }}
        >
          {options.showProviderLogo && (
            <span style={{ filter: palette.logoFilter, color: palette.fg }} aria-hidden>
              <ProviderIcon
                iconId={group.provider_id}
                size={12}
                monochrome={options.colorMode === "mono"}
              />
            </span>
          )}
          <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            ▸ {subscriptionLabel}
          </span>
        </span>
        <span>
          {options.showRequestCounts
            ? `${fmtNum(group.subtotal.request_count)}×`
            : fmtToken(subtotalTokens)}
        </span>
      </div>
      <div
        style={{
          paddingLeft: 16,
          fontSize: 10,
          color: palette.muted,
          marginTop: 2,
        }}
      >
        {group.provider_display_name}
      </div>
      {group.models.map((m, i) => (
        <ModelLineRow
          key={m.real_model_name}
          name={m.real_model_name}
          totals={m.totals}
          isLast={i === group.models.length - 1}
          options={options}
          palette={palette}
        />
      ))}
      <Row
        label={`  ${t("receipts.slip.subtotal")}`}
        value={fmtToken(subtotalTokens)}
        small
        palette={palette}
      />
      {!isLast && <Divider compact palette={palette} />}
    </div>
  );
}

function ModelLineRow({
  name,
  totals,
  isLast,
  options,
  palette,
}: {
  name: string;
  totals: ReceiptTotalsDto;
  isLast: boolean;
  options: ReceiptDisplayOptions;
  palette: Palette;
}) {
  const branch = isLast ? "└" : "├";
  return (
    <div style={{ marginTop: 3 }}>
      <div style={{ paddingLeft: 8, fontWeight: 600 }}>
        {branch} {name}
      </div>
      {options.showRequestCounts && (
        <div
          style={{
            paddingLeft: 16,
            fontSize: 10,
            color: palette.muted,
          }}
        >
          · {fmtNum(totals.request_count)}×
        </div>
      )}
      <TokenLine
        label="in"
        value={totals.input_tokens}
        label2="out"
        value2={totals.output_tokens}
        palette={palette}
        compact={options.compactTokens}
      />
      {options.showCacheTokens &&
        (totals.cache_creation_tokens > 0 || totals.cache_read_tokens > 0) && (
          <TokenLine
            label="c+"
            value={totals.cache_creation_tokens}
            label2="c-"
            value2={totals.cache_read_tokens}
            palette={palette}
            compact={options.compactTokens}
          />
        )}
    </div>
  );
}

// =============================================================================
// 视图 3: 仅汇总 (totals_only mode)
// =============================================================================

function TotalsView({
  dto,
  options,
  palette,
}: {
  dto: ReceiptDto;
  options: ReceiptDisplayOptions;
  palette: Palette;
}) {
  const { t } = useT();
  const subRows = useMemo(() => totalsBySubscription(dto), [dto]);
  const modelRows = useMemo(() => totalsByRealModel(dto), [dto]);

  return (
    <>
      <TotalsSection
        title={t("receipts.slip.section.bySubscription")}
        rows={subRows}
        showLogo={options.showProviderLogo}
        options={options}
        palette={palette}
      />
      <Divider compact palette={palette} />
      <TotalsSection
        title={t("receipts.slip.section.byRealModel")}
        rows={modelRows}
        showLogo={false}
        options={options}
        palette={palette}
      />
    </>
  );
}

function TotalsSection({
  title,
  rows,
  showLogo,
  options,
  palette,
}: {
  title: string;
  rows: TotalsRow[];
  /** model section 永远 false; subscription section 跟随 options.showProviderLogo */
  showLogo: boolean;
  options: ReceiptDisplayOptions;
  palette: Palette;
}) {
  const { t } = useT();
  const fmtToken = options.compactTokens ? fmtCompact : fmtNum;
  const sectionTotal = rows.reduce((acc, r) => acc + totalTokens(r.totals), 0);
  return (
    <div style={{ marginTop: 4 }}>
      <Row bold label={`▸ ${title}`} value={fmtToken(sectionTotal)} palette={palette} />
      {rows.length === 0 ? (
        <div
          style={{
            paddingLeft: 16,
            fontSize: 10,
            color: palette.muted,
            marginTop: 2,
          }}
        >
          {t("receipts.slip.empty")}
        </div>
      ) : (
        rows.map((r) => (
          <div
            key={r.key}
            style={{
              display: "flex",
              justifyContent: "space-between",
              alignItems: "center",
              paddingLeft: 8,
              marginTop: 2,
              fontSize: 11,
              gap: 6,
            }}
          >
            <span
              style={{
                display: "flex",
                alignItems: "center",
                gap: 4,
                minWidth: 0,
                overflow: "hidden",
              }}
            >
              {showLogo && r.provider_id && (
                <span
                  style={{ filter: palette.logoFilter, color: palette.fg }}
                  aria-hidden
                >
                  <ProviderIcon
                    iconId={r.provider_id}
                    size={11}
                    monochrome={options.colorMode === "mono"}
                  />
                </span>
              )}
              <span
                style={{
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  whiteSpace: "nowrap",
                }}
              >
                {r.display ?? t("receipts.slip.deletedSub")}
              </span>
              {options.showRequestCounts && (
                <span style={{ color: palette.muted, fontSize: 10, flexShrink: 0 }}>
                  · {fmtNum(r.totals.request_count)}×
                </span>
              )}
            </span>
            <span style={{ flexShrink: 0 }}>{fmtToken(totalTokens(r.totals))}</span>
          </div>
        ))
      )}
    </div>
  );
}
