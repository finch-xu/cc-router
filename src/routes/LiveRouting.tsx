import { CustomModelMappings } from "@/components/virtual-models/CustomModelMappings";
import { useMemo, useState } from "react";
import { useNavigate } from "react-router";
import { ArrowRight, Check, Copy, Lock } from "lucide-react";
import { runtime } from "@/runtime";
import { RouteFlowDiagram } from "@/components/RouteFlowDiagram";
import { ClientAccessSection } from "@/components/ClientAccessSection";
import { ProviderLogo } from "@/components/ProviderLogo";
import { useProxyStatus, useSettings } from "@/hooks/useSettings";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useVirtualModels } from "@/hooks/useVirtualModels";
import { isAnthropicPassthrough } from "@/lib/authTypes";
import { slotNow, slotNowText } from "@/lib/slotNow";
import { useHostClock } from "@/hooks/useHostClock";
import { MODE_LABEL_KEY, VM_ORDER, jevModelOf, vmNameToSlot } from "@/lib/virtualModels";
import { useT } from "@/i18n";
import { API_ROUTES, CLIENT_ALIASES } from "@/lib/liveRouting";
import { useTheme } from "@/hooks/useTheme";
import { isPlainBased } from "@/themes";
import { ClassicLiveRoutingPage } from "./LiveRoutingClassic";
import type { SubscriptionDto, VirtualModelDto } from "@/types";

/** 实时路由页按画风整页切换: 经典 / Win2000 = 通栏布局 (LiveRoutingClassic), 手绘 = 卡片 + 速写路由图 */
export function LiveRoutingPage() {
  const { art } = useTheme();
  return isPlainBased(art) ? <ClassicLiveRoutingPage /> : <SketchLiveRoutingPage />;
}

function SketchLiveRoutingPage() {
  const { t } = useT();
  return (
    <div className="lr-page">
      <div className="page-actions">
        <div className="page-header" style={{ marginBottom: 0 }}>
          <h1>{t("liveRouting.title")}</h1>
          <div className="subtitle">{t("liveRouting.subtitle")}</div>
        </div>
        {/* 手写批注: 指向下方路由图里流动的连线 */}
        <div className="hand-note" aria-hidden="true">
          <span>{t("liveRouting.handNote")}</span>
          <svg viewBox="0 0 40 40" width="32" height="32">
            <path d="M6 6 C 22 6, 32 14, 30 32 M23 26 L30 33 L36 25" />
          </svg>
        </div>
      </div>
      <RouteFlowDiagram />
      <ClientAccessSection variant="sketch" />
      <AccessSection />
      <MappingSection />
      <CustomModelMappings />
    </div>
  );
}

/* ============================================================
 * 接入信息 + API 入口 (并排两张卡片)
 * ============================================================ */

