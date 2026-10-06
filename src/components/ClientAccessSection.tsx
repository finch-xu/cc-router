/**
 * Live Routing 的「客户端接入」卡: 哪些客户端真的走过 cc-router (被动流量检测)。
 *
 * 判定来自请求日志按 client_tool 的聚合 (get_client_activity) —— 零配置, 不需要
 * 客户端配合; 代价是只能看到「保留期内发过请求」, 看不到此刻是否在线。日志受
 * log_retention_days 清理, 保留期即天然检测窗口 (卡脚注说明)。
 *
 * 表格按 CLIENT_GROUPS 分 4 组, 组名与路由图左侧 CLIENT 便签同源 (CLIENT_GROUP_LABEL)。
 * 组行是可折叠的手风琴, 展示组级聚合 (任一成员接入即算接入 / 最近请求取组内最新 /
 * 请求数求和); 展开后是逐客户端行。默认展开保留期内有过请求的组, 从未接入的组收起 ——
 * 与便签灰显同一口径。上面的路由图便签与这里共用同一份数据 (useClientActivity)。
 *
 * 接入状态不占独立列: 「未接入」恒等于「— + 0」, 单开一列全是文字却零信息量。改为
 * 名字前一枚状态点 (实心 = 接入), 状态文案退到 title / aria-label。
 */
import { useState } from "react";
import { ChevronRight } from "lucide-react";
import { ClientToolBadge } from "@/components/ClientToolBadge";
import { useClientActivity } from "@/hooks/useClientActivity";
import { useSettings } from "@/hooks/useSettings";
import { fmtCompact, fmtNum, fmtRelativeTime } from "@/lib/format";
import {
  CLIENT_GROUP_LABEL,
  summarizeActivity,
  type ClientGroupKey,
} from "@/lib/clientActivity";
import { useT } from "@/i18n";

/**
 * @param variant "sketch" = 手绘卡的 card + ca-* 类; "classic" = 经典主题的
 *   lrc-flush-section + lrc-ca-* 类。两版 DOM 结构一致, 只有类名与容器不同, 逻辑零复制。
 */
