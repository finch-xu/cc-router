/**
 * Token 消耗: 时间窗合计 + 缓存命中率 + 四段拆分, 下面按 30 根柱子堆叠 (缓存读 / 缓存写 / 输入 / 输出),
 * 右侧按 用途 / 虚拟模型 / 订阅 / 客户端 分组排行。
 * 四段用同一色相由浅到深 (越「贵」越深), 输出用墨色区分方向。
 */
import { useMemo, useState } from "react";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useT } from "@/i18n";
import {
  buckets,
  cacheHitRatio,
  clientKey,
  clientMeta,
  fmtTok,
  isOk,
  median,
  partsTotal,
  sumTokens,
  type TokenParts,
} from "@/lib/liveStats";
import type { LiveRequest } from "@/types";

const BARS = 30;
const PARTS: { key: keyof TokenParts; cls: string; label: string }[] = [
  { key: "cacheRead", cls: "cr", label: "live.tokens.cacheRead" },
  { key: "cacheWrite", cls: "cw", label: "live.tokens.cacheWrite" },
  { key: "input", cls: "in", label: "live.tokens.input" },
  { key: "output", cls: "out", label: "live.tokens.output" },
];
type GroupBy = "purpose" | "vm" | "sub" | "client";
const GROUPS: GroupBy[] = ["purpose", "vm", "sub", "client"];

function niceCeil(v: number): number {
  if (v <= 0) return 1;
  const p = Math.pow(10, Math.floor(Math.log10(v)));
  for (const m of [1, 2, 2.5, 5, 10]) if (m * p >= v) return m * p;
  return 10 * p;
}