function AccessSection() {
  const { t } = useT();
  const navigate = useNavigate();
  const proxy = useProxyStatus();
  const settings = useSettings();

  const host = proxy.data?.listen_all ? "0.0.0.0" : "127.0.0.1";
  const baseUrl = proxy.data?.base_url ?? "";
  // base_url 由后端 AppState::local_base_url 决定, 优先直接用它;
  // 另一种协议(双开时的那条)后端没给, 才按真实端口拼。
  const httpUrl = baseUrl.startsWith("http://")
    ? baseUrl
    : proxy.data?.http_port
      ? `http://${host}:${proxy.data.http_port}`
      : null;
  const httpsUrl = baseUrl.startsWith("https://")
    ? baseUrl
    : proxy.data?.https_port
      ? `https://${host}:${proxy.data.https_port}`
      : null;

  const authEnabled = settings.data?.auth_enabled ?? true;
  const token = settings.data?.auth_token ?? "";

  return (
    <div className="lr-grid">
      <section className="card">
        <div className="card-head">
          <span className="card-title">{t("liveRouting.access.title")}</span>
        </div>
        <div className="card-body access-fields">
          {httpUrl && (
            <CopyField label={t("liveRouting.access.httpUrl")} value={httpUrl} />
          )}
          {httpsUrl && (
            <CopyField
              label={t("liveRouting.access.httpsUrl")}
              value={httpsUrl}
              hint={t("liveRouting.access.httpsHint")}
            />
          )}
          <CopyField
            label={t("liveRouting.access.token")}
            value={authEnabled ? token : t("liveRouting.access.authOff")}
            hint={authEnabled ? t("liveRouting.access.tokenHint") : undefined}
            copyable={authEnabled}
          />
          <div className="access-pair">
            <div>
              <div className="access-label">{t("liveRouting.access.bind")}</div>
              <div className="access-value">
                {proxy.data?.listen_all
                  ? t("liveRouting.access.bindAll")
                  : t("liveRouting.access.bindLocal")}
              </div>
            </div>
            <div>
              <div className="access-label">{t("liveRouting.access.cors")}</div>
              <div className="access-value">{settings.data?.cors_allow_origin ?? "*"}</div>
            </div>
          </div>
        </div>
      </section>

      <section className="card alt">
        <div className="card-head">
          <span className="card-title">{t("liveRouting.api.title")}</span>
          <span className="hand api-note">{t("liveRouting.api.dualProtocol")}</span>
        </div>
        <div className="card-body lr-api">
          <div>
            {API_ROUTES.map((r) => (
              <div className="api-row" key={r.path}>
                <span className={`api-method ${r.method.toLowerCase()}`}>{r.method}</span>
                <span className="api-path">{r.path}</span>
                <span className="api-desc">{t(r.descKey)}</span>
              </div>
            ))}
          </div>
          <div className="readonly-note">
          <Lock size={15} />
          <span style={{ flex: 1 }}>{t("liveRouting.readonly.notice")}</span>
            <button className="btn-dark" type="button" onClick={() => navigate("/settings?tab=proxy")}>
              {t("liveRouting.readonly.goSettings")} <ArrowRight size={12} />
            </button>
          </div>
        </div>
      </section>
    </div>
  );
}

function CopyField({
  label,
  value,
  hint,
  copyable = true,
}: {
  label: string;
  value: string;
  hint?: string;
  copyable?: boolean;
}) {
  const { t } = useT();
  const [copied, setCopied] = useState(false);

  async function copy() {
    try {
      await runtime.copyText(value);
    } catch {
      /* ignore */
    }
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  }

  return (
    <div>
      <div className="access-label">{label}</div>
      <div className="access-row">
        <div className="access-value" title={value}>
          {value}
        </div>
        {copyable && (
          <button className="btn sm access-copy" type="button" onClick={copy}>
            {copied ? (
              <>
                <Check size={12} /> {t("copyable.copied")}
              </>
            ) : (
              <>
                <Copy size={12} /> {t("copyable.copy")}
              </>
            )}
          </button>
        )}
      </div>
      {hint && <div className="access-hint">{hint}</div>}
    </div>
  );
}

/* ============================================================
 * 虚拟模型映射 (传入名 → 虚拟模型 → 真实模型)
 * 三列同高严格逐行对齐 —— 行高与箭头格同为 40px, 箭头列用 padding-top 避开标题行。
 * ============================================================ */

