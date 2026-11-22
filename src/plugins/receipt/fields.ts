/**
 * 小票模板的数据字段表 v1 —— 模板能读到什么, 全在这里。
 * 内部 `ReceiptDto` 改了只改这个映射; 字段表**只增不删** (删改要升 RECEIPT_API)。
 * 时间一律 UTC, 与内置小票一致。零依赖 (社区 CI 用 FIELD_PATHS 检查模板引用的字段)。
 */
import type { CostTotals, ReceiptDto, ReceiptTotalsDto } from "../../types";
import { addCost, EMPTY_COST, equivalentParts, formatMoney, spendParts } from "../../lib/cost";

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

/** 小票页右侧的显示设置, 插件小票同样跟随 (品牌 logo 除外: 插件里没有图片) */
export type ReceiptGroupMode = "virtual_model" | "subscription" | "totals_only";
export interface ReceiptOptions {
  /** 默认数字字段用 K/M 紧凑写法; 关掉则与 *Full 相同 */
  compact: boolean;
  /** 关掉时 hasCache 恒为 false (模板里用 {{#if hasCache}} 包住缓存行即可自动隐藏) */
  showCache: boolean;
  showRequests: boolean;
  groupMode: ReceiptGroupMode;
  /** <ccr-qr> 实际画成什么: 二维码在部分平台分享时会被屏蔽, 默认条形码 */
  code: "qr" | "barcode";
}
export const DEFAULT_RECEIPT_OPTIONS: ReceiptOptions = {
  compact: true,
  showCache: true,
  showRequests: true,
  groupMode: "virtual_model",
  code: "barcode",
};

const ZERO: ReceiptTotalsDto = { request_count: 0, input_tokens: 0, output_tokens: 0, cache_creation_tokens: 0, cache_read_tokens: 0 };
const add = (a: ReceiptTotalsDto, b: ReceiptTotalsDto): ReceiptTotalsDto => ({
  request_count: a.request_count + b.request_count,
  input_tokens: a.input_tokens + b.input_tokens,
  output_tokens: a.output_tokens + b.output_tokens,
  cache_creation_tokens: a.cache_creation_tokens + b.cache_creation_tokens,
  cache_read_tokens: a.cache_read_tokens + b.cache_read_tokens,
});
const sumTokens = (t: ReceiptTotalsDto) => t.input_tokens + t.output_tokens + t.cache_creation_tokens + t.cache_read_tokens;

function totals(t: ReceiptTotalsDto, o: ReceiptOptions = DEFAULT_RECEIPT_OPTIONS) {
  const tokens = sumTokens(t);
  const num = o.compact ? compact : full;
  return {
    requests: full(t.request_count),
    input: num(t.input_tokens),
    output: num(t.output_tokens),
    cacheWrite: num(t.cache_creation_tokens),
    cacheRead: num(t.cache_read_tokens),
    tokens: num(tokens),
    inputFull: full(t.input_tokens),
    outputFull: full(t.output_tokens),
    cacheWriteFull: full(t.cache_creation_tokens),
    cacheReadFull: full(t.cache_read_tokens),
    tokensFull: full(tokens),
    hasCache: o.showCache && (t.cache_creation_tokens > 0 || t.cache_read_tokens > 0),
  };
}

export interface ReceiptContext {
  port: number;
  version: string;
  /** 已删除订阅的显示名 (按界面语言) */
  deletedLabel: string;
}

type Line = { model: string; provider: string; subscription: string; deleted: boolean; t: ReceiptTotalsDto; c: CostTotals };
type Group = { key: string; name: string; lines: Line[]; subtotal: ReceiptTotalsDto; cost: CostTotals };

/** 一行 / 一组的费用文字: 真实花费在前, 包月订阅的「等价按量费用」带 ≈; 没有价格为空串 */
function costText(c: CostTotals): string {
  const spend = spendParts(c).join(" + ");
  const equiv = equivalentParts(c).join(" + ");
  return [spend, equiv && `≈${equiv}`].filter(Boolean).join(" · ");
}