function clock(ms: number) {
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

interface Props {
  rows: readonly LiveRequest[];
  now: number;
  windowMs: number;
  windowLabel: string;
}

export function LiveTokenCard({ rows, now, windowMs, windowLabel }: Props) {
  const { t } = useT();
  const subs = useSubscriptions();
  const [by, setBy] = useState<GroupBy>("purpose");

  const total = useMemo(() => sumTokens(rows), [rows]);
  const all = partsTotal(total);
  const hit = cacheHitRatio(total);
  const ok = rows.filter(isOk);
  const speed = median(
    ok
      .filter((r) => r.output_tokens > 0 && r.total_ms != null)
      .map((r) => r.output_tokens / Math.max(0.2, ((r.total_ms ?? 0) - (r.ttft_ms ?? 0)) / 1000)),
  );

  const bars = useMemo(() => buckets(rows, now, windowMs, BARS).map(sumTokens), [rows, now, windowMs]);
  const top = niceCeil(Math.max(...bars.map(partsTotal), 1));

  const groups = useMemo(() => {
    const subsById = new Map((subs.data ?? []).map((s) => [s.id, s.display_name]));
    const m = new Map<string, LiveRequest[]>();
    for (const r of rows) {
      const k =
        by === "purpose" ? r.purpose.kind : by === "vm" ? r.virtual_model : by === "sub" ? r.subscription_id : clientKey(r.client_tool);
      m.set(k, [...(m.get(k) ?? []), r]);
    }
    return [...m.entries()]
      .map(([k, list]) => {
        const parts = sumTokens(list);
        const name =
          by === "purpose"
            ? t(`live.purpose.${k}`)
            : by === "vm"
              ? k
              : by === "sub"
                ? (subsById.get(k) ?? k.slice(0, 8))
                : clientMeta(k)
                  ? t(clientMeta(k)!.i18nKey)
                  : t("live.otherClient");
        return { k, name, parts, total: partsTotal(parts), mono: by === "vm" };
      })
      .sort((a, b) => b.total - a.total)
      .slice(0, 6);
  }, [rows, by, subs.data, t]);
  const groupTop = groups[0]?.total || 1;

  // 图表几何: viewBox 宽度固定, preserveAspectRatio=none 拉伸; 柱宽按比例
  const W = 600;
  const H = 150;
  const L = 0;
  const step = (W - L) / BARS;
  const bw = Math.max(3, step - 4);
  const axis = [0, 1, 2, 3].map((i) => (i === 3 ? t("live.tokens.now") : clock(now - windowMs * (1 - i / 3))));

  return (
    <section className="card lv-section">
      <div className="card-head">
        <div className="lv-head-lead">
          <span className="card-title">{t("live.tokens.title")}</span>
          <span className="card-sub">{t("live.tokens.sub", { window: windowLabel, bucket: Math.round(windowMs / BARS / 1000) })}</span>
        </div>
      </div>
      <div className="card-body lv-tk-grid">
        <div>
          <div className="lv-tk-top">
            <div className="lv-tk-total">
              <div className="v">
                {fmtTok(all)}
                <small>tokens</small>
              </div>
              <div className="s">
                {ok.length
                  ? t("live.tokens.avg", { avg: fmtTok(all / ok.length), speed: speed == null ? "—" : Math.round(speed) })
                  : "—"}
              </div>
            </div>
            <div className="lv-hit">
              <svg viewBox="0 0 44 44" aria-hidden>
                <circle className="bg" cx="22" cy="22" r="18" />
                <circle className="fg" cx="22" cy="22" r="18" strokeDasharray={`${((hit ?? 0) * 113.1).toFixed(1)} 200`} />
              </svg>
              <div>
                <b>{hit == null ? "—" : `${(hit * 100).toFixed(1)}%`}</b>
                <small>{t("live.tokens.hit")}</small>
              </div>
            </div>
            <div className="lv-tk-parts">
              {PARTS.map((p) => (
                <div key={p.key}>
                  <div className="k">
                    <i className={`sw ${p.cls}`} />
                    {t(p.label)}
                  </div>
                  <b>{fmtTok(total[p.key])}</b>
                </div>
              ))}
            </div>
          </div>
          <div className="lv-tk-chart">
            <div className="lv-tk-y">
              {all > 0 && (
                <>
                  <span>{fmtTok(top)}</span>
                  <span>{fmtTok(top / 2)}</span>
                  <span>0</span>
                </>
              )}
            </div>
            <svg viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" aria-hidden>
              {[0, 0.5, 1].map((f) => (
                <line key={f} className="grid" x1={0} x2={W} y1={H - f * (H - 2)} y2={H - f * (H - 2)} />
              ))}
              {bars.map((b, i) => {
                let y = H;
                const x = L + i * step + (step - bw) / 2;
                return (
                  <g key={i}>
                    {PARTS.map((p) => {
                      const h = (b[p.key] / top) * (H - 2);
                      if (h < 0.3) return null;
                      y -= h;
                      return <rect key={p.key} className={p.cls} x={x} y={y} width={bw} height={Math.max(0.5, h - 0.6)} />;
                    })}
                  </g>
                );
              })}
            </svg>
          </div>
          <div className="lv-tk-axis">
            {axis.map((a, i) => (
              <span key={i}>{a}</span>
            ))}
          </div>
        </div>
        <div className="lv-tk-side">
          <div className="seg-tabs" role="tablist">
            {GROUPS.map((g) => (
              <button key={g} type="button" role="tab" aria-selected={by === g} className={"seg-tab" + (by === g ? " active" : "")} onClick={() => setBy(g)}>
                {t(`live.tokens.by.${g}`)}
              </button>
            ))}
          </div>
          <div className="lv-tk-by">
            {groups.length === 0 && <div className="lv-empty small">{t("live.tokens.empty")}</div>}
            {groups.map((g) => (
              <div className="row" key={g.k}>
                <span className={"n" + (g.mono ? " mono" : "")} title={g.name}>
                  {g.name}
                </span>
                <div className="lv-stack">
                  {PARTS.map((p) => (
                    <i key={p.key} className={p.cls} style={{ width: `${(g.parts[p.key] / groupTop) * 100}%` }} />
                  ))}
                </div>
                <span className="v">{fmtTok(g.total)}</span>
              </div>
            ))}
          </div>
        </div>
      </div>
    </section>
  );
}
