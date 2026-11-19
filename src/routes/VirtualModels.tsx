import { CustomVirtualModels } from "@/components/virtual-models/CustomVirtualModels";
import { useMemo, useState } from "react";
import { Check, Copy, Search } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { StatusBadge } from "@/components/StatusBadge";
import { ProviderLogo } from "@/components/ProviderLogo";
import { StarPrompt } from "@/components/StarPrompt";
import { ViewTimeBar } from "@/components/schedule/ViewTimeBar";
import { VmRouteMap } from "@/components/schedule/VmRouteMap";
import { buildRouteMap, pageModelColors, type RouteMapData } from "@/components/schedule/routeMap";
import { useHostClock } from "@/hooks/useHostClock";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useProxyStatus } from "@/hooks/useSettings";
import { useVirtualModels, useUpdateVirtualModel } from "@/hooks/useVirtualModels";
import { isAnthropicPassthrough } from "@/lib/authTypes";
import type { ModelColor } from "@/lib/modelColor";
import { hhmm, isNowSelection, type WallClock } from "@/lib/slotSchedule";
import { slotNow, slotNowText } from "@/lib/slotNow";
import { customProviderLabel } from "@/lib/providerLabels";
import { providerName as localProviderName } from "@/lib/providerText";
import { VM_META, VM_ORDER, isJev, jevModelOf, vmAccepts, vmNameToSlot } from "@/lib/virtualModels";
import { runtime } from "@/runtime";
import { useT, type TFunction } from "@/i18n";
import type {
  RoutingMode,
  SubscriptionDto,
  SubscriptionSlot,
  VirtualModelDto,
  VirtualModelName,
} from "@/types";

const MODES: RoutingMode[] = ["sequential", "round_robin", "sticky"];

export function VirtualModelsPage() {
  const { t } = useT();
  const [custom, setCustom] = useState(false);
  const subs = useSubscriptions();
  const vms = useVirtualModels();
  const clock = useHostClock();
  // null = 跟随宿主机此刻; 选过星期 / 滑块后固定在所选时刻
  const [selected, setSelected] = useState<WallClock | null>(null);
  const isNow = isNowSelection(selected, clock ?? null);
  const base = selected ?? clock;
  const weekday = base?.weekday;
  const minute = base?.minute;
  // 只在星期 / 分钟变化时换新对象: 路由图按 at 记忆化, 宿主机时钟每 30 秒的重渲染不必重算
  const at = useMemo<WallClock | null>(
    () => (weekday === undefined || minute === undefined ? null : { weekday, minute }),
    [weekday, minute],
  );

  const subsMap = useMemo(() => {
    const m = new Map<string, SubscriptionDto>();
    subs.data?.forEach((s) => m.set(s.id, s));
    return m;
  }, [subs.data]);
  // 订阅列表没到时 subsMap 为空, 此时建图会把每个模型都画成「没有可用订阅」: 当作加载中
  const subsReady = subs.data !== undefined;

  const orderedVms = useMemo<VirtualModelDto[]>(
    () =>
      VM_ORDER.map((name) => vms.data?.find((v) => v.name === name)).filter(
        (v): v is VirtualModelDto => v !== undefined,
      ),
    [vms.data],
  );
  // 整页一张配色表: 同一模型在所有卡片里同色
  const colors = useMemo(() => pageModelColors(orderedVms, subsMap), [orderedVms, subsMap]);

  return (
    <>
      <div className="page-header">
        <h1>{t("virtualModels.title")}</h1>
        {!custom && <div className="subtitle">{t("virtualModels.subtitle")}</div>}
      </div>

      <div className="radio-group mb-4 w-fit" role="group" aria-label={t("virtualModels.title")}>
        <button type="button" className={!custom ? "on" : ""} aria-pressed={!custom}
          onClick={() => setCustom(false)}>{t("customModels.builtIn")}</button>
        <button type="button" className={custom ? "on" : ""} aria-pressed={custom}
          onClick={() => setCustom(true)}>{t("customModels.custom")}</button>
      </div>

      {custom ? <CustomVirtualModels subscriptions={subs.data ?? []} /> : <>
        <ViewTimeBar at={at} isNow={isNow} onPick={setSelected} onNow={() => setSelected(null)} />

        <div className="slot-grid">
          {orderedVms.map((vm) => (
            <VirtualModelCard
              key={vm.name}
              vm={vm}
              subsMap={subsMap}
              allSubs={subs.data ?? []}
              at={subsReady ? at : null}
              isNow={isNow}
              colors={colors}
            />
          ))}
        </div>

        <RouteLegend />
      </>}

      <StarPrompt className="page-end" />
    </>
  );
}

