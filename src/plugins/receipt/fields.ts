/**
 * 小票模板的数据字段表 v1 —— 模板能读到什么, 全在这里。
 * 内部 `ReceiptDto` 改了只改这个映射; 字段表**只增不删** (删改要升 RECEIPT_API)。
 * 时间一律 UTC, 与内置小票一致。零依赖 (社区 CI 用 FIELD_PATHS 检查模板引用的字段)。
 */
import type { ReceiptDto, ReceiptTotalsDto } from "../../types";

export const RECEIPT_API = 1;

const pad2 = (n: number) => String(n).padStart(2, "0");

function compact(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return (n / 1000).toFixed(2) + "K";
  if (n < 1_000_000_000) return (n / 1_000_000).toFixed(2) + "M";
  if (n < 1_000_000_000_000) return (n / 1_000_000_000).toFixed(2) + "B";
  return (n / 1_000_000_000_000).toFixed(2) + "T";
}
const full = (n: number) => n.toLocaleString("en-US");

function stamp(ms: number) {
  const d = new Date(ms);
  const Y = String(d.getUTCFullYear());
  const M = pad2(d.getUTCMonth() + 1);
  const D = pad2(d.getUTCDate());
  const h = pad2(d.getUTCHours());
  const m = pad2(d.getUTCMinutes());
  const s = pad2(d.getUTCSeconds());
  return { Y, YY: Y.slice(2), M, D, h, m, s, date: `${Y}-${M}-${D}`, time: `${h}:${m}`, weekday: ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"][d.getUTCDay()] };
}

function totals(t: ReceiptTotalsDto) {
  const tokens = t.input_tokens + t.output_tokens + t.cache_creation_tokens + t.cache_read_tokens;
  return {
    requests: full(t.request_count),
    input: compact(t.input_tokens),
    output: compact(t.output_tokens),
    cacheWrite: compact(t.cache_creation_tokens),
    cacheRead: compact(t.cache_read_tokens),
    tokens: compact(tokens),
    inputFull: full(t.input_tokens),
    outputFull: full(t.output_tokens),
    cacheWriteFull: full(t.cache_creation_tokens),
    cacheReadFull: full(t.cache_read_tokens),
    tokensFull: full(tokens),
    hasCache: t.cache_creation_tokens > 0 || t.cache_read_tokens > 0,
  };
}

export interface ReceiptContext {
  port: number;
  version: string;
  /** 已删除订阅的显示名 (按界面语言) */
  deletedLabel: string;
}

export function receiptData(dto: ReceiptDto, ctx: ReceiptContext) {
  return {
    receipt: {
      no: dto.slip_no,
      range: dto.range,
      issued: stamp(dto.generated_at_ms),
      start: stamp(dto.range_start_ms),
      end: stamp(dto.range_end_ms),
      version: ctx.version,
      port: String(ctx.port),
      site: "ccrouter.app",
    },
    total: totals(dto.grand_total),
    groups: dto.items.map((it) => ({
      key: it.virtual_model_name,
      name: it.virtual_model_name.toUpperCase(),
      empty: it.sub_items.length === 0,
      multi: it.sub_items.length > 1,
      subtotal: totals(it.subtotal),
      lines: it.sub_items.map((s) => ({
        model: s.real_model_name,
        provider: s.provider_display_name,
        subscription: s.subscription_display_name ?? ctx.deletedLabel,
        deleted: s.subscription_display_name == null,
        ...totals(s.totals),
      })),
    })),
  };
}

const TOTAL_KEYS = Object.keys(totals({ request_count: 0, input_tokens: 0, output_tokens: 0, cache_creation_tokens: 0, cache_read_tokens: 0 }));
const STAMP_KEYS = Object.keys(stamp(0));

/** 模板可用的字段路径 (校验用)。`[]` 表示可 each 的数组, 其下字段在循环里直接写短名。 */
export const FIELD_PATHS: { top: string[]; group: string[]; line: string[] } = {
  top: [
    "receipt", "receipt.no", "receipt.range", "receipt.version", "receipt.port", "receipt.site",
    ...["issued", "start", "end"].flatMap((k) => [`receipt.${k}`, ...STAMP_KEYS.map((s) => `receipt.${k}.${s}`)]),
    "total", ...TOTAL_KEYS.map((k) => `total.${k}`),
    "groups",
  ],
  group: ["key", "name", "empty", "multi", "subtotal", ...TOTAL_KEYS.map((k) => `subtotal.${k}`), "lines"],
  line: ["model", "provider", "subscription", "deleted", ...TOTAL_KEYS],
};

/** 安装预览 / 社区截图用的样例数据 */
export function sampleReceiptDto(): ReceiptDto {
  const t = (r: number, i: number, o: number, cw: number, cr: number): ReceiptTotalsDto => ({
    request_count: r, input_tokens: i, output_tokens: o, cache_creation_tokens: cw, cache_read_tokens: cr,
  });
  const now = Date.UTC(2026, 10, 22, 9, 30, 0);
  return {
    range: "last7_days",
    range_start_ms: now - 7 * 86400_000,
    range_end_ms: now,
    generated_at_ms: now,
    slip_no: "5A7C0E21",
    items: [
      { virtual_model_name: "model-fable", subtotal: t(0, 0, 0, 0, 0), sub_items: [] },
      {
        virtual_model_name: "model-opus",
        subtotal: t(412, 8_420_000, 1_230_000, 640_000, 31_200_000),
        sub_items: [
          { subscription_id: "a", subscription_display_name: "Anthropic Main", provider_id: "anthropic", provider_display_name: "Anthropic", real_model_name: "claude-opus-4-7", totals: t(388, 8_010_000, 1_180_000, 610_000, 30_100_000) },
          { subscription_id: "b", subscription_display_name: "DeepSeek", provider_id: "deepseek", provider_display_name: "DeepSeek", real_model_name: "deepseek-v4", totals: t(24, 410_000, 50_000, 30_000, 1_100_000) },
        ],
      },
      {
        virtual_model_name: "model-sonnet",
        subtotal: t(96, 1_520_000, 410_000, 0, 0),
        sub_items: [
          { subscription_id: "c", subscription_display_name: undefined, provider_id: "moonshot", provider_display_name: "Moonshot", real_model_name: "kimi-k3", totals: t(96, 1_520_000, 410_000, 0, 0) },
        ],
      },
      {
        virtual_model_name: "model-haiku",
        subtotal: t(203, 610_000, 92_000, 0, 2_400_000),
        sub_items: [
          { subscription_id: "a", subscription_display_name: "Anthropic Main", provider_id: "anthropic", provider_display_name: "Anthropic", real_model_name: "claude-haiku-4-5", totals: t(203, 610_000, 92_000, 0, 2_400_000) },
        ],
      },
    ],
    grand_total: t(711, 10_550_000, 1_732_000, 640_000, 33_600_000),
  };
}
