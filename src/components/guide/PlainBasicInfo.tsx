import { CustomModelMappings } from "@/components/virtual-models/CustomModelMappings";
import { useMemo, useState } from "react";
import { useNavigate } from "react-router";
import { ArrowRight, Check, Copy, Lock } from "lucide-react";
import { runtime } from "@/runtime";
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
import type { SubscriptionDto, VirtualModelDto } from "@/types";

export function PlainBasicInfo() {
  return (
    <div className="page-flow">
      <AccessSection />
      <MappingSection />
      <CustomModelMappings />
    </div>
  );
}

/* ============================================================
 * 区块 B: 接入信息 + API 入口
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
    <div className="lrc-flush-split">
      <div>
        <div className="lrc-flush-title" style={{ marginBottom: 12 }}>
          {t("liveRouting.access.title")}
        </div>
        <div className="lrc-access-fields">
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
          <div className="lrc-access-pair">
            <div>
              <div className="lrc-access-label">{t("liveRouting.access.bind")}</div>
              <div className="lrc-access-value">
                {proxy.data?.listen_all
                  ? t("liveRouting.access.bindAll")
                  : t("liveRouting.access.bindLocal")}
              </div>
            </div>
            <div>
              <div className="lrc-access-label">{t("liveRouting.access.cors")}</div>
              <div className="lrc-access-value">{settings.data?.cors_allow_origin ?? "*"}</div>
            </div>
          </div>
        </div>
      </div>

      <div>
        <div className="lrc-flush-title">
          {t("liveRouting.api.title")}
          <span className="mono" style={{ fontSize: 11, color: "var(--ink-4)", fontWeight: 400 }}>
            {t("liveRouting.api.dualProtocol")}
          </span>
        </div>
        <div style={{ marginTop: 4 }}>
          {API_ROUTES.map((r) => (
            <div className="lrc-api-row" key={r.path}>
              <span className="lrc-api-method">{r.method}</span>
              <span className="lrc-api-path">{r.path}</span>
              <span className="lrc-api-desc">{t(r.descKey)}</span>
            </div>
          ))}
        </div>
        <div className="lrc-readonly-note">
          <Lock size={14} />
          <span style={{ flex: 1 }}>{t("liveRouting.readonly.notice")}</span>
          <button className="lrc-btn-dark" type="button" onClick={() => navigate("/settings?tab=proxy")}>
            {t("liveRouting.readonly.goSettings")} <ArrowRight size={12} />
          </button>
        </div>
      </div>
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
      <div className="lrc-access-label">{label}</div>
      <div className="lrc-access-row">
        <div className="lrc-access-value" title={value}>
          {value}
        </div>
        {copyable && (
          <button className="lrc-access-copy" type="button" onClick={copy}>
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
      {hint && <div className="lrc-access-hint">{hint}</div>}
    </div>
  );
}

/* ============================================================
 * 区块 C: 虚拟模型映射 (传入名 → 虚拟模型 → 真实模型)
 * 三列同高严格逐行对齐 —— 中间两列箭头靠 transparent 上边框补齐 1px 分隔线。
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
    <div className="lrc-flush-section">
      <div className="lrc-flush-title" style={{ marginBottom: 11 }}>
        {t("liveRouting.map.title")}
      </div>

      <div className="lrc-vm-map">
        {/* 左: 客户端可填的模型名 */}
        <div className="lrc-vm-map-col client">
          <div className="lrc-vm-map-head">
            <span>{t("liveRouting.map.colClient")}</span>
          </div>
          {rows.map((vm) => (
            <div
              className={vm.name === "model-fallback" ? "lrc-vm-map-row fallback" : "lrc-vm-map-row"}
              key={vm.name}
            >
              {vm.name === "model-fallback" ? (
                <span className="lrc-vm-map-note">{t("liveRouting.map.fallbackLeft")}</span>
              ) : (
                CLIENT_ALIASES[vm.name].map((alias, i) => (
                  <span className={i === 0 ? "lrc-vm-chip primary" : "lrc-vm-chip"} key={alias}>
                    {alias}
                  </span>
                ))
              )}
            </div>
          ))}
        </div>

        <ArrowColumn count={rows.length} />

        {/* 中: cc-router 内部虚拟模型 */}
        <div className="lrc-vm-map-col router">
          <div className="lrc-vm-map-head">
            <span>{t("liveRouting.map.colVirtual")}</span>
          </div>
          {rows.map((vm) => (
            <div
              className={vm.name === "model-fallback" ? "lrc-vm-map-row fallback" : "lrc-vm-map-row"}
              key={vm.name}
            >
              <span className="lrc-vm-pill">{vm.name}</span>
            </div>
          ))}
        </div>

        <ArrowColumn count={rows.length} />

        {/* 右: 真实模型 (读自「虚拟模型」页的绑定) */}
        <div className="lrc-vm-map-col real">
          <div className="lrc-vm-map-head">
            <span>{t("liveRouting.map.colReal")}</span>
            <button
              className="lrc-vm-map-goto"
              type="button"
              onClick={() => navigate("/virtual-models")}
            >
              {t("liveRouting.map.goConfigure")} <ArrowRight size={10} />
            </button>
          </div>
          {rows.map((vm) => {
            const slot = vmNameToSlot(vm.name);
            return (
              <div
                className={vm.name === "model-fallback" ? "lrc-vm-map-row fallback" : "lrc-vm-map-row"}
                key={vm.name}
              >
                {vm.subscription_ids.length === 0 ? (
                  <span className="lrc-vm-map-note" style={{ color: "var(--ink-4)" }}>
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
                          className={`lrc-vm-real${sub.state === "healthy" ? "" : " err"}${slotView && slotView.model === null ? " off" : ""}`}
                          key={sid}
                          title={`${sub.display_name} · ${real}`}
                        >
                          <ProviderLogo iconId={sub.provider_icon} size={15} iconSize={10} />
                          {real}
                          {slotView?.scheduled && <span className="slot-now-mark">{t("schedule.now.scheduled")}</span>}
                        </span>
                      );
                    })}
                    <span className="lrc-vm-mode">{t(MODE_LABEL_KEY[vm.mode])}</span>
                  </>
                )}
              </div>
            );
          })}
        </div>
      </div>

      <div className="lrc-vm-map-foot">
        <span>{t("liveRouting.map.note.wildcard")}</span>
        <span>{t("liveRouting.map.note.prefix")}</span>
        <span>{t("liveRouting.map.note.source")}</span>
      </div>
    </div>
  );
}

function ArrowColumn({ count }: { count: number }) {
  return (
    <div className="lrc-vm-map-arrows" aria-hidden>
      {Array.from({ length: count }, (_, i) => (
        <span key={i}>→</span>
      ))}
    </div>
  );
}