/** 卡片头部一句话: 所选时刻请求会去哪。 */
function summaryText(summary: RouteMapData["summary"], t: TFunction): string {
  switch (summary.kind) {
    case "none":
      return t("schedule.map.summary.none");
    case "first":
      // fixed 行透传时没有模型名
      return t("schedule.map.summary.first", {
        sub: summary.sub ?? "",
        model: summary.model ?? t("schedule.map.passthrough"),
      });
    case "rr":
      return t("schedule.map.summary.rr", { n: summary.n ?? 0 });
    case "sticky":
      return t("schedule.map.summary.sticky", { n: summary.n ?? 0 });
  }
}

/** 一周缺口的例子:「周六 08:00–24:00」。 */
function gapExample(gap: NonNullable<RouteMapData["firstGap"]>, t: TFunction): string {
  const day = t("schedule.day.one", { d: t(`schedule.wd.${gap.weekday}`) });
  return `${day} ${hhmm(gap.from * 30)}–${hhmm(gap.to * 30)}`;
}

function VirtualModelCard({
  vm,
  subsMap,
  allSubs,
  at,
  isNow,
  colors,
}: {
  vm: VirtualModelDto;
  subsMap: Map<string, SubscriptionDto>;
  allSubs: SubscriptionDto[];
  /** 所选时刻; null = 宿主机时钟或订阅列表还没取到 (显示加载中) */
  at: WallClock | null;
  isNow: boolean;
  colors: Map<string, ModelColor>;
}) {
  const { t } = useT();
  const updateMut = useUpdateVirtualModel();
  const [pickerOpen, setPickerOpen] = useState(false);
  const meta = VM_META[vm.name];

  function update(mode: RoutingMode, subscription_ids: string[]) {
    updateMut.mutate({ name: vm.name, input: { mode, subscription_ids } });
  }
  function onReorder(ids: string[]) {
    update(vm.mode, ids);
  }
  function onRemove(id: string) {
    update(vm.mode, vm.subscription_ids.filter((x) => x !== id));
  }
  function addSubs(ids: string[]) {
    const existing = new Set(vm.subscription_ids);
    const merged = [...vm.subscription_ids, ...ids.filter((id) => !existing.has(id))];
    update(vm.mode, merged);
  }

  const slot = vmNameToSlot(vm.name);
  const modeHint =
    vm.mode === "round_robin"
      ? t("virtualModels.mode.roundRobinHint")
      : vm.mode === "sticky"
        ? t("virtualModels.mode.stickyHint")
        : t("virtualModels.mode.sequentialHint");
  // fallback 是第 5 个虚拟模型, 语义上与 4 个槽位并列而非同级, 投影换色区分
  const isFallback = vm.name === "model-fallback";
  const jev = isJev(vm.name);
  // 图数据只算一次: 头部摘要 / 缺口提示与路由图共用
  const data = useMemo(() => (at ? buildRouteMap(vm, subsMap, at, isNow) : null), [vm, subsMap, at, isNow]);
  const cardClass = jev ? "slot-card wide jev" : isFallback ? "slot-card wide" : "slot-card";
  // 还没绑任何订阅是待配置而非故障: 中性提示, 不报红 / 不提 503 缺口
  const unbound = vm.subscription_ids.length === 0;
  const summaryTone = !data
    ? ""
    : unbound
      ? "empty"
      : data.summary.kind === "none"
        ? "none"
        : data.summary.kind === "first"
          ? "first"
          : "pool";

  return (
    <div className={cardClass}>
      {jev && (
        <div className="jev-hangtag" aria-hidden="true">
          <span className="jev-hangtag-string" />
          <span className="jev-hangtag-tag">
            JEV
            <small>{t("virtualModels.jev.tagSub")}</small>
          </span>
        </div>
      )}
      <div className="slot-head compact vm-card-head">
        <div style={{ minWidth: 0 }}>
          <span className="slot-name">{vm.name}</span>
          <span className="slot-purpose">
            <strong>{t(meta.purposeKey)}</strong> · {t(meta.purposeEnKey)}
          </span>
        </div>
        <div className="radio-group sm" role="group" aria-label={t("virtualModels.mode.label")} title={modeHint}>
          {MODES.map((m) => (
            <button
              key={m}
              className={vm.mode === m ? "on" : ""}
              aria-pressed={vm.mode === m}
              onClick={() => update(m, vm.subscription_ids)}
              type="button"
            >
              {t(`vm.mode.${m}`)}
            </button>
          ))}
        </div>
        {data && (
          <span className={`vm-summary ${summaryTone}`}>
            {unbound ? t("schedule.map.summary.unbound") : summaryText(data.summary, t)}
          </span>
        )}
      </div>

      {data && !unbound && data.gapHours > 0 && data.firstGap && (
        <div className="vm-gap">
          {t("schedule.map.gap", { n: data.gapHours, example: gapExample(data.firstGap, t), vm: vm.name })}
        </div>
      )}

      {at && data ? (
        <VmRouteMap
          vm={vm}
          data={data}
          at={at}
          colors={colors}
          onReorder={onReorder}
          onRemove={onRemove}
          onAdd={() => setPickerOpen(true)}
        />
      ) : (
        <div className="field-hint">{t("common.loading")}</div>
      )}
      {jev && <JevAccessHint />}

      <AddSubscriptionDialog
        open={pickerOpen}
        onOpenChange={setPickerOpen}
        existingIds={vm.subscription_ids}
        allSubs={allSubs}
        vmName={vm.name}
        slot={slot}
        isFallback={isFallback}
        onConfirm={(ids) => {
          addSubs(ids);
          setPickerOpen(false);
        }}
      />
    </div>
  );
}