/** 按聚合方式分组。名字段缺省为空字符串 (按订阅合计的行没有模型名, 按模型合计的行没有订阅名) */
function groupsOf(dto: ReceiptDto, ctx: ReceiptContext, mode: ReceiptGroupMode): Group[] {
  const subs = dto.items.flatMap((it) => it.sub_items);
  const subName = (s: (typeof subs)[number]) => s.subscription_display_name ?? ctx.deletedLabel;
  const byTokens = <T extends { t: ReceiptTotalsDto }>(a: T, b: T) => sumTokens(b.t) - sumTokens(a.t);
  if (mode === "virtual_model") {
    return dto.items.map((it) => ({
      key: it.virtual_model_name,
      name: it.virtual_model_name.toUpperCase(),
      subtotal: it.subtotal,
      cost: it.sub_items.reduce((acc, s) => addCost(acc, s.cost), EMPTY_COST),
      lines: it.sub_items.map((s) => ({ model: s.real_model_name, provider: s.provider_display_name, subscription: subName(s), deleted: s.subscription_display_name == null, t: s.totals, c: addCost(EMPTY_COST, s.cost) })),
    }));
  }
  if (mode === "subscription") {
    const map = new Map<string, Group & { models: Map<string, Line> }>();
    for (const s of subs) {
      let g = map.get(s.subscription_id);
      if (!g) {
        g = { key: s.subscription_id, name: subName(s), lines: [], subtotal: ZERO, cost: EMPTY_COST, models: new Map() };
        map.set(s.subscription_id, g);
      }
      g.subtotal = add(g.subtotal, s.totals);
      g.cost = addCost(g.cost, s.cost);
      const l = g.models.get(s.real_model_name);
      if (l) {
        l.t = add(l.t, s.totals);
        l.c = addCost(l.c, s.cost);
      } else g.models.set(s.real_model_name, { model: s.real_model_name, provider: s.provider_display_name, subscription: subName(s), deleted: s.subscription_display_name == null, t: s.totals, c: addCost(EMPTY_COST, s.cost) });
    }
    return [...map.values()]
      .map(({ models, ...g }) => ({ ...g, lines: [...models.values()].sort(byTokens) }))
      .sort((a, b) => sumTokens(b.subtotal) - sumTokens(a.subtotal));
  }
  const bySub = new Map<string, Line>();
  const byModel = new Map<string, Line>();
  for (const s of subs) {
    const a = bySub.get(s.subscription_id);
    if (a) {
      a.t = add(a.t, s.totals);
      a.c = addCost(a.c, s.cost);
    } else bySub.set(s.subscription_id, { model: "", provider: s.provider_display_name, subscription: subName(s), deleted: s.subscription_display_name == null, t: s.totals, c: addCost(EMPTY_COST, s.cost) });
    const b = byModel.get(s.real_model_name);
    if (b) {
      b.t = add(b.t, s.totals);
      b.c = addCost(b.c, s.cost);
    } else byModel.set(s.real_model_name, { model: s.real_model_name, provider: "", subscription: "", deleted: false, t: s.totals, c: addCost(EMPTY_COST, s.cost) });
  }
  const total = subs.reduce((acc, s) => add(acc, s.totals), ZERO);
  const cost = subs.reduce((acc, s) => addCost(acc, s.cost), EMPTY_COST);
  return [
    { key: "by-subscription", name: "BY SUBSCRIPTION", lines: [...bySub.values()].sort(byTokens), subtotal: total, cost },
    { key: "by-model", name: "BY MODEL", lines: [...byModel.values()].sort(byTokens), subtotal: total, cost },
  ];
}

export function receiptData(dto: ReceiptDto, ctx: ReceiptContext, opts: ReceiptOptions = DEFAULT_RECEIPT_OPTIONS) {
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
    options: {
      compact: opts.compact,
      showCache: opts.showCache,
      showRequests: opts.showRequests,
      groupMode: opts.groupMode,
      byVirtualModel: opts.groupMode === "virtual_model",
      bySubscription: opts.groupMode === "subscription",
      totalsOnly: opts.groupMode === "totals_only",
    },
    total: totals(dto.grand_total, opts),
    // 计价插件 (设计 §5): 没有启用的计价插件时 has = false, 模板用 {{#if cost.has}} 包住费用行即可
    cost: costFields(dto.cost_totals),
    groups: groupsOf(dto, ctx, opts.groupMode).map((g) => ({
      key: g.key,
      name: g.name,
      empty: g.lines.length === 0,
      multi: g.lines.length > 1,
      subtotal: totals(g.subtotal, opts),
      subtotalCost: dto.cost_totals ? costText(g.cost) : "",
      lines: g.lines.map(({ t, c, ...l }) => ({ ...l, ...totals(t, opts), cost: dto.cost_totals ? costText(c) : "" })),
    })),
  };
}

