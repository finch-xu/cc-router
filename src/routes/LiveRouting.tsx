import { useMemo, useState } from "react";
import { useNavigate } from "react-router-dom";
import { ArrowRight, Check, Copy, Lock } from "lucide-react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { RouteFlowDiagram } from "@/components/RouteFlowDiagram";
import { ProviderLogo } from "@/components/ProviderLogo";
import { useProxyStatus, useSettings } from "@/hooks/useSettings";
import { useSubscriptions } from "@/hooks/useSubscriptions";
import { useVirtualModels } from "@/hooks/useVirtualModels";
import { isAnthropicPassthrough } from "@/lib/authTypes";
import { MODE_LABEL_KEY, VM_ORDER, vmNameToSlot } from "@/lib/virtualModels";
import { useT } from "@/i18n";
import type { SubscriptionDto, VirtualModelDto, VirtualModelName } from "@/types";

/**
 * 客户端可以填的模型名 → 虚拟模型。
 * 事实来源是 Rust 侧 `virtual_model/model.rs::parse`, 改那边必须同步这里。
 * `*` 表示模糊匹配; `anthropic/` `openai/` 前缀对所有别名通用, 抽到脚注里说明。
 */
const CLIENT_ALIASES: Record<VirtualModelName, string[]> = {
  "model-fable": ["model-fable", "claude-fable*", "gpt-5.6", "gpt-*-sol"],
  "model-opus": ["model-opus", "claude-opus*", "gpt-5.5", "gpt-*-terra"],
  "model-sonnet": ["model-sonnet", "claude-sonnet*", "gpt-5.4", "gpt-*-luna"],
  "model-haiku": ["model-haiku", "claude-haiku*", "gpt-*-mini"],
  "model-fallback": [],
};

/** 照抄 src-tauri/src/proxy/server.rs::build_router —— 唯一事实来源 */
const API_ROUTES: { method: string; path: string; descKey: string }[] = [
  { method: "POST", path: "/v1/messages", descKey: "liveRouting.api.messages" },
  { method: "POST", path: "/v1/responses", descKey: "liveRouting.api.responses" },
  { method: "GET", path: "/v1/models", descKey: "liveRouting.api.models" },
  { method: "GET", path: "/health", descKey: "liveRouting.api.health" },
];

export function LiveRoutingPage() {
  return (
    <div className="page-flow">
      <RouteFlowDiagram />
      <AccessSection />
      <MappingSection />
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
    <div className="flush-split">
      <div>
        <div className="flush-title" style={{ marginBottom: 12 }}>
          {t("liveRouting.access.title")}
        </div>
        <div className="access-fields">
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
      </div>

      <div>
        <div className="flush-title">
          {t("liveRouting.api.title")}
          <span className="mono" style={{ fontSize: 11, color: "var(--ink-4)", fontWeight: 400 }}>
            {t("liveRouting.api.dualProtocol")}
          </span>
        </div>
        <div style={{ marginTop: 4 }}>
          {API_ROUTES.map((r) => (
            <div className="api-row" key={r.path}>
              <span className="api-method">{r.method}</span>
              <span className="api-path">{r.path}</span>
              <span className="api-desc">{t(r.descKey)}</span>
            </div>
          ))}
        </div>
        <div className="readonly-note">
          <Lock size={14} />
          <span style={{ flex: 1 }}>{t("liveRouting.readonly.notice")}</span>
          <button className="btn-dark" type="button" onClick={() => navigate("/settings")}>
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
      await writeText(value);
    } catch {
      try {
        await navigator.clipboard.writeText(value);
      } catch {
        /* ignore */
      }
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
          <button className="access-copy" type="button" onClick={copy}>
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
 * 区块 C: 虚拟模型映射 (传入名 → 虚拟模型 → 真实模型)
 * 三列同高严格逐行对齐 —— 中间两列箭头靠 transparent 上边框补齐 1px 分隔线。
 * ============================================================ */

function MappingSection() {
  const { t } = useT();
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
    <div className="flush-section">
      <div className="flush-title" style={{ marginBottom: 11 }}>
        {t("liveRouting.map.title")}
      </div>

      <div className="vm-map">
        {/* 左: 客户端可填的模型名 */}
        <div className="vm-map-col client">
          <div className="vm-map-head">
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

        <ArrowColumn count={rows.length} />

        {/* 中: cc-router 内部虚拟模型 */}
        <div className="vm-map-col router">
          <div className="vm-map-head">
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

        <ArrowColumn count={rows.length} />

        {/* 右: 真实模型 (读自「虚拟模型」页的绑定) */}
        <div className="vm-map-col real">
          <div className="vm-map-head">
            <span>{t("liveRouting.map.colReal")}</span>
            <button
              className="vm-map-goto"
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
                      const real =
                        slot === null
                          ? fallbackModel ||
                            (isAnthropicPassthrough(sub.auth_type)
                              ? t("sortableSub.passthrough")
                              : t("sortableSub.fallbackSkipped"))
                          : sub.model_slots[slot];
                      return (
                        <span
                          className={sub.state === "healthy" ? "vm-real" : "vm-real err"}
                          key={sid}
                          title={`${sub.display_name} · ${real}`}
                        >
                          <ProviderLogo iconId={sub.provider_icon} size={15} iconSize={10} />
                          {real}
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
  );
}

function ArrowColumn({ count }: { count: number }) {
  return (
    <div className="vm-map-arrows" aria-hidden>
      {Array.from({ length: count }, (_, i) => (
        <span key={i}>→</span>
      ))}
    </div>
  );
}
