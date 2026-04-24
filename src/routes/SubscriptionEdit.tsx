import { useEffect, useMemo, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { ArrowLeft, AlertTriangle, Loader2 } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
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
import { useProviders } from "@/hooks/useProviders";
import {
  useDeleteSubscription,
  useSetSubscriptionEnabled,
  useUpdateSubscription,
  useUpdateSubscriptionKey,
} from "@/hooks/useSubscriptions";
import type { ModelInfo, ModelSlots, RefreshModelListResult, TestConnectionResult } from "@/types";

export function SubscriptionEditPage() {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();

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

  const provider = useMemo(
    () => providers.data?.find((p) => p.id === subQuery.data?.provider_id),
    [providers.data, subQuery.data],
  );

  const [endpointId, setEndpointId] = useState<string>("");
  const [displayName, setDisplayName] = useState<string>("");
  const [slots, setSlots] = useState<ModelSlots>({ opus: "", sonnet: "", haiku: "" });
  const [models, setModels] = useState<ModelInfo[] | null>(null);
  const [fetchingModels, setFetchingModels] = useState(false);
  const [modelError, setModelError] = useState<string | null>(null);
  const [testResult, setTestResult] = useState<TestConnectionResult | null>(null);
  const [keyDialog, setKeyDialog] = useState(false);
  const [newKey, setNewKey] = useState("");
  const [deleteDialog, setDeleteDialog] = useState(false);

  useEffect(() => {
    if (subQuery.data) {
      setEndpointId(subQuery.data.endpoint_id);
      setDisplayName(subQuery.data.display_name);
      setSlots(subQuery.data.model_slots);
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
    if (!id) return;
    await updateMut.mutateAsync({
      id,
      patch: { endpoint_id: endpointId, display_name: displayName, model_slots: slots },
    });
  }

  async function testConnection() {
    if (!id) return;
    const result = await api.testConnection(id);
    setTestResult(result);
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
            <div className="flex items-center gap-2 text-sm text-muted-foreground">
              <ProviderIcon iconId={provider?.icon} size={18} />
              <span>
                {provider?.display_name ?? sub.provider_id}（不可改）
              </span>
            </div>
          </div>
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
              {provider?.endpoints.find((e) => e.id === endpointId) && (
                <div className="font-mono text-[10px] text-muted-foreground">
                  {provider.endpoints.find((e) => e.id === endpointId)!.base_url}
                  {provider.endpoints.find((e) => e.id === endpointId)!.messages_path}
                </div>
              )}
            </div>
          </div>
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
            exampleModels={provider?.model_discovery.example_models}
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
          <AlertDescription>{testResult.message}</AlertDescription>
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
            <DialogDescription>新 Key 会覆盖 Keychain 中的当前值。</DialogDescription>
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
                "删除订阅会同时清除 Keychain 中存储的 API Key。"
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