export function ClientAccessSection({ variant }: { variant: "sketch" | "classic" }) {
  const { t } = useT();
  const activity = useClientActivity();
  const settings = useSettings();
  const summary = summarizeActivity(activity.data);
  // 用户手动折叠/展开的覆盖; 没动过的组按「保留期内有没有请求」取默认 (活跃展开, 未接入收起)
  const [toggled, setToggled] = useState<Partial<Record<ClientGroupKey, boolean>>>({});

  const sketch = variant === "sketch";
  const p = sketch ? "ca" : "lrc-ca";

  // 0 = 永久保留; >=36500 天兼容旧版的「永久」选项。
  const days = settings.data?.log_retention_days ?? 0;
  const forever = days === 0 || days >= 36500;

  return (
    <section className={sketch ? "card alt" : "lrc-flush-section"}>
      <div className={sketch ? "card-head" : "lrc-flush-title"}>
        <span className={sketch ? "card-title" : undefined}>
          {t("liveRouting.clientAccess.title")}
        </span>
        <span className={sketch ? "card-sub" : `${p}-sub`}>
          {t("liveRouting.clientAccess.subtitle")}
        </span>
      </div>

      <div className={sketch ? "card-body" : undefined}>
        {summary && !summary.hasAny ? (
          <div className={`${p}-empty`}>{t("liveRouting.clientAccess.empty")}</div>
        ) : (
          <div className={`${p}-list`}>
            <div className={`${p}-row head`}>
              <span>{t("liveRouting.clientAccess.col.client")}</span>
              <span className={`${p}-time`}>{t("liveRouting.clientAccess.col.lastSeen")}</span>
              <span className={`${p}-count-head`}>{t("liveRouting.clientAccess.col.count")}</span>
              <span className={`${p}-count-head`}>{t("liveRouting.clientAccess.col.tokens")}</span>
            </div>
            {summary?.table.map((g) => {
              const open = toggled[g.key] ?? g.active;
              return (
                <div className={`${p}-group`} key={g.key}>
                  <button
                    type="button"
                    className={g.active ? `${p}-row group` : `${p}-row group off`}
                    aria-expanded={open}
                    onClick={() => setToggled((s) => ({ ...s, [g.key]: !open }))}
                  >
                    <span className={`${p}-client`}>
                      {sketch ? (
                        <SketchChevron className={open ? "ca-chev open" : "ca-chev"} />
                      ) : (
                        <ChevronRight
                          className={open ? "lrc-ca-chev open" : "lrc-ca-chev"}
                          size={13}
                          aria-hidden="true"
                        />
                      )}
                      <StatusDot p={p} on={g.active} />
                      <span className={`${p}-gname`}>{CLIENT_GROUP_LABEL[g.key]}</span>
                    </span>
                    <span className={`${p}-time`}>
                      {g.lastSeen === null ? "—" : fmtRelativeTime(g.lastSeen, t)}
                    </span>
                    <span className={`${p}-count`}>{fmtNum(g.count)}</span>
                    <TokenCell p={p} n={g.tokens} />
                  </button>
                  {open &&
                    g.rows.map((r) => (
                      <div
                        className={r.connected ? `${p}-row member` : `${p}-row member off`}
                        key={r.toolId ?? "unknown"}
                      >
                        <span className={`${p}-client`}>
                          <StatusDot p={p} on={r.connected} />
                          <ClientToolBadge toolId={r.toolId} />
                        </span>
                        <span className={`${p}-time`}>
                          {r.lastSeen === null ? "—" : fmtRelativeTime(r.lastSeen, t)}
                        </span>
                        <span className={`${p}-count`}>{fmtNum(r.count)}</span>
                        <TokenCell p={p} n={r.tokens} />
                      </div>
                    ))}
                </div>
              );
            })}
          </div>
        )}

        <div className={`${p}-foot`}>
          {t(
            forever
              ? "liveRouting.clientAccess.retentionForever"
              : "liveRouting.clientAccess.retention",
            { days },
          )}
        </div>
      </div>
    </section>
  );
}

/**
 * 手绘主题的展开箭头。按侧栏图标的 32 画板来画, 才能直接套 #ccr-rough-icon
 * (滤镜参数按引用者的用户坐标生效, 换画板尺寸抖动幅度就跟着变)。
 * 经典 / Win2000 用 lucide 的 ChevronRight, 与其他下拉箭头同一来源。
 */
function SketchChevron({ className }: { className: string }) {
  return (
    <svg
      className={className}
      viewBox="0 0 32 32"
      width="13"
      height="13"
      aria-hidden="true"
      focusable="false"
      style={{ overflow: "visible" }}
    >
      <path filter="url(#ccr-rough-icon)" d="M11.2 6.4 C 15.2 10, 18.4 13.2, 21.2 16 C 18.4 18.8, 15.2 22, 10.8 25.6" />
    </svg>
  );
}

/** token 列: 紧凑单位 (K / M / B) 只为好看, 悬停给精确值 */
function TokenCell({ p, n }: { p: string; n: number }) {
  return (
    <span className={`${p}-count`} title={n >= 1000 ? fmtNum(n) : undefined}>
      {fmtCompact(n)}
    </span>
  );
}

/**
 * 一枚状态点代替整列状态文字: 实心 = 保留期内接过流量, 空心环 = 没有。
 * 信息本身由「最近请求 / 请求数」两列交叉印证 (未接入恒为 — 和 0), 这里只做一眼可辨的
 * 视觉锚点; 原来的 "已接入 / 未接入" 文案退到 title 与 aria-label, 鼠标悬停和读屏都还在。
 */
function StatusDot({ p, on }: { p: string; on: boolean }) {
  const { t } = useT();
  const label = t(
    on ? "liveRouting.clientAccess.connected" : "liveRouting.clientAccess.notConnected",
  );
  return <span className={on ? `${p}-dot on` : `${p}-dot`} role="img" aria-label={label} title={label} />;
}
