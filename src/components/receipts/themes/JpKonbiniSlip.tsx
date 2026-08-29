import { BarcodeSVG } from "../BarcodeSVG";
import type { ReceiptVirtualModelItemDto } from "@/types";
import {
  Between,
  BetweenEllipsis,
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

const INK = "#1a1a1a";
const MUTED = "#555555";
const SOFT = "#333333";
const WEEKDAY_JA = ["日", "月", "火", "水", "木", "金", "土"];

/** A · 日本コンビニ: DotGothic16 点阵字 + 領収書排版, レジ番号 = 代理端口。 */
export function JpKonbiniSlip({ dto, port }: ThemeSlipProps) {
  const issued = utcParts(dto.generated_at_ms);
  const start = utcParts(dto.range_start_ms);
  const end = utcParts(dto.range_end_ms);
  const g = dto.grand_total;
  const anyCache = hasCache(g);

  return (
    <ZigzagPaper
      color="#ffffff"
      bodyStyle={{
        color: INK,
        padding: "18px 18px 22px",
        fontSize: 12,
        lineHeight: 1.65,
        fontVariantNumeric: "tabular-nums",
        fontFamily: '"DotGothic16", "Hiragino Kaku Gothic ProN", "Yu Gothic", monospace',
      }}
    >
      <div style={{ textAlign: "center" }}>
        <div style={{ fontSize: 21, letterSpacing: 10, paddingLeft: 10 }}>領収書</div>
        <div style={{ fontSize: 16, letterSpacing: 3, marginTop: 8 }}>CC-ROUTER</div>
        <div style={{ fontSize: 10, color: MUTED, marginTop: 2 }}>
          ゲートウェイ店　127.0.0.1
        </div>
        <div style={{ fontSize: 10, color: MUTED }}>登録番号　T0-0000-{port}</div>
      </div>

      <div style={{ borderTop: `1px solid ${INK}`, margin: "10px 0 8px" }} />

      <Between
        left={`${issued.Y}年${issued.M}月${issued.D}日(${WEEKDAY_JA[issued.wd]}) ${issued.h}:${issued.m}`}
        right={`レジ#${port}`}
        style={{ fontSize: 11 }}
      />
      <Between
        left={<span style={{ color: MUTED }}>{`集計期間　${start.M}/${start.D}〜${end.M}/${end.D}`}</span>}
        right="担当 ROUTER"
        style={{ fontSize: 11 }}
      />
      <Between
        left={<span style={{ color: MUTED }}>伝票番号</span>}
        right={`RCPT-${dto.slip_no}`}
        style={{ fontSize: 11 }}
      />

      <div style={{ borderTop: "1px dashed #999999", margin: "8px 0" }} />

      {dto.items.map((item, idx) => (
        <VmBlock key={item.virtual_model_name} item={item} isLast={idx === dto.items.length - 1} />
      ))}

      <div style={{ borderTop: `1px solid ${INK}`, margin: "10px 0 6px" }} />

      <Between left="お買上点数" right={`${fmtCountEn(g.request_count)}点`} style={{ fontSize: 12 }} />
      <Between
        left={<span style={{ fontSize: 15, letterSpacing: 4 }}>合　計</span>}
        right={<span style={{ fontSize: 19 }}>{fmtTokEn(totalTokensOf(g))}</span>}
        style={{ alignItems: "baseline", marginTop: 2 }}
      />
      <div style={{ marginTop: 4 }}>
        <Between
          left={<span style={{ paddingLeft: 12 }}>（内　入力）</span>}
          right={fmtTokEn(g.input_tokens)}
          style={{ fontSize: 11, color: SOFT }}
        />
        <Between
          left={<span style={{ paddingLeft: 12 }}>（内　出力）</span>}
          right={fmtTokEn(g.output_tokens)}
          style={{ fontSize: 11, color: SOFT }}
        />
        {anyCache && (
          <>
            <Between
              left={<span style={{ paddingLeft: 12 }}>（内　C+ 作成※）</span>}
              right={fmtTokEn(g.cache_creation_tokens)}
              style={{ fontSize: 11, color: SOFT }}
            />
            <Between
              left={<span style={{ paddingLeft: 12 }}>（内　C- 読取※）</span>}
              right={fmtTokEn(g.cache_read_tokens)}
              style={{ fontSize: 11, color: SOFT }}
            />
          </>
        )}
      </div>

      <div style={{ borderTop: "1px dashed #999999", margin: "8px 0" }} />

      {anyCache && (
        <div style={{ fontSize: 10, color: MUTED }}>※印はプロンプトキャッシュ対象です</div>
      )}

      <div style={{ textAlign: "center", marginTop: 12, fontSize: 12 }}>
        <div>ありがとうございました</div>
        <div style={{ marginTop: 2 }}>またのご利用をお待ちしております</div>
      </div>

      <div style={{ display: "flex", flexDirection: "column", alignItems: "center", marginTop: 12 }}>
        <BarcodeSVG value={SITE_URL} height={36} fgColor={INK} bgColor="#ffffff" />
        <div style={{ fontSize: 10, letterSpacing: 3, marginTop: 3 }}>RCPT-{dto.slip_no}</div>
      </div>

      <div style={{ textAlign: "center", fontSize: 10, color: MUTED, marginTop: 10, letterSpacing: 1 }}>
        {SITE_LABEL}　v{VERSION}
      </div>
    </ZigzagPaper>
  );
}

function VmBlock({ item, isLast }: { item: ReceiptVirtualModelItemDto; isLast: boolean }) {
  const isEmpty = item.sub_items.length === 0;
  return (
    <div>
      <Between
        left={`【${vmDisplay(item.virtual_model_name)}】`}
        right={`${fmtCountEn(item.subtotal.request_count)}点`}
      />
      {isEmpty ? (
        <div style={{ paddingLeft: 24, fontSize: 10, color: MUTED }}>（ご利用なし）</div>
      ) : (
        <>
          {item.sub_items.map((sub) => (
            <div key={`${sub.subscription_id}|${sub.real_model_name}`} style={{ marginTop: 2 }}>
              <BetweenEllipsis
                left={`${sub.real_model_name}${hasCache(sub.totals) ? " ※" : ""}`}
                right={fmtTokEn(totalTokensOf(sub.totals))}
                style={{ paddingLeft: 12 }}
              />
              <div
                style={{
                  paddingLeft: 24,
                  fontSize: 10,
                  color: MUTED,
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  whiteSpace: "nowrap",
                }}
              >
                {`${sub.provider_display_name}／${subLabel(sub, "削除済み")}　${fmtCountEn(sub.totals.request_count)}点`}
              </div>
              <div style={{ paddingLeft: 24, fontSize: 11, color: SOFT }}>
                入 {fmtTokEn(sub.totals.input_tokens)}　出 {fmtTokEn(sub.totals.output_tokens)}
              </div>
              {hasCache(sub.totals) && (
                <div style={{ paddingLeft: 24, fontSize: 11, color: SOFT }}>
                  C+ {fmtTokEn(sub.totals.cache_creation_tokens)}　C-{" "}
                  {fmtTokEn(sub.totals.cache_read_tokens)}
                </div>
              )}
            </div>
          ))}
          <Between
            left={<span style={{ paddingLeft: 12 }}>小計</span>}
            right={fmtTokEn(totalTokensOf(item.subtotal))}
            style={{ fontSize: 11, color: MUTED }}
          />
        </>
      )}
      {!isLast && <div style={{ borderTop: "1px dashed #999999", margin: "7px 0" }} />}
    </div>
  );
}
