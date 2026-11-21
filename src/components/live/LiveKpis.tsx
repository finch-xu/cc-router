import { useMemo } from "react";
import type { InflightAttempt } from "@/hooks/useLiveTraffic";
import { useT } from "@/i18n";
import { buckets, fmtMs, fmtTok, inWindow, isOk, median, totalTokens } from "@/lib/liveStats";
import type { LiveRequest } from "@/types";

const SPARK_BUCKETS = 30;
const MAX_DOTS = 24;

function Spark({ values, floor }: { values: number[]; floor?: number }) {
  const w = 160;
  const h = 26;
  const max = Math.max(...values, 1e-9);
  const lo = floor ?? 0;
  const pts = values.map((v, i) => [
    (i / Math.max(1, values.length - 1)) * w,
    h - 2 - ((Math.max(v, lo) - lo) / (max - lo || 1)) * (h - 4),
  ]);
  const line = pts.map((p, i) => `${i ? "L" : "M"}${p[0].toFixed(1)} ${p[1].toFixed(1)}`).join(" ");
  const last = pts[pts.length - 1];
  return (
    <svg className="lv-spark" viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" aria-hidden>
      <path className="area" d={`${line} L${w} ${h} L0 ${h} Z`} />
      <path className="line" d={line} />
      {last && <circle cx={last[0]} cy={last[1]} r={2.4} />}
    </svg>
  );
}

interface Props {
  rows: readonly LiveRequest[];
  records: readonly LiveRequest[];
  inflight: readonly InflightAttempt[];
  now: number;
  windowMs: number;
}

export function LiveKpis({ rows, records, inflight, now, windowMs }: Props) {
  const { t } = useT();
  const k = useMemo(() => {
    const last60 = inWindow(records, now, 60_000);
    // inWindow 取到末尾, 上一分钟 = 最近两分钟 - 最近一分钟
    const prev60 = inWindow(records, now, 120_000).length - last60.length;
    const bk = buckets(rows, now, windowMs, SPARK_BUCKETS);
    const perMin = 60_000 / (windowMs / SPARK_BUCKETS);
    const ok = rows.filter(isOk);
    const failoverOk = ok.filter((r) => r.retry_count > 0).length;
    return {
      rpm: last60.length,
      rpmDelta: last60.length - prev60,
      rpmSpark: bk.map((b) => b.length * perMin),
      tpm: last60.reduce((s, r) => s + totalTokens(r), 0),
      tpmOut: last60.reduce((s, r) => s + r.output_tokens, 0),
      winTokens: rows.reduce((s, r) => s + totalTokens(r), 0),
      tpmSpark: bk.map((b) => b.reduce((s, r) => s + totalTokens(r), 0) * perMin),
      rate: rows.length ? ok.length / rows.length : null,
      fails: rows.length - ok.length,
      failoverOk,
      okSpark: bk.map((b) => (b.length ? b.filter(isOk).length / b.length : 1)),
      ttft: median(ok.map((r) => r.ttft_ms).filter((x): x is number => x != null)),
      total: median(ok.map((r) => r.total_ms).filter((x): x is number => x != null)),
      ttftSpark: bk.map((b) => median(b.filter(isOk).map((r) => r.ttft_ms ?? 0)) ?? 0),
    };
  }, [rows, records, now, windowMs]);

  return (
    <div className="lv-kpis">
      <div className="lv-kpi focus">
        <div className="l">
          <span className="lv-dot acc" />
          {t("live.kpi.inflight")}
        </div>
        <div className="v">{inflight.length}</div>
        <div className="lv-inflight-dots">
          {inflight.length === 0 ? (
            <span className="idle">{t("live.kpi.idle")}</span>
          ) : (
            inflight.slice(0, MAX_DOTS).map((a) => <i key={a.key} className={a.failover ? "fo" : undefined} />)
          )}
        </div>
      </div>
      <div className="lv-kpi">
        <div className="l">{t("live.kpi.rpm")}</div>
        <div className="v">{k.rpm}</div>
        <div className="s">
          {t("live.kpi.rpmSub", { delta: (k.rpmDelta >= 0 ? "+" : "") + k.rpmDelta, count: rows.length })}
        </div>
        <Spark values={k.rpmSpark} />
      </div>
      <div className="lv-kpi">
        <div className="l">{t("live.kpi.tpm")}</div>
        <div className="v">{fmtTok(k.tpm)}</div>
        <div className="s">{t("live.kpi.tpmSub", { out: fmtTok(k.tpmOut), total: fmtTok(k.winTokens) })}</div>
        <Spark values={k.tpmSpark} />
      </div>
      <div className="lv-kpi">
        <div className="l">{t("live.kpi.success")}</div>
        <div className="v">
          {k.rate == null ? "—" : (k.rate * 100).toFixed(1)}
          {k.rate != null && <small>%</small>}
        </div>
        <div className="s">
          {k.fails ? t("live.kpi.failSub", { fails: k.fails, recovered: k.failoverOk }) : t("live.kpi.noFail")}
        </div>
        <Spark values={k.okSpark} floor={0.9} />
      </div>
      <div className="lv-kpi">
        <div className="l">{t("live.kpi.ttft")}</div>
        <div className="v">
          {k.ttft == null ? "—" : (k.ttft / 1000).toFixed(2)}
          {k.ttft != null && <small>s</small>}
        </div>
        <div className="s">{t("live.kpi.totalSub", { total: fmtMs(k.total) })}</div>
        <Spark values={k.ttftSpark} />
      </div>
    </div>
  );
}