/** 顶层 `cost.*`: 两种币种、真实花费与等价按量费用分开; 金额是带币种符号的字符串, 没有为空串 */
function costFields(t: CostTotals | undefined) {
  const c = t ?? EMPTY_COST;
  const money = (micros: number, currency: string) => (micros ? formatMoney(micros, currency) : "");
  const spend = spendParts(c).join(" + ");
  const equivalent = equivalentParts(c).join(" + ");
  return {
    has: t !== undefined,
    spend,
    equivalent,
    hasSpend: spend !== "",
    hasEquivalent: equivalent !== "",
    usd: money(c.usd_micros, "USD"),
    cny: money(c.cny_micros, "CNY"),
    equivUsd: money(c.equiv_usd_micros, "USD"),
    equivCny: money(c.equiv_cny_micros, "CNY"),
  };
}

const COST_KEYS = Object.keys(costFields(undefined));

const TOTAL_KEYS = Object.keys(totals({ request_count: 0, input_tokens: 0, output_tokens: 0, cache_creation_tokens: 0, cache_read_tokens: 0 }));
const STAMP_KEYS = Object.keys(stamp(0));

/** 模板可用的字段路径 (校验用)。`[]` 表示可 each 的数组, 其下字段在循环里直接写短名。 */
export const FIELD_PATHS: { top: string[]; group: string[]; line: string[] } = {
  top: [
    "receipt", "receipt.no", "receipt.range", "receipt.version", "receipt.port", "receipt.site",
    ...["issued", "start", "end"].flatMap((k) => [`receipt.${k}`, ...STAMP_KEYS.map((s) => `receipt.${k}.${s}`)]),
    "total", ...TOTAL_KEYS.map((k) => `total.${k}`),
    "options", ...["compact", "showCache", "showRequests", "groupMode", "byVirtualModel", "bySubscription", "totalsOnly"].map((k) => `options.${k}`),
    "cost", ...COST_KEYS.map((k) => `cost.${k}`),
    "groups",
  ],
  group: ["key", "name", "empty", "multi", "subtotal", ...TOTAL_KEYS.map((k) => `subtotal.${k}`), "subtotalCost", "lines"],
  line: ["model", "provider", "subscription", "deleted", ...TOTAL_KEYS, "cost"],
};

/** 安装预览 / 社区截图用的样例数据 */
export function sampleReceiptDto(): ReceiptDto {
  const t = (r: number, i: number, o: number, cw: number, cr: number): ReceiptTotalsDto => ({
    request_count: r, input_tokens: i, output_tokens: o, cache_creation_tokens: cw, cache_read_tokens: cr,
  });
  const now = Date.UTC(2026, 10, 22, 9, 30, 0);
  // 样例费用 (虚构): 让作者在安装预览里看到 cost.* 字段的效果
  const usd = (micros: number, equivalent = false) => ({ currency: "USD" as const, micros, equivalent, source: { plugin_id: "pricing-sample", version: "1.0.0" } });
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
          { subscription_id: "a", subscription_display_name: "Anthropic Main", provider_id: "anthropic", provider_display_name: "Anthropic", real_model_name: "claude-opus-4-7", totals: t(388, 8_010_000, 1_180_000, 610_000, 30_100_000), cost: usd(42_180_000) },
          { subscription_id: "b", subscription_display_name: "DeepSeek", provider_id: "deepseek", provider_display_name: "DeepSeek", real_model_name: "deepseek-v4", totals: t(24, 410_000, 50_000, 30_000, 1_100_000), cost: usd(260_000) },
        ],
      },
      {
        virtual_model_name: "model-sonnet",
        subtotal: t(96, 1_520_000, 410_000, 0, 0),
        sub_items: [
          { subscription_id: "c", subscription_display_name: undefined, provider_id: "moonshot", provider_display_name: "Moonshot", real_model_name: "kimi-k3", totals: t(96, 1_520_000, 410_000, 0, 0), cost: usd(3_720_000, true) },
        ],
      },
      {
        virtual_model_name: "model-haiku",
        subtotal: t(203, 610_000, 92_000, 0, 2_400_000),
        sub_items: [
          { subscription_id: "a", subscription_display_name: "Anthropic Main", provider_id: "anthropic", provider_display_name: "Anthropic", real_model_name: "claude-haiku-4-5", totals: t(203, 610_000, 92_000, 0, 2_400_000), cost: usd(1_330_000) },
        ],
      },
    ],
    grand_total: t(711, 10_550_000, 1_732_000, 640_000, 33_600_000),
    cost_totals: { usd_micros: 43_770_000, cny_micros: 0, equiv_usd_micros: 3_720_000, equiv_cny_micros: 0 },
  };
}
