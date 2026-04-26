import { useEffect, useMemo, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { ArrowLeft, AlertTriangle, Loader2 } from "lucide-react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { StatusBadge } from "@/components/StatusBadge";
import { ProviderIcon } from "@/components/ProviderIcon";
import { ModelSlotPicker } from "@/components/ModelSlotPicker";
import { api } from "@/api/tauri";
import { validateConnection } from "@/lib/connectionValidation";
import { useProviders } from "@/hooks/useProviders";
import {
  useDeleteSubscription,
  useSetSubscriptionEnabled,
  useUpdateSubscription,
  useUpdateSubscriptionKey,
} from "@/hooks/useSubscriptions";
import type {
  AuthHeaderFormat,
  ModelInfo,
  ModelSlots,
  RefreshModelListResult,
  SubscriptionPatch,
  TestConnectionResult,
} from "@/types";

export function SubscriptionEditPage() {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  const subQuery = useQuery({
    queryKey: ["subscription", id],
    queryFn: () => api.getSubscription(id!),
    enabled: !!id,
  });
  const providers = useProviders();
  const updateMut = useUpdateSubscription();
  const deleteMut = useDeleteSubscription();
  const enabledMut = useSetSubscriptionEnabled();
  const updateKeyMut = useUpdateSubscriptionKey();

  // 内置订阅: 在 providers 列表里反查 yaml 模板, 用于显示 endpoint 下拉选项
  const provider = useMemo(
    () =>
      subQuery.data && !subQuery.data.is_user_defined
        ? providers.data?.find((p) => p.id === subQuery.data!.provider_id)
        : undefined,
    [providers.data, subQuery.data],
  );

  const [endpointId, setEndpointId] = useState<string>("");
  const [displayName, setDisplayName] = useState<string>("");
  const [slots, setSlots] = useState<ModelSlots>({ opus: "", sonnet: "", haiku: "" });

  // 自定义订阅可编辑的连接字段
  const [baseUrl, setBaseUrl] = useState<string>("");
  const [messagesPath, setMessagesPath] = useState<string>("");
  const [authHeaderName, setAuthHeaderName] = useState<string>("");
  const [authHeaderFormat, setAuthHeaderFormat] = useState<AuthHeaderFormat>("bearer");
  const [providerDisplayName, setProviderDisplayName] = useState<string>("");

  const [models, setModels] = useState<ModelInfo[] | null>(null);
  const [fetchingModels, setFetchingModels] = useState(false);
  const [modelError, setModelError] = useState<string | null>(null);
  const [testResult, setTestResult] = useState<TestConnectionResult | null>(null);
  const [keyDialog, setKeyDialog] = useState(false);
  const [newKey, setNewKey] = useState("");
  const [deleteDialog, setDeleteDialog] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  useEffect(() => {
    if (subQuery.data) {
      setEndpointId(subQuery.data.endpoint_id);
      setDisplayName(subQuery.data.display_name);
      setSlots(subQuery.data.model_slots);
      setBaseUrl(subQuery.data.base_url);
      setMessagesPath(subQuery.data.messages_path);
      setAuthHeaderName(subQuery.data.auth_header_name);
      setAuthHeaderFormat(subQuery.data.auth_header_format);
      setProviderDisplayName(subQuery.data.provider_display_name);
      if (subQuery.data.model_cache) {
        setModels(subQuery.data.model_cache.models);
      }
    }
  }, [subQuery.data?.id]);

  if (!id) return null;

  async function refreshModels() {
    if (!id) return;
    setFetchingModels(true);
    setModelError(null);
    try {
      const result: RefreshModelListResult = await invoke("refresh_model_list", { id });
      if (result.kind === "auto") {
        setModels(result.models);
      } else {
        setModels(null);
        setModelError(result.reason);
      }
    } catch (e) {
      setModelError(String(e));
    } finally {
      setFetchingModels(false);
    }
  }

  async function save() {
    if (!id || !sub) return;
    setSaveError(null);
    const patch: SubscriptionPatch = {
      display_name: displayName,
      model_slots: slots,
    };
    if (sub.is_user_defined) {
      const connErr = validateConnection({ base_url: baseUrl, messages_path: messagesPath });
      if (connErr) return setSaveError(connErr);
      patch.connection = {
        base_url: baseUrl.trim(),
        messages_path: messagesPath.trim(),
        auth_header_name: authHeaderName.trim(),
        auth_header_format: authHeaderFormat,
        provider_display_name: providerDisplayName.trim(),
      };
    } else {
      // 内置订阅: 切 endpoint 走 endpoint_id patch (后端 re-snapshot)
      if (endpointId !== sub.endpoint_id) {
        patch.endpoint_id = endpointId;
      }
    }
    try {
      await updateMut.mutateAsync({ id, patch });
    } catch (e) {
      setSaveError(`保存失败: ${e}`);
    }
  }

  async function testConnection() {
    if (!id) return;
    setTestResult(null);
    const result = await api.testConnection(id);
    setTestResult(result);
    if (result.ok && result.state_reset) {
      queryClient.invalidateQueries({ queryKey: ["subscriptions"] });
      queryClient.invalidateQueries({ queryKey: ["subscription", id] });
    }
  }

  async function confirmDelete() {
    if (!id) return;
    await deleteMut.mutateAsync(id);
    navigate("/subscriptions");
  }

  async function confirmUpdateKey() {
    if (!id || !newKey) return;
    await updateKeyMut.mutateAsync({ id, newKey });
    setKeyDialog(false);
    setNewKey("");
  }

  const sub = subQuery.data;

  if (subQuery.isLoading) {
    return <div className="p-8 text-sm text-muted-foreground">加载中…</div>;
  }

  if (!sub) {
    return <div className="p-8 text-sm text-muted-foreground">未找到订阅</div>;
  }

  const isCustom = sub.is_user_defined;

  return (
    <div className="p-8 max-w-3xl space-y-6">
      <Button variant="ghost" size="sm" asChild>
        <Link to="/subscriptions">
          <ArrowLeft className="h-4 w-4" /> 返回列表
        </Link>
      </Button>

      <div className="flex items-start justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold">{sub.display_name}</h1>
          <div className="mt-1 flex items-center gap-3 text-sm">
            <StatusBadge state={sub.state} />
            {isCustom && (
              <span className="text-xs px-2 py-0.5 rounded bg-muted text-muted-foreground">
                🔧 自定义
              </span>
            )}
            {sub.referenced_by.length > 0 && (
              <span className="text-muted-foreground">
                引用: {sub.referenced_by.join(", ")}
              </span>
            )}
          </div>
          {sub.last_error_message && (
            <div className="mt-1 text-xs text-destructive">{sub.last_error_message}</div>
          )}
        </div>
        <div className="flex items-center gap-2">
          <Label className="text-sm">启用</Label>
          <Switch
            checked={sub.enabled}
            onCheckedChange={(checked) =>
              enabledMut.mutate({ id: sub.id, enabled: checked })
            }
          />
        </div>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>基本信息</CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="grid grid-cols-[120px_1fr] gap-3 items-center">
            <Label>厂商</Label>
            {isCustom ? (
              <Input
                value={providerDisplayName}
                onChange={(e) => setProviderDisplayName(e.target.value)}
                placeholder="厂商显示名"
              />
            ) : (
              <div className="flex items-center gap-2 text-sm text-muted-foreground">
                <ProviderIcon iconId={sub.provider_icon} size={18} />
                <span>{sub.provider_display_name}（不可改）</span>
              </div>
            )}
          </div>

          {/* 内置订阅: endpoint 切换下拉 */}
          {!isCustom && (
            <div className="grid grid-cols-[120px_1fr] gap-3 items-start">
              <Label className="mt-2">接入点</Label>
              <div className="space-y-1">
                <Select value={endpointId} onValueChange={setEndpointId}>
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {provider?.endpoints.map((e) => (
                      <SelectItem key={e.id} value={e.id} subtitle={e.base_url}>
                        {e.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <div className="font-mono text-[10px] text-muted-foreground">
                  {sub.base_url}
                  {sub.messages_path}
                </div>
                {endpointId !== sub.endpoint_id && (
                  <div className="text-xs text-amber-600">
                    保存后将从模板重新拷贝该 endpoint 的连接信息
                  </div>
                )}
              </div>
            </div>
          )}

          {/* 自定义订阅: base_url / messages_path / auth 可编辑 */}
          {isCustom && (
            <>
              <div className="grid grid-cols-[120px_1fr] gap-3 items-center">
                <Label>Base URL</Label>
                <Input
                  className="font-mono"
                  value={baseUrl}
                  onChange={(e) => setBaseUrl(e.target.value)}
                  placeholder="https://api.example.com"
                />
              </div>
              <div className="grid grid-cols-[120px_1fr] gap-3 items-center">
                <Label>Messages Path</Label>
                <Input
                  className="font-mono"
                  value={messagesPath}
                  onChange={(e) => setMessagesPath(e.target.value)}
                  placeholder="/v1/messages"
                />
              </div>
              <div className="grid grid-cols-[120px_1fr] gap-3 items-center">
                <Label>鉴权 header 名</Label>
                <Input
                  className="font-mono"
                  value={authHeaderName}
                  onChange={(e) => setAuthHeaderName(e.target.value)}
                  placeholder="Authorization 或 x-api-key"
                />
              </div>
              <div className="grid grid-cols-[120px_1fr] gap-3 items-center">
                <Label>鉴权格式</Label>
                <Select
                  value={authHeaderFormat}
                  onValueChange={(v) => setAuthHeaderFormat(v as AuthHeaderFormat)}
                >
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="bearer">Bearer (header 值前加 "Bearer ")</SelectItem>
                    <SelectItem value="raw">Raw (header 值原样填 key)</SelectItem>
                  </SelectContent>
                </Select>
              </div>
            </>
          )}

          <div className="grid grid-cols-[120px_1fr] gap-3 items-center">
            <Label>备注名</Label>
            <Input value={displayName} onChange={(e) => setDisplayName(e.target.value)} />
          </div>
          <div className="grid grid-cols-[120px_1fr] gap-3 items-center">
            <Label>API Key</Label>
            <div className="flex items-center gap-2">
              <Input type="password" value="••••••••••••••" disabled />
              <Button variant="outline" size="sm" onClick={() => setKeyDialog(true)}>
                修改
              </Button>
            </div>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>模型槽位</CardTitle>
        </CardHeader>
        <CardContent>
          <ModelSlotPicker
            value={slots}
            onChange={setSlots}
            models={models}
            loading={fetchingModels}
            error={modelError}
            onRefresh={refreshModels}
            exampleModels={sub.model_discovery.example_models}
          />
          {sub.model_cache && (
            <div className="mt-3 text-xs text-muted-foreground">
              模型列表更新: {new Date(sub.model_cache.fetched_at).toLocaleString("zh-CN")}
            </div>
          )}
        </CardContent>
      </Card>

      {testResult && (
        <Alert variant={testResult.ok ? "default" : "destructive"}>
          <AlertDescription>
            <div>{testResult.message}</div>
            {testResult.model_used && (
              <div className="mt-1 text-xs opacity-80">
                测试模型: <code className="font-mono">{testResult.model_used}</code>
              </div>
            )}
            {testResult.state_reset && (
              <div className="mt-1 text-xs">✓ 订阅状态已重置为正常</div>
            )}
          </AlertDescription>
        </Alert>
      )}

      {saveError && (
        <Alert variant="destructive">
          <AlertDescription>{saveError}</AlertDescription>
        </Alert>
      )}

      <div className="flex justify-between">
        <Button variant="outline" onClick={testConnection}>
          测试连接
        </Button>
        <div className="flex gap-2">
          <Button variant="destructive" onClick={() => setDeleteDialog(true)}>
            删除
          </Button>
          <Button onClick={save} disabled={updateMut.isPending}>
            {updateMut.isPending && <Loader2 className="h-4 w-4 animate-spin" />}
            保存
          </Button>
        </div>
      </div>

      <Dialog open={keyDialog} onOpenChange={setKeyDialog}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>修改 API Key</DialogTitle>
            <DialogDescription>新 Key 会立即生效。</DialogDescription>
          </DialogHeader>
          <Input
            type="password"
            value={newKey}
            onChange={(e) => setNewKey(e.target.value)}
            placeholder="新的 API Key"
          />
          <DialogFooter>
            <Button variant="ghost" onClick={() => setKeyDialog(false)}>
              取消
            </Button>
            <Button onClick={confirmUpdateKey} disabled={!newKey}>
              保存
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={deleteDialog} onOpenChange={setDeleteDialog}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              <div className="flex items-center gap-2">
                <AlertTriangle className="h-4 w-4 text-destructive" />
                删除 "{sub.display_name}"?
              </div>
            </DialogTitle>
            <DialogDescription>
              {sub.referenced_by.length > 0 ? (
                <>
                  该订阅被 {sub.referenced_by.length} 个虚拟模型引用：
                  <ul className="mt-2 list-disc pl-5">
                    {sub.referenced_by.map((name) => (
                      <li key={name}>{name}</li>
                    ))}
                  </ul>
                  <p className="mt-2">删除后这些虚拟模型的订阅列表会少一个。</p>
                </>
              ) : (
                "删除订阅会同时清除存储的 API Key。"
              )}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="ghost" onClick={() => setDeleteDialog(false)}>
              取消
            </Button>
            <Button variant="destructive" onClick={confirmDelete}>
              确认删除
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
