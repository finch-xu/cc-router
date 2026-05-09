import { useState, useMemo, useRef } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { ArrowLeft, ArrowRight, ExternalLink, Check, Plus, Boxes } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { open as openShell } from "@tauri-apps/plugin-shell";
import { useQueryClient } from "@tanstack/react-query";
import { ProviderBadge } from "@/components/ProviderBadge";
import { ProviderLogo } from "@/components/ProviderLogo";
import { Spinner } from "@/components/Spinner";
import { ModelSlotPicker } from "@/components/ModelSlotPicker";
import { ChatGptOAuthDialog } from "@/components/ChatGptOAuthDialog";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useProviders } from "@/hooks/useProviders";
import { useCreateSubscription } from "@/hooks/useSubscriptions";
import { useVirtualModels } from "@/hooks/useVirtualModels";
import { useT } from "@/i18n";
import { api } from "@/api/tauri";
import { validateConnection } from "@/lib/connectionValidation";
import type {
  AuthHeaderFormat,
  ChatGptAccount,
  CreateSubscriptionInput,
  ModelInfo,
  ModelSlots,
  RefreshModelListResult,
  VirtualModelName,
} from "@/types";

type Step = 1 | 2;

const CUSTOM_VALUE = "__custom__";

/** 自定义路径的鉴权方式预设: 选一个就同时确定 header_name + header_format */
type AuthPreset = "bearer" | "x_api_key";
const AUTH_PRESETS: Record<AuthPreset, { name: string; format: AuthHeaderFormat; label: string }> = {
  bearer: { name: "Authorization", format: "bearer", label: "Authorization: Bearer <key>" },
  x_api_key: { name: "x-api-key", format: "raw", label: "x-api-key: <key>" },
};

