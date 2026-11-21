import { useEffect, useMemo, useRef, useState } from "react";
import { Pause, Play } from "lucide-react";
import { LiveSessionSection } from "@/components/LiveSessionSection";
import { LiveKpis } from "@/components/live/LiveKpis";
import { LiveFlowGraph } from "@/components/live/LiveFlowGraph";
import { LiveEventFeed } from "@/components/live/LiveEventFeed";
import { LiveTokenCard } from "@/components/live/LiveTokenCard";
import { LiveSubscriptionLoad } from "@/components/live/LiveSubscriptionLoad";
import { useLiveTopology } from "@/components/live/useLiveTopology";
import "@/components/live/live-routing.css";
import { useLiveTraffic, type InflightAttempt } from "@/hooks/useLiveTraffic";
import { useTheme } from "@/hooks/useTheme";
import { useT } from "@/i18n";
import { inWindow } from "@/lib/liveStats";
import { isPlainBased } from "@/themes";
import type { LiveRequest } from "@/types";

const WINDOWS = [5, 15, 60] as const;
type WindowMin = (typeof WINDOWS)[number];

/** 1 秒一跳的时钟: 时间窗随时间滑动、冷却倒计时要走, 没有新事件也得重算 */
function useNow(paused: boolean) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (paused) return;
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [paused]);
  return now;
}

export function LiveRoutingPage() {
  const { t } = useT();
  const { art } = useTheme();
  const live = useLiveTraffic();
  const [windowMin, setWindowMin] = useState<WindowMin>(15);
  const [paused, setPaused] = useState(false);
  const now = useNow(paused);

  // 暂停 = 冻结画面: 记下暂停那一刻的数据, 新事件照常进 store, 只累计条数
  const frozen = useRef<{ records: LiveRequest[]; inflight: InflightAttempt[]; version: number; at: number } | null>(null);
  if (paused && !frozen.current) {
    frozen.current = { records: [...live.records], inflight: [...live.inflight.values()], version: live.version, at: now };
  }
  if (!paused) frozen.current = null;
  const records = frozen.current?.records ?? live.records;
  const inflight = useMemo(
    () => frozen.current?.inflight ?? [...live.inflight.values()],
    [live.version, paused],
  );
  const viewNow = frozen.current?.at ?? now;
  const queued = paused ? live.records.length - (frozen.current?.records.length ?? 0) : 0;

  const windowMs = windowMin * 60_000;
  const rows = useMemo(() => inWindow(records, viewNow, windowMs), [records, viewNow, windowMs, live.version]);
  const topo = useLiveTopology(rows, inflight);

  const windowLabel = t(`live.window.${windowMin}`);
  const coolingCount = topo.subs.filter((s) => s.cooling).length;

  return (
    <div className="lv-page">
      <div className="page-actions">
        <div className="page-header" style={{ marginBottom: 0 }}>
          <h1>{t("liveRouting.title")}</h1>
          <div className="subtitle">
            {t("live.subtitle", {
              window: windowLabel,
              clients: topo.clients.length,
              subs: new Set(rows.map((r) => r.subscription_id)).size,
              cooling: coolingCount,
            })}
          </div>
        </div>
        <div className="lv-actions">
          <span className={"lv-live-tag" + (paused ? " paused" : "")}>
            <span className="lv-live-dot" />
            {paused ? t("live.paused") : t("live.live")}
            {queued > 0 && <span className="lv-queued">+{queued}</span>}
          </span>
          <div className="range-tabs" role="tablist" aria-label={t("live.windowLabel")}>
            {WINDOWS.map((w) => (
              <button
                key={w}
                type="button"
                role="tab"
                aria-selected={windowMin === w}
                className={"range-tab" + (windowMin === w ? " active" : "")}
                onClick={() => setWindowMin(w)}
              >
                {t(`live.window.${w}`)}
              </button>
            ))}
          </div>
          <button type="button" className="btn sm" onClick={() => setPaused((p) => !p)} title={t("live.pauseHint")}>
            {paused ? <Play size={13} /> : <Pause size={13} />}
            {paused ? t("live.resume") : t("live.pause")}
          </button>
        </div>
      </div>

      <section className="card lv-hero">
        <LiveKpis rows={rows} records={records} inflight={inflight} now={viewNow} windowMs={windowMs} />
        <LiveFlowGraph topo={topo} rows={rows} inflight={inflight} now={viewNow} paused={paused} />
      </section>

      <LiveEventFeed rows={rows} now={viewNow} paused={paused} />
      <LiveTokenCard rows={rows} now={viewNow} windowMs={windowMs} windowLabel={windowLabel} />
      <LiveSubscriptionLoad topo={topo} rows={rows} now={viewNow} windowMs={windowMs} />

      <LiveSessionSection variant={isPlainBased(art) ? "classic" : "sketch"} />
    </div>
  );
}
