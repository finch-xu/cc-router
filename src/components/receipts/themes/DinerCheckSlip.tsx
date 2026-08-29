import type { CSSProperties, ReactNode } from "react";
import type { ReceiptVirtualModelItemDto } from "@/types";
import {
  Between,
  SITE_LABEL,
  VERSION,
  ZigzagPaper,
  fmtCountEn,
  fmtTokEn,
  totalTokensOf,
  utcParts,
  type ThemeSlipProps,
} from "./shared";

const PAPER = "#f3f6e9";
const RED = "#8c2f22";
const PEN = "#26418f";
const RULE = "1px solid #c9cfb2";
const PRINT = "#6b6b5f";
const HAND = "'Caveat', cursive";
const OSWALD = "'Oswald', 'Arial Narrow', sans-serif";

/** F · 点单纸风格: USAGE CHECK 红头 + 绿格纸 + 蓝笔手写; 元信息全是真实字段。 */
export function DinerCheckSlip({ dto, port, sched }: ThemeSlipProps) {
  const issued = utcParts(dto.generated_at_ms);
  const start = utcParts(dto.range_start_ms);
  const end = utcParts(dto.range_end_ms);
  const g = dto.grand_total;

  return (
    <ZigzagPaper
      color={PAPER}
      bodyStyle={{
        color: "#2c2c2c",
        padding: "0 0 20px",
        fontVariantNumeric: "tabular-nums",
        fontFamily: OSWALD,
      }}
    >
      <div style={{ background: RED, color: "#f6efe2", padding: "10px 16px 9px", textAlign: "center" }}>
        <div style={{ fontWeight: 700, fontSize: 19, letterSpacing: 5 }}>USAGE CHECK</div>
        <div style={{ fontSize: 10, letterSpacing: 3, marginTop: 1, opacity: 0.85 }}>
          CC-ROUTER &#183; LOCAL PROXY &#183; ALWAYS ON
        </div>
      </div>

      <div
        style={{
          display: "grid",
          gridTemplateColumns: "1fr 1fr 1fr 1fr",
          borderBottom: `2px solid ${RED}`,
          fontSize: 9,
          letterSpacing: 1,
          textAlign: "center",
          color: PRINT,
        }}
      >
        <MetaCell label="PORT" value={String(port)} border />
        <MetaCell label="SLOTS" value={String(dto.items.length)} border />
        <MetaCell label="SCHED" value={sched} border />
        <MetaCell label="SLIP Nº" value={dto.slip_no} />
      </div>

      <div style={{ padding: "6px 16px 0" }}>
        <Between
          left="QTY&nbsp;&nbsp;ITEM"
          right="AMOUNT"
          style={{ fontSize: 9, letterSpacing: 1, color: PRINT, paddingBottom: 3 }}
        />
        {dto.items.map((item) => (
          <VmRows key={item.virtual_model_name} item={item} />
        ))}
        <div style={{ borderBottom: RULE, height: 22 }} />
        <div style={{ borderBottom: RULE, height: 22 }} />
      </div>

      <div style={{ padding: "10px 16px 0" }}>
        <Between
          left={`SUBTOTAL · ${fmtCountEn(g.request_count)} REQUESTS`}
          right={
            <span style={{ fontFamily: HAND, fontSize: 17, color: PEN }}>
              {fmtTokEn(totalTokensOf(g))}
            </span>
          }
          style={{ fontSize: 11, letterSpacing: 1, color: "#444444", alignItems: "baseline" }}
        />
        <div style={{ fontSize: 10, color: PRINT, marginTop: 2 }}>
          IN {fmtTokEn(g.input_tokens)} &#183; OUT {fmtTokEn(g.output_tokens)}
        </div>
        <div style={{ fontSize: 10, color: PRINT }}>
          CACHE C+ {fmtTokEn(g.cache_creation_tokens)} &#183; C- {fmtTokEn(g.cache_read_tokens)}
        </div>

        <div
          style={{
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
            marginTop: 8,
            borderTop: `2px solid ${RED}`,
            paddingTop: 6,
          }}
        >
          <span style={{ fontWeight: 700, fontSize: 14, letterSpacing: 3 }}>TOTAL</span>
          <span
            style={{
              display: "inline-flex",
              alignItems: "center",
              justifyContent: "center",
              fontFamily: HAND,
              fontSize: 26,
              fontWeight: 700,
              color: PEN,
              border: `2px solid ${PEN}`,
              borderRadius: "50%",
              padding: "2px 14px",
              transform: "rotate(-3deg)",
            }}
          >
            {fmtTokEn(totalTokensOf(g))}
          </span>
        </div>
      </div>

      <div style={{ textAlign: "center", marginTop: 14 }}>
        <div style={{ fontFamily: HAND, fontSize: 21, color: RED, transform: "rotate(-2deg)" }}>
          Thanks for routing with cc-router!
        </div>
        <div style={{ fontSize: 9, letterSpacing: 2, color: PRINT, marginTop: 6 }}>
          {start.M}/{start.D} - {end.M}/{end.D} &#183; ISSUED {issued.M}/{issued.D} {issued.h}:
          {issued.m} &#183; RCPT-{dto.slip_no}
        </div>
        <div style={{ fontSize: 9, letterSpacing: 1, color: PRINT, marginTop: 2 }}>
          {SITE_LABEL} &#183; v{VERSION}
        </div>
      </div>
    </ZigzagPaper>
  );
}

