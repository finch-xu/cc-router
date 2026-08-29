import { BarcodeSVG } from "../BarcodeSVG";
import type { ReceiptVirtualModelItemDto } from "@/types";
import {
  Between,
  BetweenEllipsis,
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

const INK = "#141414";
const MUTED = "#444444";
const PAPER = "#fffdf5";
const STARS = "* * * * * * * * * * * * * * * * * *";

/** B · 美国大卖场: 全大写 + 星号分隔 + CACHE SAVINGS 省钱框。 */
export function UsGrocerySlip({ dto, port }: ThemeSlipProps) {
  const issued = utcParts(dto.generated_at_ms);
  const start = utcParts(dto.range_start_ms);
  const end = utcParts(dto.range_end_ms);
  const g = dto.grand_total;
  const yy = (y: number) => String(y).slice(2);

  return (
    <ZigzagPaper
      color={PAPER}
      bodyStyle={{
        color: INK,
        padding: "18px 18px 22px",
        fontSize: 12,
        lineHeight: 1.5,
        fontVariantNumeric: "tabular-nums",
        fontFamily: '"Courier Prime", "Courier New", monospace',
      }}
    >
      <div style={{ textAlign: "center" }}>
        <div style={{ fontWeight: 700, fontSize: 26, letterSpacing: 2 }}>CC-ROUTER</div>
        <div style={{ fontWeight: 700, fontSize: 12, letterSpacing: 5, marginTop: 1 }}>
          MEGASTORE
        </div>
        <div style={{ fontSize: 11, marginTop: 5 }}>SAVE TOKENS. ROUTE SMARTER.</div>
        <div style={{ fontSize: 11 }}>1-800-CC-ROUTE</div>
      </div>

      <div style={{ textAlign: "center", fontSize: 10, letterSpacing: 1, marginTop: 8, color: "#333333" }}>
        ST# 0001&nbsp;&nbsp;OP# {port}&nbsp;&nbsp;TE# 04&nbsp;&nbsp;TR# {dto.slip_no}
      </div>
      <div style={{ textAlign: "center", fontSize: 10, letterSpacing: 1, color: "#333333" }}>
        PERIOD {start.M}/{start.D}/{yy(start.Y)} - {end.M}/{end.D}/{yy(end.Y)}
      </div>

      <div style={{ textAlign: "center", fontSize: 11, letterSpacing: 1, margin: "8px 0" }}>{STARS}</div>

      {dto.items.map((item) => (
        <VmBlock key={item.virtual_model_name} item={item} />
      ))}

      <div style={{ borderTop: "1px dashed #888888", margin: "8px 0" }} />

      <Between left="SUBTOTAL" right={fmtTokEn(totalTokensOf(g))} style={{ fontSize: 12 }} />
      <Between left={<span>&nbsp;IN</span>} right={fmtTokEn(g.input_tokens)} style={{ fontSize: 11, color: MUTED }} />
      <Between left={<span>&nbsp;OUT</span>} right={fmtTokEn(g.output_tokens)} style={{ fontSize: 11, color: MUTED }} />
      <Between left={<span>&nbsp;CACHE-W</span>} right={fmtTokEn(g.cache_creation_tokens)} style={{ fontSize: 11, color: MUTED }} />
      <Between left={<span>&nbsp;CACHE-R</span>} right={fmtTokEn(g.cache_read_tokens)} style={{ fontSize: 11, color: MUTED }} />

      <Between
        left={<span style={{ fontSize: 16, letterSpacing: 2 }}>TOTAL</span>}
        right={<span style={{ fontSize: 20 }}>{fmtTokEn(totalTokensOf(g))}</span>}
        style={{ fontWeight: 700, marginTop: 6, alignItems: "baseline" }}
      />
      <Between left="# ITEMS SOLD" right={fmtCountEn(g.request_count)} style={{ fontSize: 11, marginTop: 2 }} />

      <div style={{ textAlign: "center", fontSize: 11, letterSpacing: 1, margin: "8px 0" }}>{STARS}</div>

      <div style={{ border: `2px solid ${INK}`, padding: "8px 10px", textAlign: "center" }}>
        <div style={{ fontWeight: 700, fontSize: 13, letterSpacing: 2 }}>CACHE SAVINGS</div>
        <div style={{ fontSize: 11, marginTop: 3 }}>YOU SAVED</div>
        <div style={{ fontWeight: 700, fontSize: 22, letterSpacing: 1 }}>
          {fmtTokEn(g.cache_read_tokens)}
        </div>
        <div style={{ fontSize: 10, color: MUTED }}>CACHE-READ TOKENS THIS PERIOD</div>
      </div>

      <div style={{ textAlign: "center", fontSize: 10, letterSpacing: 1, color: "#333333", marginTop: 10 }}>
        TC# {dto.slip_no}
      </div>

      <div style={{ display: "flex", justifyContent: "center", marginTop: 6 }}>
        <BarcodeSVG value={SITE_URL} height={36} fgColor={INK} bgColor={PAPER} />
      </div>

      <div style={{ textAlign: "center", marginTop: 10 }}>
        <div style={{ fontWeight: 700, fontSize: 12, letterSpacing: 2 }}>EVERY TOKEN COUNTS.</div>
        <div style={{ fontSize: 10, color: MUTED, marginTop: 4 }}>TELL US HOW WE ROUTED</div>
        <div style={{ fontSize: 10, color: MUTED }}>SURVEY AT CCROUTER.APP</div>
        <div style={{ fontSize: 10, color: MUTED, marginTop: 4 }}>
          {issued.M}/{issued.D}/{yy(issued.Y)}&nbsp;&nbsp;{issued.h}:{issued.m}:{issued.s}
          &nbsp;&nbsp;v{VERSION}
        </div>
      </div>
    </ZigzagPaper>
  );
}

function VmBlock({ item }: { item: ReceiptVirtualModelItemDto }) {
  const isEmpty = item.sub_items.length === 0;
  return (
    <div style={{ marginTop: 6 }}>
      <div style={{ fontWeight: 700, letterSpacing: 1 }}>{vmDisplay(item.virtual_model_name)}</div>
      {isEmpty ? (
        <div style={{ fontSize: 10, color: MUTED }}>&nbsp;&nbsp;NO ITEMS THIS PERIOD</div>
      ) : (
        <>
          {item.sub_items.map((sub) => (
            <div key={`${sub.subscription_id}|${sub.real_model_name}`}>
              <BetweenEllipsis
                left={<span>&nbsp;{sub.real_model_name.toUpperCase()}</span>}
                right={`${fmtTokEn(totalTokensOf(sub.totals))} T`}
              />
              <div
                style={{
                  fontSize: 10,
                  color: MUTED,
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  whiteSpace: "nowrap",
                }}
              >
                &nbsp;&nbsp;{sub.provider_display_name.toUpperCase()}/
                {subLabel(sub, "DELETED SUB").toUpperCase()}&nbsp;&nbsp;QTY{" "}
                {fmtCountEn(sub.totals.request_count)}
              </div>
              <div style={{ fontSize: 10, color: MUTED }}>
                &nbsp;&nbsp;IN {fmtTokEn(sub.totals.input_tokens)}&nbsp;&nbsp;OUT{" "}
                {fmtTokEn(sub.totals.output_tokens)}
                {hasCache(sub.totals) && (
                  <>
                    &nbsp;&nbsp;CW {fmtTokEn(sub.totals.cache_creation_tokens)}&nbsp;&nbsp;CR{" "}
                    {fmtTokEn(sub.totals.cache_read_tokens)}
                  </>
                )}
              </div>
            </div>
          ))}
          {item.sub_items.length > 1 && (
            <Between
              left={<span>&nbsp;GROUP SUBTOTAL</span>}
              right={fmtTokEn(totalTokensOf(item.subtotal))}
              style={{ fontSize: 10, color: MUTED }}
            />
          )}
        </>
      )}
    </div>
  );
}