export function SubscriptionNewPage() {
  const { t } = useT();
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const returnTo = searchParams.get("returnTo");
  const isOnboarding = searchParams.get("onboarding") === "1";
  const queryClient = useQueryClient();
  const providers = useProviders();
  const createMut = useCreateSubscription();
  const vms = useVirtualModels();

  const [step, setStep] = useState<Step>(1);
  const [providerId, setProviderId] = useState<string>("");
  const [endpointId, setEndpointId] = useState<string>("");
  const [apiKey, setApiKey] = useState<string>("");
  const [displayName, setDisplayName] = useState<string>("");

  const [slots, setSlots] = useState<ModelSlots>({ opus: "", sonnet: "", haiku: "" });
  const [models, setModels] = useState<ModelInfo[] | null>(null);
  const [modelFetchError, setModelFetchError] = useState<string | null>(null);
  const [fetchingModels, setFetchingModels] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  // 自定义路径专用字段
  const [customProviderName, setCustomProviderName] = useState<string>("");
  const [customBaseUrl, setCustomBaseUrl] = useState<string>("");
  const [customMessagesPath, setCustomMessagesPath] = useState<string>("/v1/messages");
  const [customAuthPreset, setCustomAuthPreset] = useState<AuthPreset>("bearer");

  const isCustom = providerId === CUSTOM_VALUE;

  const provider = useMemo(
    () => (isCustom ? undefined : providers.data?.find((p) => p.id === providerId)),
    [providers.data, providerId, isCustom],
  );
  const endpoint = useMemo(
    () => provider?.endpoints.find((e) => e.id === endpointId),
    [provider, endpointId],
  );
  const isChatGptOAuth = provider?.auth.type === "chatgpt_oauth";

  // ChatGPT OAuth 流程 state. deviceCode + account 同生命周期, 合一份
  const [oauthDialogOpen, setOauthDialogOpen] = useState(false);
  const [oauthResult, setOauthResult] = useState<{
    deviceCode: string;
    account: ChatGptAccount;
  } | null>(null);

  // 追踪自动生成的 displayName;用户手改后不再覆盖
  const autoGenNameRef = useRef<string>("");

  function handleProviderChange(v: string) {
    setProviderId(v);
    setSubmitError(null);
    if (v === CUSTOM_VALUE) {
      setEndpointId("");
      // 自定义路径备注名自动: <自定义厂商名> <随机后缀>
      // 等用户填了 customProviderName 再生成,这里清掉旧值
      if (displayName === autoGenNameRef.current) {
        setDisplayName("");
        autoGenNameRef.current = "";
      }
      return;
    }
    const p = providers.data?.find((x) => x.id === v);
    setEndpointId(p?.default_endpoint ?? p?.endpoints[0]?.id ?? "");

    if (p && (displayName === "" || displayName === autoGenNameRef.current)) {
      const suffix = Math.random().toString(36).slice(2, 8);
      const generated = `${p.display_name} ${suffix}`;
      setDisplayName(generated);
      autoGenNameRef.current = generated;
    }
  }

  function handleCustomProviderNameChange(v: string) {
    setCustomProviderName(v);
    if (v && (displayName === "" || displayName === autoGenNameRef.current)) {
      const suffix = Math.random().toString(36).slice(2, 8);
      const generated = `${v} ${suffix}`;
      setDisplayName(generated);
      autoGenNameRef.current = generated;
    }
  }

  // ChatGPT OAuth 路径: step1 用 placeholder slots 调 createChatGptOAuthSubscription
  // 落 DB (消费 device_code) → refresh_model_list 拿真实模型 → step2.
  // 与内置 API Key 路径 (goToStep2) 完全对称, 保证两条路 UX 一致.
  async function goToStep2OAuth() {
    if (!provider || !endpoint) return;
    if (!oauthResult || !displayName) return;

    setFetchingModels(true);
    setModelFetchError(null);
    try {
      const placeholderSlots: ModelSlots = {
        opus: "(pending)",
        sonnet: "(pending)",
        haiku: "(pending)",
      };
      const created = await api.createChatGptOAuthSubscription({
        device_code: oauthResult.deviceCode,
        provider_id: provider.id,
        endpoint_id: endpoint.id,
        display_name: displayName,
        model_slots: placeholderSlots,
      });
      await queryClient.invalidateQueries({ queryKey: ["subscriptions"] });

      try {
        const result: RefreshModelListResult = await invoke("refresh_model_list", {
          id: created.id,
        });
        if (result.kind === "auto") {
          setModels(result.models);
          const first = result.models[0]?.id ?? "";
          if (first) {
            setSlots({ opus: first, sonnet: first, haiku: first });
          }
        } else {
          setModels(null);
          setModelFetchError(result.reason);
          const fallback = provider.model_discovery.example_models[0] ?? "";
          if (fallback) setSlots({ opus: fallback, sonnet: fallback, haiku: fallback });
        }
      } catch (e) {
        setModelFetchError(String(e));
        const fallback = provider.model_discovery.example_models[0] ?? "";
        if (fallback) setSlots({ opus: fallback, sonnet: fallback, haiku: fallback });
      }

      setCreatedId(created.id);
      setStep(2);
    } catch (e) {
      setModelFetchError(`${t("subscriptionNew.errCreate")}: ${e}`);
    } finally {
      setFetchingModels(false);
    }
  }

  // step2 保存: 改 OAuth 订阅的 model_slots (订阅在 step1 已经创建落 DB).
  // 不再调 createChatGptOAuthSubscription —— device_code 已经在 step1 消费.
  async function saveOAuth() {
    if (!createdId || !provider || !endpoint) return;
    if (!slots.opus || !slots.sonnet || !slots.haiku) {
      return setSubmitError(t("subscriptionNew.errFillSlots"));
    }
    setSubmitting(true);
    setSubmitError(null);
    try {
      await invoke("update_subscription", {
        id: createdId,
        patch: { model_slots: slots },
      });
      await queryClient.invalidateQueries({ queryKey: ["subscriptions"] });
      await bindToVirtualModelsIfOnboarding(createdId);
      if (isOnboarding) {
        navigate("/guide", { replace: true });
        return;
      }
      navigate(returnTo ?? `/subscriptions/${createdId}`);
    } catch (e) {
      setSubmitError(`${t("subscriptionNew.errCreate")}: ${e}`);
    } finally {
      setSubmitting(false);
    }
  }

  // 内置路径: step1 → 调 create_subscription(from_template, placeholder slots) → refresh_model_list → step2
  async function goToStep2() {
    if (!provider || !endpoint) return;
    if (!apiKey || !displayName) return;

    setFetchingModels(true);
    setModelFetchError(null);
    try {
      const placeholderSlots: ModelSlots = {
        opus: "(pending)",
        sonnet: "(pending)",
        haiku: "(pending)",
      };
      const input: CreateSubscriptionInput = {
        display_name: displayName,
        api_key: apiKey,
        model_slots: placeholderSlots,
        source: {
          kind: "from_template",
          provider_id: provider.id,
          endpoint_id: endpoint.id,
        },
      };
      const created = await createMut.mutateAsync(input);
      try {
        const result: RefreshModelListResult = await invoke("refresh_model_list", {
          id: created.id,
        });
        if (result.kind === "auto") {
          setModels(result.models);
          if (provider.model_discovery.example_models.length > 0 || result.models.length > 0) {
            const first = result.models[0]?.id ?? "";
            setSlots({ opus: first, sonnet: first, haiku: first });
          }
        } else {
          setModels(null);
          setModelFetchError(result.reason);
        }
      } catch (e) {
        setModelFetchError(String(e));
      }
      setCreatedId(created.id);
      setStep(2);
    } catch (e) {
      setModelFetchError(`${t("subscriptionNew.errCreate")}: ${e}`);
    } finally {
      setFetchingModels(false);
    }
  }

  const [createdId, setCreatedId] = useState<string | null>(null);

  async function refreshModels() {
    if (!createdId) return;
    setFetchingModels(true);
    setModelFetchError(null);
    try {
      const result: RefreshModelListResult = await invoke("refresh_model_list", {
        id: createdId,
      });
      if (result.kind === "auto") {
        setModels(result.models);
      } else {
        setModels(null);
        setModelFetchError(result.reason);
      }
    } catch (e) {
      setModelFetchError(String(e));
    } finally {
      setFetchingModels(false);
    }
  }

  async function bindToVirtualModelsIfOnboarding(subscriptionId: string) {
    if (!isOnboarding) return;
    const names: VirtualModelName[] = [
      "model-opus",
      "model-sonnet",
      "model-haiku",
      "model-fallback",
    ];
    await Promise.allSettled(
      names.map((name) => {
        const current = vms.data?.find((v) => v.name === name);
        const merged = Array.from(
          new Set([...(current?.subscription_ids ?? []), subscriptionId]),
        );
        return api.updateVirtualModel(name, {
          mode: current?.mode ?? "sequential",
          subscription_ids: merged,
        });
      }),
    );

    await api.completeOnboarding();
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["onboarding-state"] }),
      queryClient.invalidateQueries({ queryKey: ["virtual-models"] }),
      queryClient.invalidateQueries({ queryKey: ["subscriptions"] }),
    ]);
  }

  // 内置路径 step2: 保存 slot
  async function save() {
    if (!createdId || !provider || !endpoint) return;
    await invoke("update_subscription", {
      id: createdId,
      patch: { model_slots: slots },
    });

    await bindToVirtualModelsIfOnboarding(createdId);

    if (isOnboarding) {
      navigate("/guide", { replace: true });
      return;
    }
    navigate(returnTo ?? `/subscriptions/${createdId}`);
  }

  // 自定义路径: 单页提交
  async function saveCustom() {
    setSubmitError(null);
    if (!customProviderName) return setSubmitError(t("subscriptionNew.errFillProvider"));
    if (!customBaseUrl) return setSubmitError(t("subscriptionNew.errFillBase"));
    const connErrKey = validateConnection({
      base_url: customBaseUrl,
      messages_path: customMessagesPath,
    });
    if (connErrKey) return setSubmitError(t(connErrKey));
    if (!apiKey) return setSubmitError(t("subscriptionNew.errFillKey"));
    if (!displayName) return setSubmitError(t("subscriptionNew.errFillNote"));
    if (!slots.opus || !slots.sonnet || !slots.haiku) {
      return setSubmitError(t("subscriptionNew.errFillSlots"));
    }

    const preset = AUTH_PRESETS[customAuthPreset];
    const input: CreateSubscriptionInput = {
      display_name: displayName,
      api_key: apiKey,
      model_slots: slots,
      source: {
        kind: "custom",
        provider_display_name: customProviderName,
        base_url: customBaseUrl.trim(),
        messages_path: customMessagesPath.trim(),
        auth_header_name: preset.name,
        auth_header_format: preset.format,
      },
    };

    setSubmitting(true);
    try {
      const created = await createMut.mutateAsync(input);
      await bindToVirtualModelsIfOnboarding(created.id);
      if (isOnboarding) {
        navigate("/guide", { replace: true });
        return;
      }
      navigate(returnTo ?? `/subscriptions/${created.id}`);
    } catch (e) {
      setSubmitError(`${t("subscriptionNew.errCreate")}: ${e}`);
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <>
      {!isOnboarding && (
        <Link
          to={returnTo ?? "/subscriptions"}
          className="btn bare sm"
          style={{ marginBottom: 18, textDecoration: "none" }}
        >
          <ArrowLeft size={12} /> {returnTo ? t("subscriptionNew.back") : t("subscriptionNew.backToList")}
        </Link>
      )}

      <div className="page-header">
        <h1>{isOnboarding ? t("subscriptionNew.welcomeTitle") : t("subscriptionNew.title")}</h1>
        <div className="subtitle">
          {isOnboarding ? t("subscriptionNew.welcomeSubtitle") : t("subscriptionNew.subtitle")}
        </div>
      </div>

      <div className="wizard">
        {/* 自定义路径不分步, 隐藏步骤指示器 */}
        {!isCustom && (
          <div className="steps">
            <div className={`step ${step >= 1 ? "active" : ""} ${step > 1 ? "done" : ""}`}>
              <span className="num">{step > 1 ? <Check size={11} /> : 1}</span>
              <span>{t("subscriptionNew.step1")}</span>
            </div>
            <div className="step-bar" />
            <div className={`step ${step === 2 ? "active" : ""} ${step > 2 ? "done" : ""}`}>
              <span className="num">2</span>
              <span>{t("subscriptionNew.step2")}</span>
            </div>
          </div>
        )}

        <div className="card">
          <div className="card-body" style={{ paddingTop: 24 }}>
            {/* 步骤 1 (内置) 或 单页表单 (自定义) 共用厂商 dropdown */}
            {step === 1 && (
              <>
                <div style={{ marginBottom: 20 }}>
                  <label className="field-label">{t("subscriptionNew.field.provider")}</label>
                  <Select value={providerId} onValueChange={handleProviderChange}>
                    <SelectTrigger className="select h-auto">
                      {isCustom ? (
                        <span style={{ display: "inline-flex", alignItems: "center", gap: 8, minWidth: 0 }}>
                          <Boxes size={20} />
                          <span>{t("subscriptionNew.customProvider")}</span>
                        </span>
                      ) : provider ? (
                        <span style={{ display: "inline-flex", alignItems: "center", gap: 8, minWidth: 0 }}>
                          <ProviderLogo iconId={provider.icon} size={20} />
                          <span style={{ overflow: "hidden", textOverflow: "ellipsis" }}>
                            {provider.display_name}
                          </span>
                        </span>
                      ) : (
                        <span style={{ color: "var(--ink-4)" }}>{t("subscriptionNew.providerSelect")}</span>
                      )}
                    </SelectTrigger>
                    <SelectContent>
                      {providers.data?.map((p) => (
                        <SelectItem key={p.id} value={p.id}>
                          <span style={{ display: "inline-flex", alignItems: "center", gap: 8 }}>
                            <ProviderLogo iconId={p.icon} size={20} />
                            <span>{p.display_name}</span>
                          </span>
                        </SelectItem>
                      ))}
                      {/* 末尾分隔: 自定义入口 */}
                      <div style={{ height: 1, background: "var(--line)", margin: "4px 0" }} />
                      <SelectItem value={CUSTOM_VALUE}>
                        <span style={{ display: "inline-flex", alignItems: "center", gap: 8 }}>
                          <Plus size={20} />
                          <span>{t("subscriptionNew.customProvider")}</span>
                        </span>
                      </SelectItem>
                    </SelectContent>
                  </Select>
                  {provider && !isCustom && (
                    <div
                      style={{
                        marginTop: 8,
                        display: "flex",
                        alignItems: "center",
                        gap: 10,
                        fontSize: 12,
                        color: "var(--ink-3)",
                      }}
                    >
                      <ProviderBadge compatibility={provider.compatibility} />
                      {provider.compatibility_notes && (
                        <span>{provider.compatibility_notes}</span>
                      )}
                    </div>
                  )}
                  {isCustom && (
                    <div className="field-hint" style={{ marginTop: 6 }}>
                      {t("subscriptionNew.customHint")}
                    </div>
                  )}
                </div>

                {/* 内置路径: endpoint dropdown */}
                {provider && !isCustom && (
                  <div style={{ marginBottom: 20 }}>
                    <label className="field-label">{t("subscriptionNew.field.endpoint")}</label>
                    <Select value={endpointId} onValueChange={setEndpointId}>
                      <SelectTrigger className="select h-auto">
                        <SelectValue placeholder={t("subscriptionNew.endpointSelect")} />
                      </SelectTrigger>
                      <SelectContent>
                        {provider.endpoints.map((e) => (
                          <SelectItem key={e.id} value={e.id} subtitle={e.base_url}>
                            {e.label}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                    {endpoint && (
                      <div className="field-hint">
                        {endpoint.description && <div>{endpoint.description}</div>}
                        <div className="mono" style={{ color: "var(--ink-3)", marginTop: 4 }}>
                          {endpoint.base_url}
                          {endpoint.messages_path}
                        </div>
                        {provider.api_key_url && (
                          <button
                            type="button"
                            onClick={() => openShell(provider.api_key_url!).catch(() => {})}
                            style={{
                              marginTop: 6,
                              background: "transparent",
                              border: "none",
                              color: "var(--accent-ink)",
                              padding: 0,
                              fontSize: 11.5,
                              display: "inline-flex",
                              alignItems: "center",
                              gap: 4,
                              cursor: "pointer",
                            }}
                          >
                            <ExternalLink size={11} /> {t("subscriptionNew.openApiKey")}
                          </button>
                        )}
                      </div>
                    )}
                  </div>
                )}

                {/* 自定义路径: 厂商显示名 / base_url / messages_path / 鉴权方式 */}
                {isCustom && (
                  <>
                    <div style={{ marginBottom: 20 }}>
                      <label className="field-label">{t("subscriptionNew.field.providerName")}</label>
                      <input
                        className="input"
                        value={customProviderName}
                        onChange={(e) => handleCustomProviderNameChange(e.target.value)}
                        placeholder={t("subscriptionNew.providerNamePh")}
                      />
                      <div className="field-hint">{t("subscriptionNew.providerNameHint")}</div>
                    </div>

                    <div style={{ marginBottom: 20 }}>
                      <label className="field-label">Base URL</label>
                      <input
                        className="input mono"
                        value={customBaseUrl}
                        onChange={(e) => setCustomBaseUrl(e.target.value)}
                        placeholder="https://api.example.com"
                      />
                      <div className="field-hint">{t("subscriptionNew.baseUrlHint")}</div>
                    </div>

                    <div style={{ marginBottom: 20 }}>
                      <label className="field-label">Messages Path</label>
                      <input
                        className="input mono"
                        value={customMessagesPath}
                        onChange={(e) => setCustomMessagesPath(e.target.value)}
                        placeholder="/v1/messages"
                      />
                      <div className="field-hint">{t("subscriptionNew.messagesPathHint")}</div>
                    </div>

                    <div style={{ marginBottom: 20 }}>
                      <label className="field-label">{t("subscriptionNew.authMethod")}</label>
                      <Select
                        value={customAuthPreset}
                        onValueChange={(v) => setCustomAuthPreset(v as AuthPreset)}
                      >
                        <SelectTrigger className="select h-auto">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          {Object.entries(AUTH_PRESETS).map(([k, v]) => (
                            <SelectItem key={k} value={k}>
                              {v.label}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </div>
                  </>
                )}

                {/* ChatGPT OAuth 路径: 用「连接账号」按钮替代 API Key 输入框 */}
                {isChatGptOAuth ? (
                  <div style={{ marginBottom: 20 }}>
                    <label className="field-label">{t("subscriptionNew.field.chatgptAccount")}</label>
                    {oauthResult ? (
                      <div
                        style={{
                          display: "flex",
                          alignItems: "center",
                          justifyContent: "space-between",
                          padding: 12,
                          border: "1px solid var(--line)",
                          borderRadius: 6,
                          background: "var(--surface-2)",
                        }}
                      >
                        <div>
                          <div style={{ fontSize: 13, fontWeight: 500 }}>
                            {oauthResult.account.email ?? t("oauth.chatgpt.noEmail")}
                          </div>
                          <div className="mono" style={{ fontSize: 11, color: "var(--ink-3)" }}>
                            {oauthResult.account.account_id}
                          </div>
                        </div>
                        <button
                          type="button"
                          className="btn bare sm"
                          onClick={() => {
                            setOauthResult(null);
                            setOauthDialogOpen(true);
                          }}
                        >
                          {t("oauth.chatgpt.reconnect")}
                        </button>
                      </div>
                    ) : (
                      <button
                        type="button"
                        className="btn primary"
                        style={{ width: "100%" }}
                        onClick={() => setOauthDialogOpen(true)}
                      >
                        {t("oauth.chatgpt.connectButton")}
                      </button>
                    )}
                    <div className="field-hint" style={{ marginTop: 8 }}>
                      {t("oauth.chatgpt.connectHint")}
                    </div>
                  </div>
                ) : (
                  <div style={{ marginBottom: 20 }}>
                    <label className="field-label">API Key</label>
                    <input
                      className="input mono"
                      type="password"
                      value={apiKey}
                      onChange={(e) => setApiKey(e.target.value)}
                      placeholder="sk-..."
                    />
                  </div>
                )}

                <div style={{ marginBottom: 24 }}>
                  <label className="field-label">{t("subscriptionNew.field.note")}</label>
                  <input
                    className="input"
                    value={displayName}
                    onChange={(e) => setDisplayName(e.target.value)}
                    placeholder={t("subscriptionNew.notePh")}
                  />
                  <div className="field-hint">{t("subscriptionNew.noteHint")}</div>
                </div>

                {/* 自定义路径: 单页直接显示 3 slot 输入 */}
                {isCustom && (
                  <div style={{ marginBottom: 24 }}>
                    <label className="field-label">{t("subscriptionNew.slotsLabel")}</label>
                    <div className="field-hint" style={{ marginBottom: 8 }}>
                      {t("subscriptionNew.slotsHint")}
                    </div>
                    <SlotInput
                      label="model-opus →"
                      value={slots.opus}
                      onChange={(v) => setSlots({ ...slots, opus: v })}
                    />
                    <SlotInput
                      label="model-sonnet →"
                      value={slots.sonnet}
                      onChange={(v) => setSlots({ ...slots, sonnet: v })}
                    />
                    <SlotInput
                      label="model-haiku →"
                      value={slots.haiku}
                      onChange={(v) => setSlots({ ...slots, haiku: v })}
                    />
                  </div>
                )}

                {(modelFetchError || submitError) && (
                  <div className="alert err" style={{ marginBottom: 16 }}>
                    {submitError ?? modelFetchError}
                  </div>
                )}

                <div
                  style={{
                    display: "flex",
                    justifyContent: "flex-end",
                    gap: 8,
                    paddingTop: 12,
                    borderTop: "1px solid var(--line)",
                  }}
                >
                  {!isOnboarding && (
                    <Link className="btn" to={returnTo ?? "/subscriptions"}>
                      {t("common.cancel")}
                    </Link>
                  )}
                  {isCustom ? (
                    <button
                      className="btn primary"
                      onClick={saveCustom}
                      disabled={submitting}
                      type="button"
                    >
                      {submitting && <Spinner />}
                      {t("common.save")}
                    </button>
                  ) : isChatGptOAuth ? (
                    <button
                      className="btn primary"
                      onClick={goToStep2OAuth}
                      disabled={!provider || !endpoint || !oauthResult || !displayName}
                      type="button"
                    >
                      {t("common.next")} <ArrowRight size={12} />
                    </button>
                  ) : (
                    <button
                      className="btn primary"
                      onClick={goToStep2}
                      disabled={!provider || !endpoint || !apiKey || !displayName || fetchingModels}
                      type="button"
                    >
                      {fetchingModels && <Spinner />}
                      {t("common.next")} <ArrowRight size={12} />
                    </button>
                  )}
                </div>
              </>
            )}

            {step === 2 && provider && (
              <>
                <ModelSlotPicker
                  value={slots}
                  onChange={setSlots}
                  models={models}
                  loading={fetchingModels}
                  error={modelFetchError}
                  onRefresh={refreshModels}
                  exampleModels={provider.model_discovery.example_models}
                />

                {submitError && (
                  <div className="alert err" style={{ marginTop: 12 }}>
                    {submitError}
                  </div>
                )}

                <div
                  style={{
                    display: "flex",
                    justifyContent: "space-between",
                    paddingTop: 16,
                    marginTop: 16,
                    borderTop: "1px solid var(--line)",
                  }}
                >
                  <button className="btn bare" onClick={() => setStep(1)} type="button">
                    <ArrowLeft size={12} /> {t("common.prev")}
                  </button>
                  <button
                    className="btn primary"
                    onClick={isChatGptOAuth ? saveOAuth : save}
                    disabled={
                      !slots.opus || !slots.sonnet || !slots.haiku || submitting
                    }
                    type="button"
                  >
                    {submitting && <Spinner />}
                    {t("common.save")}
                  </button>
                </div>
              </>
            )}
          </div>
        </div>
      </div>

      <ChatGptOAuthDialog
        open={oauthDialogOpen}
        onClose={() => setOauthDialogOpen(false)}
        onSuccess={(deviceCode, account) => {
          setOauthResult({ deviceCode, account });
          if (!displayName) {
            const suffix = Math.random().toString(36).slice(2, 8);
            const generated = account.email
              ? `Codex · ${account.email} ${suffix}`
              : `Codex · ${suffix}`;
            setDisplayName(generated);
            autoGenNameRef.current = generated;
          }
          // 让 dialog 短暂展示「已连接」状态再关
          setTimeout(() => setOauthDialogOpen(false), 500);
        }}
      />
    </>
  );
}

function SlotInput({
  label,
  value,
  onChange,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
}) {
  const { t } = useT();
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 8 }}>
      <span style={{ fontFamily: "var(--mono)", fontSize: 12, color: "var(--ink-3)", width: 130 }}>
        {label}
      </span>
      <input
        className="input mono"
        style={{ flex: 1 }}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={t("subscriptionNew.slotPh")}
      />
    </div>
  );
}