function MetaCell({ label, value, border }: { label: string; value: string; border?: boolean }) {
  return (
    <div style={{ padding: "4px 0", borderRight: border ? RULE : undefined }}>
      {label}
      <br />
      <span style={{ fontFamily: HAND, fontSize: value.length > 7 ? 12 : 16, color: PEN }}>
        {value}
      </span>
    </div>
  );
}

function HandRow({
  left,
  right,
  size = 18,
  color = PEN,
  style,
}: {
  left: ReactNode;
  right: ReactNode;
  size?: number;
  color?: string;
  style?: CSSProperties;
}) {
  return (
    <div
      style={{
        display: "flex",
        justifyContent: "space-between",
        alignItems: "baseline",
        gap: 6,
        borderBottom: RULE,
        padding: "3px 0 1px",
        fontFamily: HAND,
        fontSize: size,
        color,
        minWidth: 0,
        ...style,
      }}
    >
      <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", minWidth: 0 }}>
        {left}
      </span>
      <span style={{ flexShrink: 0 }}>{right}</span>
    </div>
  );
}

function VmRows({ item }: { item: ReceiptVirtualModelItemDto }) {
  if (item.sub_items.length === 0) {
    return (
      <HandRow
        left={
          <>
            &#8212;&nbsp;&nbsp;{item.virtual_model_name}{" "}
            <span style={{ fontSize: 13 }}>(no requests)</span>
          </>
        }
        right="&#8212;"
        size={15}
        color="#9a9a8a"
      />
    );
  }
  const [first, ...rest] = item.sub_items;
  return (
    <>
      <HandRow
        left={
          <>
            {fmtCountEn(first.totals.request_count)}&nbsp;&nbsp;{first.real_model_name}{" "}
            <span style={{ fontSize: 14 }}>({first.provider_display_name})</span>
          </>
        }
        right={fmtTokEn(totalTokensOf(first.totals))}
      />
      {rest.map((sub) => (
        <HandRow
          key={`${sub.subscription_id}|${sub.real_model_name}`}
          left={
            <span style={{ paddingLeft: 14 }}>
              &#8627; {sub.real_model_name} &#215;{fmtCountEn(sub.totals.request_count)}{" "}
              <span style={{ fontSize: 13 }}>({sub.provider_display_name})</span>
            </span>
          }
          right={fmtTokEn(totalTokensOf(sub.totals))}
          size={15}
        />
      ))}
    </>
  );
}
