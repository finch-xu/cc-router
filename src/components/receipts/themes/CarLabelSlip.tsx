import { BarcodeSVG } from "../BarcodeSVG";
import {
  Between,
  BetweenEllipsis,
  SITE_LABEL,
  SITE_URL,
  VERSION,
  fmtCountEn,
  fmtTokEn,
  pct,
  totalTokensOf,
  utcParts,
  vmDisplay,
  type ThemeSlipProps,
} from "./shared";

const NAVY = "#14213d";
const MUTED = "#5b6474";
const LINE = "1px solid #cfd4dd";
const OSWALD = "'Oswald', sans-serif";

/** G · 窗贴标签风格: 分区排版 + TOKEN EFFICIENCY 大数字框 (派生指标提到 C 位)。 */
export function CarLabelSlip({ dto, port }: ThemeSlipProps) {
  const issued = utcParts(dto.generated_at_ms);
  const start = utcParts(dto.range_start_ms);
  const end = utcParts(dto.range_end_ms);
  const g = dto.grand_total;
  const grand = totalTokensOf(g);
  const avg = g.request_count > 0 ? Math.round(grand / g.request_count) : 0;
  const cacheShare = pct(g.cache_read_tokens, grand).toFixed(1);
  const subItems = dto.items.flatMap((it) => it.sub_items);

  const sectionTitle: React.CSSProperties = {
    fontFamily: OSWALD,
    fontWeight: 700,
    fontSize: 12,
    letterSpacing: 2,
    borderBottom: `2px solid ${NAVY}`,
    paddingBottom: 3,
  };

  return (
    <div
      style={{
        width: 360,
        background: "#ffffff",
        color: NAVY,
        boxShadow: "0 8px 18px rgba(0,0,0,0.18)",
        border: LINE,
        fontVariantNumeric: "tabular-nums",
        boxSizing: "border-box",
        fontFamily: '"Archivo Narrow", "Arial Narrow", sans-serif',
      }}
    >
      <div style={{ background: NAVY, color: "#ffffff", padding: "12px 16px" }}>
        <Between
          left={<span style={{ fontFamily: OSWALD, fontWeight: 700, fontSize: 20, letterSpacing: 2 }}>CC-ROUTER</span>}
          right={
            <span style={{ fontFamily: OSWALD, fontWeight: 500, fontSize: 12, letterSpacing: 2, opacity: 0.85 }}>
              v{VERSION}
            </span>
          }
          style={{ alignItems: "baseline" }}
        />
        <div style={{ fontSize: 10, letterSpacing: 1, opacity: 0.75, marginTop: 2 }}>
          USAGE SUMMARY &#183; PERIOD {start.M}/{start.D} - {end.M}/{end.D}/{end.Y}
        </div>
      </div>

      <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", fontSize: 10, borderBottom: LINE }}>
        <div style={{ padding: "6px 16px", borderRight: LINE }}>
          <span style={{ color: MUTED }}>SLIP</span>&nbsp;&nbsp;RCPT-{dto.slip_no}
        </div>
        <div style={{ padding: "6px 16px" }}>
          <span style={{ color: MUTED }}>ENDPOINT</span>&nbsp;&nbsp;127.0.0.1:{port}
        </div>
      </div>

      <div style={{ padding: "10px 16px 0" }}>
        <div style={sectionTitle}>VIRTUAL MODELS</div>
        <div style={{ fontSize: 11, marginTop: 6 }}>
          {dto.items.map((item) => {
            const tok = totalTokensOf(item.subtotal);
            return (
              <Between
                key={item.virtual_model_name}
                left={
                  <span>
                    {vmDisplay(item.virtual_model_name)}
                    {item.sub_items.length > 1 && (
                      <span style={{ color: MUTED }}>
                        {" "}
                        &#183; {item.sub_items.length} subscriptions
                      </span>
                    )}
                  </span>
                }
                right={
                  <span style={{ fontWeight: 600, color: tok === 0 ? "#9aa2b1" : undefined }}>
                    {fmtTokEn(tok)}
                  </span>
                }
                style={{ padding: "2px 0" }}
              />
            );
          })}
        </div>
      </div>

      <div style={{ padding: "12px 16px 0" }}>
        <div style={sectionTitle}>SUBSCRIPTIONS &#183; REAL MODELS</div>
        <div style={{ fontSize: 10, marginTop: 6 }}>
          {subItems.length === 0 ? (
            <div style={{ color: MUTED, padding: "2px 0" }}>NO REQUESTS THIS PERIOD</div>
          ) : (
            subItems.map((sub) => (
              <BetweenEllipsis
                key={`${sub.subscription_id}|${sub.real_model_name}`}
                left={
                  <>
                    {sub.real_model_name.toUpperCase()} &#183;{" "}
                    {sub.provider_display_name.toUpperCase()} /{" "}
                    {(sub.subscription_display_name ?? "DELETED").toUpperCase()} &#183;{" "}
                    {fmtCountEn(sub.totals.request_count)} REQS
                  </>
                }
                right={<span style={{ fontWeight: 600 }}>{fmtTokEn(totalTokensOf(sub.totals))}</span>}
                style={{ padding: "2px 0" }}
              />
            ))
          )}
        </div>
      </div>

      <div style={{ margin: "12px 16px 0", border: `3px solid ${NAVY}`, borderRadius: 6, overflow: "hidden" }}>
        <div
          style={{
            background: NAVY,
            color: "#ffffff",
            textAlign: "center",
            fontFamily: OSWALD,
            fontWeight: 700,
            fontSize: 11,
            letterSpacing: 3,
            padding: "4px 0",
          }}
        >
          TOKEN EFFICIENCY
        </div>
        <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr" }}>
          <div style={{ padding: "10px 8px", textAlign: "center", borderRight: LINE }}>
            <div style={{ fontFamily: OSWALD, fontWeight: 700, fontSize: 30, lineHeight: 1 }}>
              {avg > 0 ? fmtCountEn(avg) : "—"}
            </div>
            <div style={{ fontSize: 9, letterSpacing: 1, color: MUTED, marginTop: 3 }}>
              TOKENS / REQUEST
              <br />
              PERIOD AVG
            </div>
          </div>
          <div style={{ padding: "10px 8px", textAlign: "center" }}>
            <div style={{ fontFamily: OSWALD, fontWeight: 700, fontSize: 30, lineHeight: 1 }}>
              {cacheShare}
              <span style={{ fontSize: 16 }}>%</span>
            </div>
            <div style={{ fontSize: 9, letterSpacing: 1, color: MUTED, marginTop: 3 }}>
              CACHE-READ SHARE
              <br />
              OF ALL TOKENS
            </div>
          </div>
        </div>
        <div style={{ fontSize: 9, color: MUTED, padding: "4px 10px 6px", borderTop: LINE, lineHeight: 1.4 }}>
          Averages computed locally by cc-router from the {fmtCountEn(g.request_count)} requests
          logged this period.
        </div>
      </div>

      <div style={{ margin: "12px 16px 0", background: "#f2f4f8", borderRadius: 4, padding: "10px 12px" }}>
        <Between
          left={<span style={{ fontFamily: OSWALD, fontWeight: 700, fontSize: 13, letterSpacing: 2 }}>TOTAL PERIOD USAGE</span>}
          right={<span style={{ fontFamily: OSWALD, fontWeight: 700, fontSize: 24 }}>{fmtTokEn(grand)}</span>}
          style={{ alignItems: "baseline" }}
        />
        <div style={{ fontSize: 10, color: MUTED, marginTop: 3 }}>
          IN {fmtTokEn(g.input_tokens)} &#183; OUT {fmtTokEn(g.output_tokens)} &#183; C+{" "}
          {fmtTokEn(g.cache_creation_tokens)} &#183; C- {fmtTokEn(g.cache_read_tokens)}
        </div>
      </div>

      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          padding: "12px 16px 16px",
        }}
      >
        <div style={{ fontSize: 9, color: MUTED, lineHeight: 1.5 }}>
          ISSUED {issued.M}/{issued.D}/{issued.Y} {issued.h}:{issued.m}
          <br />
          {SITE_LABEL} &#183; v{VERSION}
        </div>
        <BarcodeSVG value={SITE_URL} height={28} fgColor={NAVY} bgColor="#ffffff" />
      </div>
    </div>
  );
}
