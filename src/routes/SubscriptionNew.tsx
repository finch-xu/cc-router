import { useState, useMemo, useRef } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { ArrowLeft, ArrowRight, ExternalLink, Check } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { open as openShell } from "@tauri-apps/plugin-shell";
import { ProviderBadge } from "@/components/ProviderBadge";
import { ProviderLogo } from "@/components/ProviderLogo";
import { Spinner } from "@/components/Spinner";
import { ModelSlotPicker } from "@/components/ModelSlotPicker";
import { useProviders } from "@/hooks/useProviders";
import { useCreateSubscription } from "@/hooks/useSubscriptions";
import type {
  CreateSubscriptionInput,
  ModelInfo,
  ModelSlots,
  RefreshModelListResult,
} from "@/types";

type Step = 1 | 2;

export function SubscriptionNewPage() {
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const returnTo = searchParams.get("returnTo");
  const providers = useProviders();
  const createMut = useCreateSubscription();

  const [step, setStep] = useState<Step>(1);
  const [providerId, setProviderId] = useState<string>("");
  const [endpointId, setEndpointId] = useState<string>("");
  const [apiKey, setApiKey] = useState<string>("");
  const [displayName, setDisplayName] = useState<string>("");

  const [slots, setSlots] = useState<ModelSlots>({ opus: "", sonnet: "", haiku: "" });
  const [models, setModels] = useState<ModelInfo[] | null>(null);
  const [modelFetchError, setModelFetchError] = useState<string | null>(null);
  const [fetchingModels, setFetchingModels] = useState(false);

  const provider = useMemo(
    () => providers.data?.find((p) => p.id === providerId),
    [providers.data, providerId],
  );
  const endpoint = useMemo(
    () => provider?.endpoints.find((e) => e.id === endpointId),
    [provider, endpointId],
  );

  // 追踪自动生成的 displayName;用户手改后不再覆盖
  const autoGenNameRef = useRef<string>("");

  function handleProviderChange(v: string) {
    setProviderId(v);
    const p = providers.data?.find((x) => x.id === v);
    setEndpointId(p?.default_endpoint ?? p?.endpoints[0]?.id ?? "");

    if (p && (displayName === "" || displayName === autoGenNameRef.current)) {
      const suffix = Math.random().toString(36).slice(2, 8);
      const generated = `${p.display_name} ${suffix}`;
      setDisplayName(generated);
      autoGenNameRef.current = generated;
    }
  }

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
        provider_id: provider.id,
        endpoint_id: endpoint.id,
        display_name: displayName,
        api_key: apiKey,
        model_slots: placeholderSlots,
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
      setModelFetchError(`创建失败: ${e}`);
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

  async function save() {
    if (!createdId || !provider || !endpoint) return;
    await invoke("update_subscription", {
      id: createdId,
      patch: { model_slots: slots },
    });
    navigate(returnTo ?? `/subscriptions/${createdId}`);
  }

  return (
    <>
      <Link
        to={returnTo ?? "/subscriptions"}
        className="btn bare sm"
        style={{ marginBottom: 18, textDecoration: "none" }}
      >
        <ArrowLeft size={12} /> {returnTo ? "返回" : "返回列表"}
      </Link>

      <div className="page-header">
        <h1>添加订阅</h1>
        <div className="subtitle">把新厂商的 API Key 接入路由器,绑定到虚拟模型槽位。</div>
      </div>

      <div className="wizard">
        <div className="steps">
          <div className={`step ${step >= 1 ? "active" : ""} ${step > 1 ? "done" : ""}`}>
            <span className="num">{step > 1 ? <Check size={11} /> : 1}</span>
            <span>基本信息</span>
          </div>
          <div className="step-bar" />
          <div className={`step ${step === 2 ? "active" : ""} ${step > 2 ? "done" : ""}`}>
            <span className="num">2</span>
            <span>绑定模型</span>
          </div>
        </div>

        <div className="card">
          <div className="card-body" style={{ paddingTop: 24 }}>
            {step === 1 && (
              <>
                <div style={{ marginBottom: 20 }}>
                  <label className="field-label">厂商</label>
                  <select
                    className="select"
                    value={providerId}
                    onChange={(e) => handleProviderChange(e.target.value)}
                  >
                    <option value="" disabled>
                      选择厂商
                    </option>
                    {providers.data?.map((p) => (
                      <option key={p.id} value={p.id}>
                        {p.display_name}
                      </option>
                    ))}
                  </select>
                  {provider && (
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
                      <ProviderLogo iconId={provider.icon} size={22} />
                      <ProviderBadge compatibility={provider.compatibility} />
                      {provider.compatibility_notes && (
                        <span>{provider.compatibility_notes}</span>
                      )}
                    </div>
                  )}
                </div>

                {provider && (
                  <div style={{ marginBottom: 20 }}>
                    <label className="field-label">接入点</label>
                    <select
                      className="select"
                      value={endpointId}
                      onChange={(e) => setEndpointId(e.target.value)}
                    >
                      {provider.endpoints.map((e) => (
                        <option key={e.id} value={e.id}>
                          {e.label}
                        </option>
                      ))}
                    </select>
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
                            <ExternalLink size={11} /> 去官网获取 API Key
                          </button>
                        )}
                      </div>
                    )}
                  </div>
                )}

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

                <div style={{ marginBottom: 24 }}>
                  <label className="field-label">备注名</label>
                  <input
                    className="input"
                    value={displayName}
                    onChange={(e) => setDisplayName(e.target.value)}
                    placeholder="例如: MiniMax 主号"
                  />
                  <div className="field-hint">仅用于本地区分,不会上传到任何地方。</div>
                </div>

                {modelFetchError && (
                  <div className="alert err" style={{ marginBottom: 16 }}>
                    {modelFetchError}
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
                  <Link className="btn" to={returnTo ?? "/subscriptions"}>
                    取消
                  </Link>
                  <button
                    className="btn primary"
                    onClick={goToStep2}
                    disabled={!provider || !endpoint || !apiKey || !displayName || fetchingModels}
                    type="button"
                  >
                    {fetchingModels && <Spinner />}
                    下一步 <ArrowRight size={12} />
                  </button>
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
                    <ArrowLeft size={12} /> 上一步
                  </button>
                  <button
                    className="btn primary"
                    onClick={save}
                    disabled={!slots.opus || !slots.sonnet || !slots.haiku}
                    type="button"
                  >
                    保存
                  </button>
                </div>
              </>
            )}
          </div>
        </div>
      </div>
    </>
  );
}
