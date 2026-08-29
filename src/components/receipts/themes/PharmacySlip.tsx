import { BarcodeSVG } from "../BarcodeSVG";
import type { ReceiptSubItemDto, ReceiptVirtualModelItemDto } from "@/types";
import {
  Between,
  SITE_LABEL,
  SITE_URL,
  VERSION,
  ZigzagPaper,
  fmtCountEn,
  fmtTokEn,
  hasCache,
  subLabel,
  totalTokensOf,
  utcParts,
  vmDisplay,
  type ThemeSlipProps,
} from "./shared";

const INK = "#1b1b1b";
const GREEN = "#135e3d";
const MUTED = "#444444";
const AMBER = "#8a5a00";
const CARD_BORDER = "1px solid #c9d6cd";
const MONO = "'IBM Plex Mono', monospace";

/** E · 药房风格: 绿十字 + 卡片式分组; 文字全部用真实术语, 不用处方用语。 */
export function PharmacySlip({ dto, port }: ThemeSlipProps) {
  const issued = utcParts(dto.generated_at_ms);
  const start = utcParts(dto.range_start_ms);
  const end = utcParts(dto.range_end_ms);
  const g = dto.grand_total;
  const yy = (y: number) => String(y).slice(2);

  return (
    <ZigzagPaper
      color="#ffffff"
      bodyStyle={{
        color: INK,
        padding: "16px 16px 20px",
        fontSize: 11,
        lineHeight: 1.5,
        fontVariantNumeric: "tabular-nums",
        fontFamily: '"Archivo", "Helvetica Neue", Arial, sans-serif',
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
        <svg width="34" height="34" viewBox="0 0 34 34" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
          <rect x="1" y="1" width="32" height="32" rx="7" fill={GREEN} />
          <rect x="14" y="7" width="6" height="20" rx="1.5" fill="#ffffff" />
          <rect x="7" y="14" width="20" height="6" rx="1.5" fill="#ffffff" />
        </svg>
        <div>
          <div style={{ fontWeight: 800, fontSize: 17, letterSpacing: 1, color: GREEN }}>
            CC-ROUTER
          </div>
          <div style={{ fontSize: 9, letterSpacing: 2, color: "#555555" }}>
            USAGE SLIP &#183; 24H LOCAL GATEWAY
          </div>
        </div>
      </div>

      <div style={{ borderTop: `2px solid ${GREEN}`, margin: "10px 0 8px" }} />

      <div
        style={{
          display: "grid",
          gridTemplateColumns: "1fr 1fr",
          rowGap: 2,
          fontFamily: MONO,
          fontSize: 10,
          color: "#333333",
        }}
      >
        <span>SLIP RCPT-{dto.slip_no}</span>
        <span style={{ textAlign: "right" }}>
          DATE {issued.M}/{issued.D}/{yy(issued.Y)} {issued.h}:{issued.m}
        </span>
        <span>CLIENT: CLAUDE CODE</span>
        <span style={{ textAlign: "right" }}>GATEWAY: 127.0.0.1</span>
        <span>
          PERIOD: {start.M}/{start.D} - {end.M}/{end.D}
        </span>
        <span style={{ textAlign: "right" }}>PORT {port}</span>
      </div>

      {dto.items.map((item, idx) => (
        <SlotCard key={item.virtual_model_name} item={item} slot={idx + 1} />
      ))}

      <div style={{ background: "#f0f6f2", borderRadius: 4, padding: "10px 12px", marginTop: 12 }}>
        <Between
          left={<span style={{ fontWeight: 800, fontSize: 12, letterSpacing: 1, color: GREEN }}>TOTAL TOKENS</span>}
          right={<span style={{ fontWeight: 800, fontSize: 19 }}>{fmtTokEn(totalTokensOf(g))}</span>}
          style={{ alignItems: "baseline" }}
        />
        <Between
          left="TOTAL REQUESTS"
          right={fmtCountEn(g.request_count)}
          style={{ fontSize: 10, color: MUTED, marginTop: 2 }}
        />
        <div style={{ fontFamily: MONO, fontSize: 10, color: MUTED, marginTop: 4 }}>
          IN {fmtTokEn(g.input_tokens)} &#183; OUT {fmtTokEn(g.output_tokens)} &#183; C+{" "}
          {fmtTokEn(g.cache_creation_tokens)} &#183; C- {fmtTokEn(g.cache_read_tokens)}
        </div>
      </div>

      <div style={{ fontSize: 9, color: AMBER, marginTop: 10, lineHeight: 1.5 }}>
        &#9888;&#65038; ALL USAGE AGGREGATED LOCALLY BY CC-ROUTER.
        <br />
        KEEP API KEYS OUT OF PUBLIC REPOS.
      </div>

      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-end", marginTop: 12 }}>
        <div style={{ fontSize: 10, color: MUTED }}>
          ISSUED:{" "}
          <span style={{ fontWeight: 600 }}>
            {issued.M}/{issued.D} {issued.h}:{issued.m}
          </span>
        </div>
        <BarcodeSVG value={SITE_URL} height={30} fgColor={INK} bgColor="#ffffff" />
      </div>

      <div style={{ textAlign: "center", fontSize: 9, color: "#666666", marginTop: 10, letterSpacing: 0.5 }}>
        {SITE_LABEL} &#183; v{VERSION} &#183; SLIP RCPT-{dto.slip_no}
      </div>
    </ZigzagPaper>
  );
}

function SlotCard({ item, slot }: { item: ReceiptVirtualModelItemDto; slot: number }) {
  const [first, ...rest] = item.sub_items;
  return (
    <div style={{ border: CARD_BORDER, borderRadius: 4, padding: "9px 10px", marginTop: 8 }}>
      <Between
        left={
          <span style={{ color: GREEN }}>
            SLOT {slot} &#183; {vmDisplay(item.virtual_model_name)}
          </span>
        }
        right={fmtTokEn(totalTokensOf(item.subtotal))}
        style={{ fontWeight: 800, fontSize: 12 }}
      />
      {!first ? (
        <div style={{ fontSize: 10, color: "#888888", marginTop: 3 }}>NO REQUESTS THIS PERIOD</div>
      ) : (
        <>
          <SubLines sub={first} />
          {rest.length > 0 && (
            <>
              <div style={{ borderTop: "1px dashed #c9d6cd", margin: "6px 0" }} />
              <div style={{ fontSize: 9, fontWeight: 600, letterSpacing: 1, color: AMBER }}>
                ALSO ROUTED THIS SLOT
              </div>
              {rest.map((sub) => (
                <SubLines key={`${sub.subscription_id}|${sub.real_model_name}`} sub={sub} />
              ))}
            </>
          )}
        </>
      )}
    </div>
  );
}

function SubLines({ sub }: { sub: ReceiptSubItemDto }) {
  return (
    <div style={{ marginTop: 4 }}>
      <div
        style={{
          fontFamily: MONO,
          fontSize: 10,
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
        }}
      >
        {sub.real_model_name.toUpperCase()} &#183; {sub.provider_display_name.toUpperCase()} /{" "}
        {subLabel(sub, "DELETED SUBSCRIPTION").toUpperCase()}
      </div>
      <div style={{ fontSize: 10, color: MUTED }}>
        {fmtTokEn(totalTokensOf(sub.totals))} TOKENS &#183;{" "}
        {fmtCountEn(sub.totals.request_count)} REQUESTS
      </div>
      <div style={{ fontSize: 10, color: MUTED }}>
        IN {fmtTokEn(sub.totals.input_tokens)} / OUT {fmtTokEn(sub.totals.output_tokens)}
        {hasCache(sub.totals) && (
          <>
            {" "}
            / C+ {fmtTokEn(sub.totals.cache_creation_tokens)} / C-{" "}
            {fmtTokEn(sub.totals.cache_read_tokens)}
          </>
        )}
      </div>
    </div>
  );
}
