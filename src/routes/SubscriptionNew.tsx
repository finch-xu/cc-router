import { useState, useMemo, useRef } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { ArrowLeft, ExternalLink, Loader2 } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { open as openShell } from "@tauri-apps/plugin-shell";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { ProviderBadge } from "@/components/ProviderBadge";
import { ProviderIcon } from "@/components/ProviderIcon";
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

  // 追踪自动生成的 displayName；用户手改后不再覆盖
  const autoGenNameRef = useRef<string>("");

  function handleProviderChange(v: string) {
    setProviderId(v);
    const p = providers.data?.find((x) => x.id === v);
    setEndpointId(p?.default_endpoint ?? p?.endpoints[0]?.id ?? "");

    // 自动填充备注名：{厂商名} {6 位随机后缀}
    // 仅当用户尚未手动编辑过（为空，或仍是上一次自动生成的值）时覆盖
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

    // 先创建一个临时订阅，在 step 2 保存时更新 slots？
    // 简化：step 2 保存时一次性 create。暂时在 step2 调用 model_discovery 需要订阅 id。
    // 退而求其次：step 1 创建一个占位订阅（slots 空字符串），再 step 2 refresh。
    // 但 store::insert 要求 slots 非空 UI 层。后端允许空字符串。

    setFetchingModels(true);
    setModelFetchError(null);
    try {
      // 模拟：先创建，再 refresh。
      const placeholderSlots: ModelSlots = { opus: "(pending)", sonnet: "(pending)", haiku: "(pending)" };
      const input: CreateSubscriptionInput = {
        provider_id: provider.id,
        endpoint_id: endpoint.id,
        display_name: displayName,
        api_key: apiKey,
        model_slots: placeholderSlots,
      };
      const created = await createMut.mutateAsync(input);
      // 立即尝试 refresh models
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
      // 保存刚才创建的订阅 id 到局部，用于 step 2 更新
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
    <div className="p-8 max-w-2xl space-y-6">
      <Button variant="ghost" size="sm" asChild>
        <Link to={returnTo ?? "/subscriptions"}>
          <ArrowLeft className="h-4 w-4" /> {returnTo ? "返回" : "返回列表"}
        </Link>
      </Button>

      <div>
        <h1 className="text-2xl font-semibold">添加订阅</h1>
        <p className="text-sm text-muted-foreground">
          步骤 {step} / 2 · {step === 1 ? "基本信息" : "模型槽位配置"}
        </p>
      </div>

      {step === 1 && (
        <Card>
          <CardHeader>
            <CardTitle>基本信息</CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="space-y-2">
              <Label>厂商</Label>
              <Select value={providerId} onValueChange={handleProviderChange}>
                <SelectTrigger>
                  <SelectValue placeholder="选择厂商" />
                </SelectTrigger>
                <SelectContent>
                  {providers.data?.map((p) => (
                    <SelectItem key={p.id} value={p.id}>
                      <span className="inline-flex items-center gap-2">
                        <ProviderIcon iconId={p.icon} size={16} />
                        {p.display_name}
                      </span>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {provider && (
                <div className="flex items-center gap-2 text-xs">
                  <ProviderBadge compatibility={provider.compatibility} />
                  {provider.compatibility_notes && (
                    <span className="text-muted-foreground">
                      {provider.compatibility_notes}
                    </span>
                  )}
                </div>
              )}
            </div>

            {provider && (
              <div className="space-y-2">
                <Label>接入点</Label>
                <Select value={endpointId} onValueChange={setEndpointId}>
                  <SelectTrigger>
                    <SelectValue placeholder="选择接入点" />
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
                  <div className="space-y-0.5 text-xs">
                    {endpoint.description && (
                      <div className="text-muted-foreground">{endpoint.description}</div>
                    )}
                    <div className="font-mono text-muted-foreground">
                      {endpoint.base_url}
                      {endpoint.messages_path}
                    </div>
                  </div>
                )}
                {provider.api_key_url && (
                  <Button
                    variant="link"
                    size="sm"
                    className="p-0 h-auto text-xs"
                    onClick={() => openShell(provider.api_key_url!).catch(() => {})}
                  >
                    <ExternalLink className="h-3 w-3" /> 去官网获取 API Key
                  </Button>
                )}
              </div>
            )}

            <div className="space-y-2">
              <Label>API Key</Label>
              <Input
                type="password"
                value={apiKey}
                onChange={(e) => setApiKey(e.target.value)}
                placeholder="sk-..."
              />
            </div>

            <div className="space-y-2">
              <Label>备注名</Label>
              <Input
                value={displayName}
                onChange={(e) => setDisplayName(e.target.value)}
                placeholder="例如: MiniMax 主号"
              />
            </div>

            {modelFetchError && (
              <div className="text-sm text-destructive">{modelFetchError}</div>
            )}

            <div className="flex justify-end pt-2">
              <Button
                onClick={goToStep2}
                disabled={!provider || !endpoint || !apiKey || !displayName || fetchingModels}
              >
                {fetchingModels && <Loader2 className="h-4 w-4 animate-spin" />}
                下一步
              </Button>
            </div>
          </CardContent>
        </Card>
      )}

      {step === 2 && provider && (
        <Card>
          <CardHeader>
            <CardTitle>模型槽位配置</CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <ModelSlotPicker
              value={slots}
              onChange={setSlots}
              models={models}
              loading={fetchingModels}
              error={modelFetchError}
              onRefresh={refreshModels}
              exampleModels={provider.model_discovery.example_models}
            />

            <div className="flex justify-between pt-2">
              <Button variant="ghost" onClick={() => setStep(1)}>
                上一步
              </Button>
              <Button
                onClick={save}
                disabled={!slots.opus || !slots.sonnet || !slots.haiku}
              >
                保存
              </Button>
            </div>
          </CardContent>
        </Card>
      )}
    </div>
  );
}
