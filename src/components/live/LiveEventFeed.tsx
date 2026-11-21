/**
 * 实时事件: 每行 = 一次上游 attempt, 行组件与请求日志共用 (components/requests/RequestRow)。
 * 这里只给概要: 点开一行看判定依据, 完整信息 (请求头、UA、IP、错误体…) 在请求日志的详情页。
 */
import { useMemo, useRef, useState } from "react";
import { ArrowRight } from "lucide-react";
import { Link } from "react-router";
import { PurposeChips, RequestRow } from "@/components/requests/RequestRow";
import { useLiveSessions } from "@/components/live/useLiveSessions";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useT } from "@/i18n";
import { isOk, matchFilter, type FeedFilter } from "@/lib/liveStats";
import type { LiveRequest } from "@/types";

const MAX_ROWS = 12;
const FILTERS: FeedFilter[] = ["all", "main", "agent", "bg", "fail"];

interface Props {
  rows: readonly LiveRequest[];
  now: number;
  paused: boolean;
}

export function LiveEventFeed({ rows }: Props) {
  const { t } = useT();
  const subs = useSubscriptions();
  const sessions = useLiveSessions();
  const [filter, setFilter] = useState<FeedFilter>("all");
  const [session, setSession] = useState<string>("all");
  const [open, setOpen] = useState<Set<string>>(() => new Set());
  // 首次渲染时已在的行不算「新」, 之后出现的行播一次入场动画
  const seen = useRef<Set<string> | null>(null);

  const subsById = useMemo(() => new Map((subs.data ?? []).map((s) => [s.id, s])), [subs.data]);

  const counts = useMemo(() => {
    const c: Record<FeedFilter, number> = { all: 0, main: 0, agent: 0, bg: 0, fail: 0 };
    for (const r of rows) for (const f of FILTERS) if (matchFilter(r, f)) c[f] += 1;
    return c;
  }, [rows]);

  const sessionIds = useMemo(() => {
    const m = new Map<string, number>();
    for (const r of rows) if (r.session) m.set(r.session.id, r.timestamp);
    return [...m.entries()].sort((a, b) => b[1] - a[1]).slice(0, 6).map(([id]) => id);
  }, [rows]);

  const list = useMemo(() => {
    const out: LiveRequest[] = [];
    for (let i = rows.length - 1; i >= 0 && out.length < MAX_ROWS; i -= 1) {
      const r = rows[i];
      if (!matchFilter(r, filter)) continue;
      if (session !== "all" && r.session?.id !== session) continue;
      out.push(r);
    }
    return out;
  }, [rows, filter, session]);

  /** 失败的 attempt 之后, 同一虚拟模型上重试次数 +1 的那条就是「改走」的下家 */
  const nextSub = (r: LiveRequest): string | undefined => {
    if (isOk(r)) return undefined;
    const i = rows.indexOf(r);
    for (let j = i + 1; j < rows.length && rows[j].timestamp - r.timestamp < 120_000; j += 1) {
      const n = rows[j];
      if (n.virtual_model === r.virtual_model && n.retry_count === r.retry_count + 1 && n.client_tool === r.client_tool) {
        return subsById.get(n.subscription_id)?.display_name ?? n.provider_id;
      }
    }
    return undefined;
  };

  // 第一批出现的行 (含首屏回填) 不算新; 之后才播入场动画。只记最近的 id, 不无限增长
  const fresh = new Set<string>();
  if (seen.current) {
    for (const r of list) if (!seen.current.has(r.id)) fresh.add(r.id);
  }
  if (list.length > 0 || seen.current) {
    seen.current = new Set([...[...(seen.current ?? [])].slice(-200), ...list.map((r) => r.id)]);
  }

  // 「在请求日志里查看」带上当前的筛选: 最近 1 小时 + 用途 + 会话
  const toLogs = `/request-logs?${new URLSearchParams({
    range: "1h",
    ...(filter !== "all" ? { purpose: filter } : {}),
    ...(session !== "all" ? { session } : {}),
  })}`;

  const toggle = (id: string) =>
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  return (
    <section className="card lv-section">
      <div className="card-head">
        <div className="lv-head-lead">
          <span className="card-title">{t("live.feed.title")}</span>
          <span className="card-sub">{t("live.feed.sub")}</span>
        </div>
        <Link className="lv-link" to={toLogs}>
          {t("live.feed.toLogs")} <ArrowRight size={13} />
        </Link>
      </div>
      <div className="card-body">
        <PurposeChips
          filters={FILTERS}
          value={filter}
          counts={counts}
          onChange={setFilter}
          sessions={sessionIds.map((id) => ({ id, label: sessions.get(id)?.project ?? id.slice(0, 8) }))}
          session={session === "all" ? null : session}
          onSession={(id) => setSession(id ?? "all")}
          t={t}
        />
        {list.length === 0 ? (
          <div className="lv-empty">{rows.length ? t("live.feed.emptyFilter") : t("live.feed.empty")}</div>
        ) : (
          <div className="rq-list">
            {list.map((r) => (
              <RequestRow
                key={r.id}
                mode="expand"
                r={r}
                sub={subsById.get(r.subscription_id)}
                session={r.session ? sessions.get(r.session.id) : undefined}
                next={nextSub(r)}
                fresh={fresh.has(r.id)}
                open={open.has(r.id)}
                onActivate={() => toggle(r.id)}
                moreAction={
                  <Link className="btn sm" to={`/request-logs/${r.id}`}>
                    {t("live.feed.openDetail")} <ArrowRight size={12} />
                  </Link>
                }
                t={t}
              />
            ))}
          </div>
        )}
      </div>
    </section>
  );
}