/** 页面底部的线条图例 (与 VmRouteMap 的 .vrm-edge 同一套线型)。 */
function RouteLegend() {
  const { t } = useT();
  const items: [string, string][] = [
    ["first", "schedule.map.legend.first"],
    ["backup", "schedule.map.legend.backup"],
    ["join", "schedule.map.legend.join"],
    ["off", "schedule.map.legend.off"],
  ];
  return (
    <div className="vrm-legend" role="list" aria-label={t("schedule.map.legend.aria")}>
      {items.map(([tone, key]) => (
        <span key={tone} className="vrm-legend-item" role="listitem">
          <span className={`vrm-legend-line ${tone}`} aria-hidden />
          {t(key)}
        </span>
      ))}
    </div>
  );
}

/** 候选多于这个数才显示搜索框 */
const PICKER_SEARCH_THRESHOLD = 6;

/** base_url → 主机名; 解析失败原样返回。用来区分同一厂商下接了不同地址的订阅 */
function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

function AddSubscriptionDialog({
  open,
  onOpenChange,
  existingIds,
  allSubs,
  vmName,
  slot,
  isFallback,
  onConfirm,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  existingIds: string[];
  allSubs: SubscriptionDto[];
  vmName: VirtualModelName;
  /** null = fallback 卡片, 订阅走兜底槽或原样透传 */
  slot: SubscriptionSlot | null;
  /** fallback 卡片打开时 true: 对「翻译类且未配兜底槽」的候选显示提示 (不禁选) */
  isFallback: boolean;
  onConfirm: (ids: string[]) => void;
}) {
  const { t, locale } = useT();
  const clock = useHostClock();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [query, setQuery] = useState("");
  const candidates = allSubs.filter(
    (s) => s.enabled && !existingIds.includes(s.id) && vmAccepts(vmName, s.endpoint_protocol),
  );

  const providerName = (sub: SubscriptionDto) =>
    customProviderLabel(sub.provider_id, t) ?? localProviderName(sub, locale);
  // 与路由图同一套规则: 加进来之后这条订阅在本卡片里此刻实际会用的模型
  const modelFor = (sub: SubscriptionDto) => {
    if (isJev(vmName)) return jevModelOf(sub) ?? t("sortableSub.passthrough");
    if (slot !== null) return slotNowText(slotNow(sub, slot, clock), t);
    return sub.model_slots.fallback?.trim() || t("sortableSub.passthrough");
  };

  const q = query.trim().toLowerCase();
  const visible = q
    ? candidates.filter((sub) =>
        [sub.display_name, providerName(sub), hostOf(sub.base_url), modelFor(sub), sub.oauth_account?.email ?? ""]
          .join(" ")
          .toLowerCase()
          .includes(q),
      )
    : candidates;

  function toggle(id: string) {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setSelected(next);
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(v) => {
        if (!v) {
          setSelected(new Set());
          setQuery("");
        }
        onOpenChange(v);
      }}
    >
      <DialogContent className="cc-dialog" style={{ maxWidth: 680, width: "92vw" }}>
        <DialogHeader>
          <DialogTitle>{t("virtualModels.dialog.title")}</DialogTitle>
          <div className="pick-target">
            {t("virtualModels.dialog.target")} <span className="mono">{vmName}</span>
          </div>
        </DialogHeader>
        {candidates.length === 0 ? (
          <div className="field-hint">
            {isJev(vmName) ? t("virtualModels.dialog.emptyJev") : t("virtualModels.dialog.empty")}
          </div>
        ) : (
          <>
            {candidates.length > PICKER_SEARCH_THRESHOLD && (
              <label className="pick-search">
                <Search size={14} aria-hidden />
                <input
                  className="input"
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  placeholder={t("virtualModels.dialog.search")}
                  aria-label={t("virtualModels.dialog.search")}
                />
              </label>
            )}
            <div className="pick-list">
              {visible.map((sub) => {
                const on = selected.has(sub.id);
                const host = hostOf(sub.base_url);
                const needFallback =
                  isFallback && !sub.model_slots.fallback?.trim() && !isAnthropicPassthrough(sub.auth_type);
                return (
                  <label key={sub.id} className={on ? "pick-row on" : "pick-row"}>
                    <input
                      type="checkbox"
                      className="pick-check-input"
                      checked={on}
                      onChange={() => toggle(sub.id)}
                    />
                    <span className="pick-check" aria-hidden>
                      {on && <Check size={12} strokeWidth={3} />}
                    </span>
                    <ProviderLogo iconId={sub.provider_icon} size={32} iconSize={20} />
                    <span className="pick-main">
                      <span className="pick-name">
                        {sub.display_name}
                        <StatusBadge state={sub.state} />
                        {needFallback && (
                          <span className="pill warn">{t("virtualModels.dialog.needFallbackSlot")}</span>
                        )}
                      </span>
                      <span className="pick-meta">
                        {providerName(sub)}
                        <span className="pick-sep">·</span>
                        <span className="mono">{sub.oauth_account?.email ?? host}</span>
                      </span>
                    </span>
                    <span className="pick-side">
                      <span className="pick-label">{t("virtualModels.dialog.slotModel")}</span>
                      <span className="pick-model mono" title={modelFor(sub)}>
                        {modelFor(sub)}
                      </span>
                      <span className="pick-used">
                        {sub.referenced_by.length > 0 ? (
                          <>
                            {t("virtualModels.dialog.usedBy")}{" "}
                            {sub.referenced_by.map((n) => (
                              <span key={n} className="pill tag mono">
                                {n.replace("model-", "")}
                              </span>
                            ))}
                          </>
                        ) : (
                          t("virtualModels.dialog.unused")
                        )}
                      </span>
                    </span>
                  </label>
                );
              })}
              {visible.length === 0 && <div className="field-hint">{t("virtualModels.dialog.noMatch")}</div>}
            </div>
          </>
        )}
        <DialogFooter className="pick-footer">
          {selected.size > 0 && (
            <span className="pick-count">{t("virtualModels.dialog.selectedCount", { count: selected.size })}</span>
          )}
          <button className="btn" onClick={() => onOpenChange(false)} type="button">
            {t("common.cancel")}
          </button>
          <button
            className="btn primary"
            disabled={selected.size === 0}
            onClick={() => onConfirm(Array.from(selected))}
            type="button"
          >
            {t("virtualModels.dialog.add")}
          </button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/**
 * jev 卡片底部的接入示例: 调用方是用户自己的代码, 没有 CC 那样现成的环境变量片段。
 * model 写虚拟名 model-jev, 后端按每条订阅的 Jev 槽 / 端点示例模型改写, 所以对任何上游都能直接用。
 */
function JevAccessHint() {
  const { t } = useT();
  const status = useProxyStatus();
  const [copied, setCopied] = useState(false);
  const base = status.data?.base_url ?? "http://127.0.0.1:23456";
  const curl = [
    `curl ${base}/v1/systemone \\`,
    `  -H "Content-Type: application/json" \\`,
    `  -H "Authorization: Bearer <cc-router token>" \\`,
    `  -d '{"model":"model-jev","state":"Hello World","questions":{"says_hello":{"type":"noul","instructions":"Does the state text contain a greeting?"}}}'`,
  ].join("\n");
  return (
    <div className="jev-access">
      <b>{t("virtualModels.jev.access")}</b>
      <span className="mono">{base}/v1/systemone</span>
      <span>{t("virtualModels.jev.keyHint")}</span>
      <button
        type="button"
        className="jev-copy"
        onClick={() => {
          runtime
            .copyText(curl)
            .then(() => {
              setCopied(true);
              setTimeout(() => setCopied(false), 1500);
            })
            .catch(() => {});
        }}
      >
        <Copy size={11} /> {copied ? t("virtualModels.jev.copied") : t("virtualModels.jev.copyCurl")}
      </button>
    </div>
  );
}
