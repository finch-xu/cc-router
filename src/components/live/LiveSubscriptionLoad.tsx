/**
 * 订阅负载: 路由图右列每个订阅一张卡 —— 在途、请求数、token、首字延迟,
 * 24 根请求量小柱 (出过错的柱子标红)、额度进度、绑定了哪些虚拟模型。
 * 排序: 停用的沉底, 其余按在途 → 请求量。
 */
import { useMemo } from "react";
import { Link } from "react-router";
import { ProviderLogo } from "@/components/ProviderLogo";
import { stateLabel } from "@/components/StatusBadge";
import { quotaFraction } from "@/components/live/LiveFlowGraph";
import type { LiveTopology } from "@/components/live/useLiveTopology";
import { useT } from "@/i18n";
import { buckets, fmtTok, isOk, median, totalTokens } from "@/lib/liveStats";
import type { LiveRequest } from "@/types";

const BARS = 24;

interface Props {
  topo: LiveTopology;
  rows: readonly LiveRequest[];
  now: number;
  windowMs: number;
}

export function LiveSubscriptionLoad({ topo, rows, now, windowMs }: Props) {
  const { t } = useT();
  const cards = useMemo(() => {
    const per = new Map<string, LiveRequest[]>();
    for (const r of rows) per.set(r.subscription_id, [...(per.get(r.subscription_id) ?? []), r]);
    return topo.subs
      .map((s) => {
        const list = per.get(s.id) ?? [];
        const ok = list.filter(isOk);
        return {
          ...s,
          list,
          ok,
          tokens: list.reduce((x, r) => x + totalTokens(r), 0),
          ttft: median(ok.map((r) => r.ttft_ms).filter((x): x is number => x != null)),
          bars: buckets(list, now, windowMs, BARS),
          vms: topo.vms.filter((v) => v.subIds.includes(s.id)).map((v) => v.name.replace(/^model-/, "")),
        };
      })
      .sort((a, b) => {
        const off = Number(a.dto ? !a.dto.enabled : 0) - Number(b.dto ? !b.dto.enabled : 0);
        return off || b.list.length - a.list.length;
      });
  }, [topo, rows, now, windowMs]);

  if (cards.length === 0) return null;
  return (
    <section className="card lv-section">
      <div className="card-head">
        <div className="lv-head-lead">
          <span className="card-title">{t("live.load.title")}</span>
          <span className="card-sub">{t("live.load.sub")}</span>
        </div>
      </div>
      <div className="card-body lv-load-grid">
        {cards.map((c) => {
          const sub = c.dto;
          const disabled = sub ? !sub.enabled : false;
          const fails = c.list.length - c.ok.length;
          const mx = Math.max(...c.bars.map((b) => b.length), 1);
          const quota = quotaFraction(sub);
          const left = c.cooling && sub?.cooldown_until ? Math.ceil((sub.cooldown_until - now) / 1000) : 0;
          let pill = (
            <span className="lv-state ok">
              <span className="lv-dot ok" />
              {t("live.load.ok")}
            </span>
          );
          if (disabled) pill = <span className="pill">{t("live.sub.disabled")}</span>;
          else if (c.cooling) pill = <span className="pill warn"><span className="dot" />{t("live.sub.cooling", { state: stateLabel(sub!.state, t), left: `${left}s` })}</span>;
          else if (sub && sub.state !== "healthy") pill = <span className="pill err"><span className="dot" />{stateLabel(sub.state, t)}</span>;
          else if (c.list.length >= 5 && fails / c.list.length > 0.1)
            pill = <span className="pill err"><span className="dot" />{t("live.load.failRate", { pct: Math.round((fails / c.list.length) * 100) })}</span>;
          return (
            <div key={c.id} className={"lv-load" + (c.cooling ? " cool" : "") + (disabled ? " off" : "")}>
              <div className="top">
                <ProviderLogo iconId={sub?.provider_icon} size={28} />
                <div className="t">
                  {sub ? <Link to={`/subscriptions/${sub.id}`}>{sub.display_name}</Link> : <b>{t("live.sub.removed")}</b>}
                  <small>{c.list.at(-1)?.model ?? "—"}</small>
                </div>
                {pill}
              </div>
              <div className="stats">
                <div>{t("live.load.requests")}<b>{c.list.length}</b></div>
                <div>{t("live.load.fails")}<b className={fails ? "err" : undefined}>{fails}</b></div>
                <div>Token<b>{fmtTok(c.tokens)}</b></div>
                <div>{t("live.load.ttft")}<b>{c.ttft == null ? "—" : `${(c.ttft / 1000).toFixed(1)}s`}</b></div>
              </div>
              <div className="bars" aria-hidden>
                {c.bars.map((b, i) => (
                  <i
                    key={i}
                    className={!b.length ? "z" : b.some((r) => !isOk(r)) ? "x" : undefined}
                    style={{ height: `${Math.max(8, (b.length / mx) * 100)}%` }}
                  />
                ))}
              </div>
              <div className="foot">
                {quota ? (
                  <div className="quota">
                    {t(`live.load.period.${quota.period}`)}
                    <div className="lv-qbar">
                      <i className={quota.frac > 0.75 ? "hi" : undefined} style={{ width: `${Math.min(100, quota.frac * 100)}%` }} />
                    </div>
                    <span>{Math.round(quota.frac * 100)}%</span>
                  </div>
                ) : (
                  <span>{t("live.load.noQuota")}</span>
                )}
                {c.vms.length > 0 && <span className="vms">{c.vms.join(" · ")}</span>}
              </div>
            </div>
          );
        })}
      </div>
    </section>
  );
}