function MappingSection() {
  const { t } = useT();
  const clock = useHostClock();
  const navigate = useNavigate();
  const vms = useVirtualModels();
  const subs = useSubscriptions();

  const subsMap = useMemo(() => {
    const m = new Map<string, SubscriptionDto>();
    subs.data?.forEach((s) => m.set(s.id, s));
    return m;
  }, [subs.data]);

  const rows = useMemo<VirtualModelDto[]>(
    () =>
      VM_ORDER.map((name) => vms.data?.find((v) => v.name === name)).filter(
        (v): v is VirtualModelDto => v !== undefined,
      ),
    [vms.data],
  );

  return (
    <section className="card alt">
      <div className="card-head">
        <span className="card-title">{t("liveRouting.map.title")}</span>
        <button className="btn sm" type="button" onClick={() => navigate("/virtual-models")}>
          {t("liveRouting.map.goConfigure")} <ArrowRight size={12} />
        </button>
      </div>
      <div className="card-body">

      <div className="vm-map">
        {/* 左: 客户端可填的模型名 */}
        <div className="vm-map-col client">
          <div className="vm-map-head">
            <span className="vm-map-num">1</span>
            <span>{t("liveRouting.map.colClient")}</span>
          </div>
          {rows.map((vm) => (
            <div
              className={vm.name === "model-fallback" ? "vm-map-row fallback" : "vm-map-row"}
              key={vm.name}
            >
              {vm.name === "model-fallback" ? (
                <span className="vm-map-note">{t("liveRouting.map.fallbackLeft")}</span>
              ) : (
                CLIENT_ALIASES[vm.name].map((alias, i) => (
                  <span className={i === 0 ? "vm-chip primary" : "vm-chip"} key={alias}>
                    {alias}
                  </span>
                ))
              )}
            </div>
          ))}
        </div>

        <ArrowColumn rows={rows} />

        {/* 中: cc-router 内部虚拟模型 */}
        <div className="vm-map-col router">
          <div className="vm-map-head">
            <span className="vm-map-num">2</span>
            <span>{t("liveRouting.map.colVirtual")}</span>
          </div>
          {rows.map((vm) => (
            <div
              className={vm.name === "model-fallback" ? "vm-map-row fallback" : "vm-map-row"}
              key={vm.name}
            >
              <span className="vm-pill">{vm.name}</span>
            </div>
          ))}
        </div>

        <ArrowColumn rows={rows} />

        {/* 右: 真实模型 (读自「虚拟模型」页的绑定) */}
        <div className="vm-map-col real">
          <div className="vm-map-head">
            <span className="vm-map-num">3</span>
            <span>{t("liveRouting.map.colReal")}</span>
          </div>
          {rows.map((vm) => {
            const slot = vmNameToSlot(vm.name);
            return (
              <div
                className={vm.name === "model-fallback" ? "vm-map-row fallback" : "vm-map-row"}
                key={vm.name}
              >
                {vm.subscription_ids.length === 0 ? (
                  <span className="vm-map-note" style={{ color: "var(--ink-4)" }}>
                    {t("routeFlow.notBound")}
                  </span>
                ) : (
                  <>
                    {vm.subscription_ids.map((sid) => {
                      const sub = subsMap.get(sid);
                      if (!sub) return null;
                      // fallback 行三态: 兜底槽值 / 透传 / 翻译类未配槽会被跳过
                      const fallbackModel = sub.model_slots.fallback?.trim() ?? "";
                      // 四槽位显示宿主机此刻的生效模型 (分时段规则)
                      const slotView = slot !== null ? slotNow(sub, slot, clock) : null;
                      const real =
                        vm.name === "model-jev"
                          ? jevModelOf(sub) ?? t("sortableSub.passthrough")
                          : slot === null
                          ? fallbackModel ||
                            (isAnthropicPassthrough(sub.auth_type)
                              ? t("sortableSub.passthrough")
                              : t("sortableSub.fallbackSkipped"))
                          : slotView
                          ? slotNowText(slotView, t)
                          : "";
                      return (
                        <span
                          className={`vm-real${sub.state === "healthy" ? "" : " err"}${slotView && slotView.model === null ? " off" : ""}`}
                          key={sid}
                          title={`${sub.display_name} · ${real}`}
                        >
                          <ProviderLogo iconId={sub.provider_icon} size={15} iconSize={10} />
                          {real}
                          {slotView?.scheduled && <span className="slot-now-mark">{t("schedule.now.scheduled")}</span>}
                        </span>
                      );
                    })}
                    <span className="vm-mode">{t(MODE_LABEL_KEY[vm.mode])}</span>
                  </>
                )}
              </div>
            );
          })}
        </div>
      </div>

      <div className="vm-map-foot">
        <span>{t("liveRouting.map.note.wildcard")}</span>
        <span>{t("liveRouting.map.note.prefix")}</span>
        <span>{t("liveRouting.map.note.source")}</span>
      </div>
      </div>
    </section>
  );
}

/** 手画箭头列; 兜底行用虚线箭头, 与该行的虚线分隔呼应。两种笔迹交替, 免得像盖章 */
function ArrowColumn({ rows }: { rows: VirtualModelDto[] }) {
  return (
    <div className="vm-map-arrows" aria-hidden>
      {rows.map((vm, i) => (
        <svg key={vm.name} viewBox="0 0 34 40" width="34" height="40">
          <path
            className={vm.name === "model-fallback" ? "fallback" : undefined}
            d={i % 2 === 0 ? "M3 21 C 10 19, 20 23, 29 20 M23 15.5 L30 20 L23.5 25" : "M3 20 C 10 22, 20 18, 29 20 M23 15.5 L30 20 L23.5 25"}
          />
        </svg>
      ))}
    </div>
  );
}
