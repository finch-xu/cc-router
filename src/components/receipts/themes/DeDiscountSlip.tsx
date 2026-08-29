import { Fragment } from "react";
import { BarcodeSVG } from "../BarcodeSVG";
import {
  Between,
  BetweenEllipsis,
  SITE_LABEL,
  SITE_URL,
  VERSION,
  ZigzagPaper,
  fmtCountDe,
  fmtTokDe,
  hasCache,
  pct,
  pseudoSignature,
  subLabel,
  totalTokensOf,
  utcParts,
  vmDisplay,
  type ThemeSlipProps,
} from "./shared";

const INK = "#111111";
const MUTED = "#555555";
const SOFT = "#444444";
const FAINT = "#777777";
const KLASSEN = ["A", "B", "C", "D", "E", "F"];

/** C · 德国折扣超市: 高密度排版 + Klassen 表 (税级字母=虚拟模型) + TSE 签名行。 */
export function DeDiscountSlip({ dto, port }: ThemeSlipProps) {
  const issued = utcParts(dto.generated_at_ms);
  const start = utcParts(dto.range_start_ms);
  const end = utcParts(dto.range_end_ms);
  const g = dto.grand_total;
  const grand = totalTokensOf(g);
  const dePct = (part: number) => pct(part, grand).toFixed(1).replace(".", ",") + "%";

  return (
    <ZigzagPaper
      color="#ffffff"
      bodyStyle={{
        color: INK,
        padding: "16px 16px 20px",
        fontSize: 11,
        lineHeight: 1.5,
        fontVariantNumeric: "tabular-nums",
        fontFamily: '"IBM Plex Mono", Menlo, monospace',
      }}
    >
      <div style={{ textAlign: "center" }}>
        <div style={{ fontWeight: 700, fontSize: 20, letterSpacing: 1 }}>CC-ROUTER MARKT</div>
        <div style={{ fontSize: 10, color: SOFT, marginTop: 3 }}>
          Filiale 127.0.0.1 &#183; Kasse {port}
        </div>
      </div>

      <div style={{ borderTop: `1px solid ${INK}`, margin: "9px 0 7px" }} />

      <Between
        left={`Zeitraum ${start.D}.${start.M}. - ${end.D}.${end.M}.${end.Y}`}
        right={`Bon-Nr. ${dto.slip_no}`}
        style={{ fontSize: 10, color: SOFT }}
      />

      <div style={{ borderTop: "1px dashed #999999", margin: "7px 0" }} />

      {dto.items.map((item, idx) =>
        item.sub_items.map((sub) => (
          <div key={`${sub.subscription_id}|${sub.real_model_name}`} style={{ marginTop: 4 }}>
            <BetweenEllipsis
              left={sub.real_model_name}
              right={`${fmtTokDe(totalTokensOf(sub.totals))} ${KLASSEN[idx] ?? "?"}`}
              leftStyle={{ fontWeight: 500 }}
              style={{ fontWeight: 500 }}
            />
            <div
              style={{
                fontSize: 10,
                color: MUTED,
                paddingLeft: 8,
                overflow: "hidden",
                textOverflow: "ellipsis",
                whiteSpace: "nowrap",
              }}
            >
              {sub.provider_display_name}/{subLabel(sub, "gelöschtes Abo")} &#183;{" "}
              {fmtCountDe(sub.totals.request_count)}x
            </div>
            <div style={{ fontSize: 10, color: MUTED, paddingLeft: 8 }}>
              Ein {fmtTokDe(sub.totals.input_tokens)}&nbsp;&nbsp;Aus{" "}
              {fmtTokDe(sub.totals.output_tokens)}
              {hasCache(sub.totals) && (
                <>
                  &nbsp;&nbsp;C+ {fmtTokDe(sub.totals.cache_creation_tokens)}&nbsp;&nbsp;C-{" "}
                  {fmtTokDe(sub.totals.cache_read_tokens)}
                </>
              )}
            </div>
          </div>
        )),
      )}

      <div style={{ borderTop: `1px solid ${INK}`, margin: "8px 0 5px" }} />

      <Between
        left={<span style={{ fontSize: 14, letterSpacing: 1 }}>SUMME</span>}
        right={<span style={{ fontSize: 18 }}>{fmtTokDe(grand)}</span>}
        style={{ fontWeight: 700, alignItems: "baseline" }}
      />
      <Between
        left="Posten"
        right={fmtCountDe(g.request_count)}
        style={{ fontSize: 10, color: SOFT, marginTop: 2 }}
      />
      <div style={{ fontSize: 10, color: SOFT }}>
        Eingabe {fmtTokDe(g.input_tokens)} &#183; Ausgabe {fmtTokDe(g.output_tokens)}
      </div>
      <div style={{ fontSize: 10, color: SOFT }}>
        Cache-Schreiben {fmtTokDe(g.cache_creation_tokens)} &#183; Cache-Lesen{" "}
        {fmtTokDe(g.cache_read_tokens)}
      </div>

      <div style={{ borderTop: "1px dashed #999999", margin: "8px 0" }} />

      <div
        style={{
          display: "grid",
          gridTemplateColumns: "24px 1fr auto auto",
          columnGap: 8,
          fontSize: 10,
        }}
      >
        <span style={{ fontWeight: 700 }}>KL</span>
        <span style={{ fontWeight: 700 }}>MODELLKLASSE</span>
        <span style={{ fontWeight: 700, textAlign: "right" }}>TOKENS</span>
        <span style={{ fontWeight: 700, textAlign: "right" }}>ANTEIL</span>
        {dto.items.map((item, idx) => (
          <Fragment key={item.virtual_model_name}>
            <span>{KLASSEN[idx] ?? "?"}</span>
            <span>{vmDisplay(item.virtual_model_name)}</span>
            <span style={{ textAlign: "right" }}>{fmtTokDe(totalTokensOf(item.subtotal))}</span>
            <span style={{ textAlign: "right" }}>{dePct(totalTokensOf(item.subtotal))}</span>
          </Fragment>
        ))}
        <span style={{ borderTop: "1px solid #cccccc", paddingTop: 2 }} />
        <span style={{ borderTop: "1px solid #cccccc", paddingTop: 2 }}>GESAMT</span>
        <span style={{ borderTop: "1px solid #cccccc", paddingTop: 2, textAlign: "right" }}>
          {fmtTokDe(grand)}
        </span>
        <span style={{ borderTop: "1px solid #cccccc", paddingTop: 2, textAlign: "right" }}>
          {grand > 0 ? "100,0%" : "0,0%"}
        </span>
      </div>

      <div style={{ borderTop: "1px dashed #999999", margin: "8px 0" }} />

      <Between
        left={`${issued.D}.${issued.M}.${issued.Y}`}
        right={
          <span>
            {issued.h}:{issued.m}:{issued.s}&nbsp;&nbsp;&nbsp;Beleg {dto.slip_no}
          </span>
        }
        style={{ fontSize: 10, color: SOFT }}
      />

      <div style={{ marginTop: 7 }}>
        <div style={{ fontSize: 9, fontWeight: 700, letterSpacing: 1, color: SOFT }}>
          TSE-SIGNATUR
        </div>
        <div style={{ fontSize: 9, color: FAINT, wordBreak: "break-all", lineHeight: 1.45 }}>
          {pseudoSignature(dto)}
        </div>
        <Between
          left={`TSE-TransNr. ${port}`}
          right={`Sig-Zähler ${fmtCountDe(g.request_count)}`}
          style={{ fontSize: 9, color: FAINT, marginTop: 2 }}
        />
      </div>

      <div style={{ textAlign: "center", fontWeight: 700, fontSize: 12, marginTop: 12, letterSpacing: 0.5 }}>
        Vielen Dank f&#252;r Ihren Einkauf!
      </div>

      <div style={{ display: "flex", justifyContent: "center", marginTop: 10 }}>
        <BarcodeSVG value={SITE_URL} height={36} fgColor={INK} bgColor="#ffffff" />
      </div>

      <div style={{ textAlign: "center", fontSize: 9, color: MUTED, marginTop: 8, letterSpacing: 0.5 }}>
        {SITE_LABEL} &#183; v{VERSION}
      </div>
    </ZigzagPaper>
  );
}
