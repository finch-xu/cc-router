import { BarcodeSVG } from "../BarcodeSVG";
import type { ReceiptVirtualModelItemDto } from "@/types";
import {
  Between,
  BetweenEllipsis,
  SITE_LABEL,
  SITE_URL,
  VERSION,
  ZigzagPaper,
  fmtCountFr,
  fmtTokFr,
  hasCache,
  subLabel,
  totalTokensOf,
  utcParts,
  type ThemeSlipProps,
} from "./shared";

const INK = "#23418f";
const PAPER = "#fbf6e9";
const SERIF = "'Cormorant Garamond', Georgia, serif";

/** "model-fable" → "MODÈLE-FABLE" */
function vmFr(name: string): string {
  return name.toUpperCase().replace(/^MODEL-/, "MODÈLE-");
}

/** D · 法国超级市场: 奶油纸 + 蓝墨 + 衬线店名 + 积分卡 (1 pt = 1M tokens)。 */
export function FrMarketSlip({ dto, port }: ThemeSlipProps) {
  const issued = utcParts(dto.generated_at_ms);
  const start = utcParts(dto.range_start_ms);
  const end = utcParts(dto.range_end_ms);
  const g = dto.grand_total;
  const points = Math.round(totalTokensOf(g) / 1_000_000);

  return (
    <ZigzagPaper
      color={PAPER}
      bodyStyle={{
        color: INK,
        padding: "18px 18px 22px",
        fontSize: 11,
        lineHeight: 1.55,
        fontVariantNumeric: "tabular-nums",
        fontFamily: '"Space Mono", Menlo, monospace',
      }}
    >
      <div style={{ textAlign: "center" }}>
        <div style={{ fontFamily: SERIF, fontWeight: 600, fontSize: 30, letterSpacing: 1, lineHeight: 1.1 }}>
          CC-Router
        </div>
        <div style={{ fontFamily: SERIF, fontStyle: "italic", fontWeight: 600, fontSize: 15, marginTop: 1 }}>
          March&#233; des jetons
        </div>
        <div style={{ fontSize: 10, marginTop: 6, opacity: 0.75 }}>
          Caisse {port} &#183; H&#244;te ROUTER
        </div>
      </div>

      <div style={{ borderTop: `1px solid ${INK}`, margin: "10px 0 7px", opacity: 0.8 }} />

      <Between
        left={`le ${issued.D}/${issued.M}/${issued.Y} à ${issued.h}h${issued.m}`}
        right={`Ticket N° ${dto.slip_no}`}
        style={{ fontSize: 10 }}
      />
      <Between
        left="P&#233;riode"
        right={`du ${start.D}/${start.M} au ${end.D}/${end.M}`}
        style={{ fontSize: 10, opacity: 0.75 }}
      />

      <div style={{ borderTop: `1px dashed ${INK}`, margin: "8px 0", opacity: 0.45 }} />

      {dto.items.map((item) => (
        <VmBlock key={item.virtual_model_name} item={item} />
      ))}

      <div style={{ borderTop: `1px solid ${INK}`, margin: "10px 0 6px", opacity: 0.8 }} />

      <Between
        left={<span style={{ fontSize: 14, letterSpacing: 2 }}>TOTAL</span>}
        right={<span style={{ fontSize: 19 }}>{fmtTokFr(totalTokensOf(g))}</span>}
        style={{ fontWeight: 700, alignItems: "baseline" }}
      />
      <Between
        left="Articles"
        right={fmtCountFr(g.request_count)}
        style={{ fontSize: 10, opacity: 0.85, marginTop: 2 }}
      />
      <div style={{ fontSize: 10, opacity: 0.75, marginTop: 3 }}>
        dont entr&#233;e {fmtTokFr(g.input_tokens)} &#183; sortie {fmtTokFr(g.output_tokens)}
      </div>
      <div style={{ fontSize: 10, opacity: 0.75 }}>
        dont cache &#233;crit {fmtTokFr(g.cache_creation_tokens)} &#183; lu{" "}
        {fmtTokFr(g.cache_read_tokens)}
      </div>

      <div style={{ border: `1px dashed ${INK}`, borderRadius: 2, padding: "8px 10px", marginTop: 10 }}>
        <div style={{ fontWeight: 700, fontSize: 11, letterSpacing: 2, textAlign: "center" }}>
          CARTE FID&#201;LIT&#201;
        </div>
        <Between
          left="Points acquis"
          right={<span style={{ fontWeight: 700 }}>+{fmtCountFr(points)} pts</span>}
          style={{ fontSize: 10, marginTop: 5 }}
        />
        <div style={{ fontSize: 9, opacity: 0.7, marginTop: 3, textAlign: "center" }}>
          1 pt = 1M de jetons rout&#233;s
        </div>
      </div>

      <div style={{ textAlign: "center", marginTop: 12 }}>
        <div style={{ fontFamily: SERIF, fontStyle: "italic", fontWeight: 600, fontSize: 15 }}>
          Merci de votre visite, &#224; bient&#244;t&#8239;!
        </div>
      </div>

      <div style={{ display: "flex", justifyContent: "center", marginTop: 10 }}>
        <BarcodeSVG value={SITE_URL} height={36} fgColor={INK} bgColor={PAPER} />
      </div>

      <div style={{ textAlign: "center", fontSize: 9, opacity: 0.7, marginTop: 8, letterSpacing: 0.5 }}>
        {SITE_LABEL} &#183; v{VERSION}
      </div>
    </ZigzagPaper>
  );
}

function VmBlock({ item }: { item: ReceiptVirtualModelItemDto }) {
  const isEmpty = item.sub_items.length === 0;
  return (
    <div style={{ marginTop: 8 }}>
      <Between
        left={vmFr(item.virtual_model_name)}
        right={fmtTokFr(totalTokensOf(item.subtotal))}
        style={{ fontWeight: 700 }}
      />
      {isEmpty ? (
        <div style={{ paddingLeft: 20, fontSize: 9, opacity: 0.75, marginTop: 1 }}>
          (aucun article)
        </div>
      ) : (
        item.sub_items.map((sub) => (
          <div key={`${sub.subscription_id}|${sub.real_model_name}`} style={{ marginTop: 2 }}>
            <BetweenEllipsis
              left={sub.real_model_name}
              right={fmtTokFr(totalTokensOf(sub.totals))}
              style={{ paddingLeft: 10 }}
            />
            <div
              style={{
                paddingLeft: 20,
                fontSize: 9,
                opacity: 0.75,
                overflow: "hidden",
                textOverflow: "ellipsis",
                whiteSpace: "nowrap",
              }}
            >
              {sub.provider_display_name} / {subLabel(sub, "abonnement supprimé")} &#183;{" "}
              {fmtCountFr(sub.totals.request_count)} art.
            </div>
            <div style={{ paddingLeft: 20, fontSize: 10, opacity: 0.85 }}>
              entr&#233;e {fmtTokFr(sub.totals.input_tokens)} &#183; sortie{" "}
              {fmtTokFr(sub.totals.output_tokens)}
            </div>
            {hasCache(sub.totals) && (
              <div style={{ paddingLeft: 20, fontSize: 10, opacity: 0.85 }}>
                cache &#233;crit {fmtTokFr(sub.totals.cache_creation_tokens)} &#183; lu{" "}
                {fmtTokFr(sub.totals.cache_read_tokens)}
              </div>
            )}
          </div>
        ))
      )}
    </div>
  );
}
