import type { CSSProperties, ReactNode } from "react";
import type { ReceiptDto, ReceiptSubItemDto, ReceiptTotalsDto } from "@/types";
import { fmtCompact } from "@/lib/format";
import { version as VERSION } from "../../../../package.json";

// 主题字体走自有 CDN webfont (d.cc-router.catonthe.top/fonts/), 不打包进安装包
// (woff2 合计 ~2.2MB)。receipt-fonts.css 由 scripts/collect-receipt-fonts.mjs 生成,
// unicode-range 分片让运行时只下载用到的切片; 离线时 font-display: swap 回退系统字体。
import "@/receipt-fonts.css";

export const SITE_URL = "https://ccrouter.app";
export const SITE_LABEL = "ccrouter.app";
export { VERSION };

/** 主题小票组件的统一入参: 真实 DTO + 代理端口 + 调度模式标签 (diner 用)。 */
export interface ThemeSlipProps {
  dto: ReceiptDto;
  port: number;
  sched: string;
}

// ===== 日期 (与 ReceiptSlip 现有 formatPeriod/formatIssued 一致, 统一 UTC) =====

export interface UtcParts {
  Y: number;
  M: string;
  D: string;
  h: string;
  m: string;
  s: string;
  /** 0=Sunday */
  wd: number;
}

const pad2 = (n: number) => String(n).padStart(2, "0");

export function utcParts(ms: number): UtcParts {
  const d = new Date(ms);
  return {
    Y: d.getUTCFullYear(),
    M: pad2(d.getUTCMonth() + 1),
    D: pad2(d.getUTCDate()),
    h: pad2(d.getUTCHours()),
    m: pad2(d.getUTCMinutes()),
    s: pad2(d.getUTCSeconds()),
    wd: d.getUTCDay(),
  };
}

// ===== 数字格式化 (德/法用逗号小数与本地千分位) =====

export const fmtTokEn = fmtCompact;
export const fmtTokDe = (n: number) => fmtCompact(n).replace(".", ",");
export const fmtTokFr = fmtTokDe;
export const fmtCountEn = (n: number) => n.toLocaleString("en-US");
export const fmtCountDe = (n: number) => n.toLocaleString("de-DE");
export const fmtCountFr = (n: number) => n.toLocaleString("fr-FR");

export function totalTokensOf(t: ReceiptTotalsDto): number {
  return t.input_tokens + t.output_tokens + t.cache_creation_tokens + t.cache_read_tokens;
}

export function hasCache(t: ReceiptTotalsDto): boolean {
  return t.cache_creation_tokens > 0 || t.cache_read_tokens > 0;
}

/** 百分比 (0-100), 分母为 0 时返回 0。 */
export function pct(part: number, whole: number): number {
  return whole > 0 ? (part / whole) * 100 : 0;
}

/** "model-fable" → "MODEL-FABLE" */
export function vmDisplay(name: string): string {
  return name.toUpperCase();
}

export function subLabel(s: ReceiptSubItemDto, deletedText: string): string {
  return s.subscription_display_name ?? deletedText;
}

/** 德国板 TSE 签名行: 由单号+时间派生的确定性伪 base64, 纯装饰。 */
export function pseudoSignature(dto: ReceiptDto): string {
  const raw = `${dto.slip_no}:${dto.range_start_ms}:${dto.range_end_ms}:${dto.generated_at_ms}`;
  let sig: string;
  try {
    sig = btoa(raw);
  } catch {
    sig = dto.slip_no.repeat(8);
  }
  return (sig + sig).replace(/[+/=]/g, "x").slice(0, 64);
}

// ===== 视觉基元 =====

const ZIGZAG_TOP =
  "polygon(0 100%, 5% 0, 10% 100%, 15% 0, 20% 100%, 25% 0, 30% 100%, 35% 0, 40% 100%, 45% 0, 50% 100%, 55% 0, 60% 100%, 65% 0, 70% 100%, 75% 0, 80% 100%, 85% 0, 90% 100%, 95% 0, 100% 100%)";
const ZIGZAG_BOTTOM =
  "polygon(0 0, 5% 100%, 10% 0, 15% 100%, 20% 0, 25% 100%, 30% 0, 35% 100%, 40% 0, 45% 100%, 50% 0, 55% 100%, 60% 0, 65% 100%, 70% 0, 75% 100%, 80% 0, 85% 100%, 90% 0, 95% 100%, 100% 0)";

/** 热敏纸壳: 上下锯齿撕边 + 阴影, 宽度固定 360 (与现行小票一致)。 */
export function ZigzagPaper({
  color,
  bodyStyle,
  children,
}: {
  color: string;
  bodyStyle?: CSSProperties;
  children: ReactNode;
}) {
  return (
    <div style={{ width: 360, filter: "drop-shadow(0 8px 18px rgba(0,0,0,0.18))" }}>
      <div style={{ height: 8, background: color, clipPath: ZIGZAG_TOP }} />
      <div style={{ background: color, boxSizing: "border-box", ...bodyStyle }}>{children}</div>
      <div style={{ height: 8, background: color, clipPath: ZIGZAG_BOTTOM }} />
    </div>
  );
}

/** 左右两端对齐的一行; left/right 直接传节点, 字符串会包 span。 */
export function Between({
  left,
  right,
  style,
}: {
  left: ReactNode;
  right?: ReactNode;
  style?: CSSProperties;
}) {
  return (
    <div style={{ display: "flex", justifyContent: "space-between", gap: 6, ...style }}>
      {typeof left === "string" ? <span>{left}</span> : left}
      {right == null || typeof right === "string" ? <span>{right}</span> : right}
    </div>
  );
}

/** 可截断的左侧文本 (长模型名/订阅名), 右侧数值不换行。 */
export function BetweenEllipsis({
  left,
  right,
  style,
  leftStyle,
}: {
  left: ReactNode;
  right: ReactNode;
  style?: CSSProperties;
  leftStyle?: CSSProperties;
}) {
  return (
    <div
      style={{ display: "flex", justifyContent: "space-between", gap: 6, minWidth: 0, ...style }}
    >
      <span
        style={{
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
          minWidth: 0,
          ...leftStyle,
        }}
      >
        {left}
      </span>
      <span style={{ flexShrink: 0 }}>{right}</span>
    </div>
  );
}
