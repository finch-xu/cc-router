import { useEffect, useMemo, useState } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router";
import { ArrowLeft, TriangleAlert, LoaderCircle } from "lucide-react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { runtime } from "@/runtime";
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
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { StatusBadge } from "@/components/StatusBadge";
import { ProviderIcon } from "@/components/ProviderIcon";
import {
  ModelListModeBar,
  ModelSlotPicker,
  useModelListMode,
} from "@/components/ModelSlotPicker";
import { SlotScheduleEditor } from "@/components/schedule/SlotScheduleEditor";
import { SubscriptionBalancePanel } from "@/components/SubscriptionBalanceCard";
import { SubscriptionQuotaCard } from "@/components/SubscriptionQuotaCard";
import { api } from "@/api/tauri";
import { KeyValueEditor, type KeyValueRow } from "@/components/KeyValueEditor";
import { validateConnection, validateHeaderPolicyNames, validateRequiredHeaders } from "@/lib/connectionValidation";
import { HeaderPolicyEditor } from "@/components/HeaderPolicyEditor";
import { useProviders } from "@/hooks/useProviders";
import {
  useDeleteSubscription,
  useSetSubscriptionEnabled,
  useUpdateSubscription,
  useUpdateSubscriptionKey,
} from "@/hooks/useSubscriptions";
import { useT } from "@/i18n";
import type {
  AuthHeaderFormat,
  ModelInfo,
  ModelSlots,
  RefreshModelListResult,
  SlotEfforts,
  SlotSchedules,
  SubscriptionPatch,
  TestConnectionResult, HeaderPolicy,
} from "@/types";
import { MODEL_SLOT_KEYS, parseSlotParam, uniformSlots } from "@/lib/modelSlots";
import { normalizeSchedules, schedulesEqual, validateRules } from "@/lib/slotSchedule";
import { lastErrorText, testResultText } from "@/lib/backendText";
import { providerName } from "@/lib/providerText";
import { UrlParamFields, urlParamErrors } from "@/components/UrlParamFields";
import { errorText } from "@/lib/errorText";

/** 请求头编辑器的行 → 预览用的名值表 (与保存时同样 trim、丢弃空名)。 */
function draftHeaderMap(rows: KeyValueRow[]): Record<string, string> {
  return Object.fromEntries(
    rows.map((r) => [r.key.trim(), r.value.trim()] as const).filter(([k]) => k !== ""),
  );
}

export function SubscriptionEditPage() {
  const { t, locale } = useT();
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  // 虚拟模型页「编辑槽位」带 ?slot=<槽位>, 打开时停在那个槽位的标签上
  const [searchParams] = useSearchParams();
  const initialSlot = parseSlotParam(searchParams.get("slot")) ?? undefined;

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
  const [urlParams, setUrlParams] = useState<Record<string, string>>({});
  const [showParamErrors, setShowParamErrors] = useState(false);
  const [displayName, setDisplayName] = useState<string>("");
  const [slots, setSlots] = useState<ModelSlots>(uniformSlots(""));
  const [slotEfforts, setSlotEfforts] = useState<SlotEfforts>({});
  const [schedules, setSchedules] = useState<SlotSchedules>({});

  // 自定义订阅可编辑的连接字段
  const [baseUrl, setBaseUrl] = useState<string>("");
  const [messagesPath, setMessagesPath] = useState<string>("");
  const [authHeaderName, setAuthHeaderName] = useState<string>("");
  const [authHeaderFormat, setAuthHeaderFormat] = useState<AuthHeaderFormat>("bearer");
  const [providerDisplayName, setProviderDisplayName] = useState<string>("");
  const [requiredHeaders, setRequiredHeaders] = useState<KeyValueRow[]>([]);
  const [headerPolicy, setHeaderPolicy] = useState<HeaderPolicy>({ mode: "none", names: [] });
  /** 内置订阅的自定义请求头 (extra_headers); 自定义订阅继续编辑 required_headers */
  const [extraHeaders, setExtraHeaders] = useState<KeyValueRow[]>([]);

  const [models, setModels] = useState<ModelInfo[] | null>(null);
  const [fetchingModels, setFetchingModels] = useState(false);
  const [modelError, setModelError] = useState<string | null>(null);
  /** 模型列表的自动 / 手动, 开关在槽位卡片右上角 (手动时分时段编辑也用文本输入) */
  const [modelMode, chooseModelMode] = useModelListMode(models, modelError);
  const [testResult, setTestResult] = useState<TestConnectionResult | null>(null);
  const [keyDialog, setKeyDialog] = useState(false);
  const [newKey, setNewKey] = useState("");
  const [deleteDialog, setDeleteDialog] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  /** 时段规则与槽位分两次提交, updateMut.isPending 盖不住规则那一段 */
  const [saving, setSaving] = useState(false);
  const modelIds = useMemo(() => (models ?? []).map((m) => m.id), [models]);

  useEffect(() => {
    if (subQuery.data) {
      setEndpointId(subQuery.data.endpoint_id);
      setUrlParams(subQuery.data.url_params ?? {});
      setShowParamErrors(false);
      setDisplayName(subQuery.data.display_name);
      setSlots(subQuery.data.model_slots);
      setSlotEfforts(subQuery.data.slot_efforts ?? {});
      setSchedules(subQuery.data.slot_schedules ?? {});
      setBaseUrl(subQuery.data.base_url);
      setMessagesPath(subQuery.data.messages_path);
      setAuthHeaderName(subQuery.data.auth_header_name);
      setAuthHeaderFormat(subQuery.data.auth_header_format);
      setProviderDisplayName(subQuery.data.provider_display_name);
      setHeaderPolicy(subQuery.data.header_policy ?? { mode: "none", names: [] });
      setExtraHeaders(
        Object.entries(subQuery.data.extra_headers ?? {}).map(([key, value]) => ({ key, value })),
      );
      setRequiredHeaders(
        Object.entries(subQuery.data.required_headers ?? {}).map(([key, value]) => ({
          key,
          value,
        })),
      );
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
      const result: RefreshModelListResult = await runtime.invoke("refresh_model_list", { id });
      if (result.kind === "auto") {
        setModels(result.models);
      } else {
        setModels(null);
        setModelError(result.reason);
      }
    } catch (e) {
      setModelError(errorText(e));
    } finally {
      setFetchingModels(false);
    }
  }

  async function save() {
    if (!id || !sub) return;
    setSaveError(null);
    // 自定义 System One 订阅没有示例模型可回退 (后端 require_jev_model 同规则)
    if (sub.is_user_defined && sub.endpoint_protocol === "systemone" && !slots.jev?.trim()) {
      return setSaveError(t("subscriptionNew.errFillJevModel"));
    }
    // 时段规则先整体校验: 任一不过就什么都不发 (与后端 validate_slot_schedules 同规则)
    if (!isSystemOne) {
      for (const k of MODEL_SLOT_KEYS) {
        const errKey = validateRules(schedules[k] ?? []);
        if (errKey) return setSaveError(t("schedule.edit.saveInvalid", { slot: `model-${k}`, msg: t(errKey) }));
      }
    }
    const patch: SubscriptionPatch = {
      display_name: displayName,
      model_slots: slots,
      slot_efforts: slotEfforts,
    };
    // 仅透传订阅有透传策略 (翻译类订阅不渲染也不提交, 保持后端字段不变)
    if (sub.auth_type === "api_key" && !isSystemOne) {
      const nameErr = validateHeaderPolicyNames(headerPolicy.names);
      if (nameErr) return setSaveError(t(nameErr));
      patch.header_policy = headerPolicy;
    }
    if (!sub.is_user_defined) {
      const extraRows = extraHeaders
        .map((r) => ({ key: r.key.trim(), value: r.value.trim() }))
        .filter((r) => r.key !== "" || r.value !== "");
      const extraErr = validateRequiredHeaders(extraRows);
      if (extraErr) return setSaveError(t(extraErr));
      patch.extra_headers = Object.fromEntries(extraRows.map((r) => [r.key, r.value]));
    }
    if (sub.is_user_defined) {
      const connErrKey = validateConnection({ base_url: baseUrl, messages_path: messagesPath });
      if (connErrKey) return setSaveError(t(connErrKey));
      const headerRows = requiredHeaders
        .map((r) => ({ key: r.key.trim(), value: r.value.trim() }))
        .filter((r) => r.key !== "" || r.value !== ""); // 全空行静默丢弃
      const hdrErr = validateRequiredHeaders(headerRows);
      if (hdrErr) return setSaveError(t(hdrErr));
      patch.connection = {
        base_url: baseUrl.trim(),
        messages_path: messagesPath.trim(),
        auth_header_name: authHeaderName.trim(),
        auth_header_format: authHeaderFormat,
        provider_display_name: providerDisplayName.trim(),
        required_headers: Object.fromEntries(headerRows.map((r) => [r.key, r.value])),
      };
    } else {
      // 内置订阅: 切 endpoint 走 endpoint_id patch (后端 re-snapshot)
      const selectedEndpoint = provider?.endpoints.find((e) => e.id === endpointId);
      if (provider && selectedEndpoint) {
        if (Object.keys(urlParamErrors(provider, selectedEndpoint, urlParams)).length > 0) {
          setShowParamErrors(true);
          return;
        }
        const currentParams = Object.fromEntries(
          selectedEndpoint.url_params_used.map((pid) => [pid, (urlParams[pid] ?? "").trim()]),
        );
        const paramsChanged = JSON.stringify(currentParams) !== JSON.stringify(sub.url_params ?? {});
        if (endpointId !== sub.endpoint_id) patch.endpoint_id = endpointId;
        if (paramsChanged || endpointId !== sub.endpoint_id) patch.url_params = currentParams;
      } else if (endpointId !== sub.endpoint_id) {
        patch.endpoint_id = endpointId;
      }
    }
    setSaving(true);
    try {
      // 规则独立成列, 有改动才提交 (systemone 订阅后端拒绝规则); 先于默认模型提交:
      // 反过来时第二步失败会留下「默认已清空、规则没存上」的空槽 (此后不会被调用)
      if (!isSystemOne && !schedulesEqual(schedules, sub.slot_schedules ?? {})) {
        await api.updateSlotSchedules(id, normalizeSchedules(schedules));
        queryClient.invalidateQueries({ queryKey: ["subscriptions"] });
        queryClient.invalidateQueries({ queryKey: ["subscription", id] });
      }
      await updateMut.mutateAsync({ id, patch });
    } catch (e) {
      setSaveError(`${t("subscriptionEdit.errSave")}: ${errorText(e)}`);
    } finally {
      setSaving(false);
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
  const isSystemOne = sub?.endpoint_protocol === "systemone";

  if (subQuery.isLoading) {
    return <div className="p-8 text-sm text-muted-foreground">{t("common.loading")}</div>;
  }

  if (!sub) {
    return <div className="p-8 text-sm text-muted-foreground">{t("subscriptionEdit.notFound")}</div>;
  }

  const isCustom = sub.is_user_defined;
  const selectedEndpointForParams = provider?.endpoints.find((e) => e.id === endpointId);

  return (
    <>
      <div className="page-bar">
        <div className="page-col page-bar-lead">
          <Button
            variant="ghost"
            size="icon"
            className="h-8 w-8 shrink-0"
            asChild
            title={t("subscriptionNew.backToList")}
            aria-label={t("subscriptionNew.backToList")}
          >
            <Link to="/subscriptions">
              <ArrowLeft className="h-4 w-4" />
            </Link>
          </Button>
          <h1 title={sub.display_name}>{sub.display_name}</h1>
          <StatusBadge state={sub.state} className="shrink-0" />
          {isCustom && (
            <span className="shrink-0 text-xs px-2 py-0.5 rounded bg-muted text-muted-foreground">
              🔧 {t("subscriptions.custom")}
            </span>
          )}
        </div>
      </div>

      <div className="page-flow">
        <div className="page-flow-pad">
          <div className="page-col space-y-6">
            {(sub.referenced_by.length > 0 || sub.last_error_message) && (
              <div className="space-y-1">
                {sub.referenced_by.length > 0 && (
                  <div className="text-sm text-muted-foreground">
                    {t("subscriptionEdit.referencedPrefix")}{sub.referenced_by.join(", ")}
                  </div>
                )}
                {sub.last_error_message && (
                  <div className="text-xs text-destructive">
                    {lastErrorText(sub.last_error_code, sub.last_error_message, t)}
                  </div>
                )}
              </div>
            )}

            <Card>
              <CardHeader>
                <CardTitle>{t("subscriptionEdit.basicInfo")}</CardTitle>
              </CardHeader>
              <CardContent className="basic-info-body">
                <div className="basic-info-row">
                  <div className="basic-info-fields space-y-4">
                    <div className="grid grid-cols-[120px_1fr] gap-3 items-center">
                      <Label>{t("subscriptionNew.field.provider")}</Label>
                      {isCustom ? (
                        <Input
                          value={providerDisplayName}
                          onChange={(e) => setProviderDisplayName(e.target.value)}
                          placeholder={t("subscriptionEdit.providerNamePh")}
                        />
                      ) : (
                        <div className="flex items-center gap-2 text-sm text-muted-foreground">
                          <ProviderIcon iconId={sub.provider_icon} size={18} />
                          <span>{providerName(sub, locale)}{t("subscriptionEdit.providerLocked")}</span>
                        </div>
                      )}
                    </div>

                    {/* 内置订阅: endpoint 切换下拉 */}
                    {!isCustom && (
                      <div className="grid grid-cols-[120px_1fr] gap-3 items-start">
                        <Label className="mt-2">{t("subscriptionNew.field.endpoint")}</Label>
                        <div className="space-y-1 min-w-0">
                          <Select value={endpointId} onValueChange={setEndpointId}>
                            <SelectTrigger>
                              <SelectValue />
                            </SelectTrigger>
                            <SelectContent>
                              {provider?.endpoints.filter((e) => e.protocol === sub.endpoint_protocol).map((e) => (
                                <SelectItem key={e.id} value={e.id} subtitle={e.base_url}>
                                  {e.label}
                                </SelectItem>
                              ))}
                            </SelectContent>
                          </Select>
                          <div className="font-mono text-[10px] text-muted-foreground break-all">
                            {sub.base_url}
                            {sub.messages_path}
                          </div>
                          {endpointId !== sub.endpoint_id && (
                            <div className="text-xs text-amber-600">
                              {t("subscriptionEdit.endpointChangeWarn")}
                            </div>
                          )}
                        </div>
                      </div>
                    )}

                    {!isCustom && provider && selectedEndpointForParams && (
                      <UrlParamFields
                        provider={provider}
                        endpoint={selectedEndpointForParams}
                        values={urlParams}
                        onChange={(pid, v) => setUrlParams((prev) => ({ ...prev, [pid]: v }))}
                        showErrors={showParamErrors}
                        layout="grid"
                      />
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
                          <Label>{t("subscriptionEdit.authHeaderName")}</Label>
                          <Input
                            className="font-mono"
                            value={authHeaderName}
                            onChange={(e) => setAuthHeaderName(e.target.value)}
                            placeholder={t("subscriptionEdit.authHeaderNamePh")}
                          />
                        </div>
                        <div className="grid grid-cols-[120px_1fr] gap-3 items-center">
                          <Label>{t("subscriptionEdit.authHeaderFormat")}</Label>
                          <Select
                            value={authHeaderFormat}
                            onValueChange={(v) => setAuthHeaderFormat(v as AuthHeaderFormat)}
                          >
                            <SelectTrigger>
                              <SelectValue />
                            </SelectTrigger>
                            <SelectContent>
                              <SelectItem value="bearer">{t("subscriptionEdit.authBearerOption")}</SelectItem>
                              <SelectItem value="raw">{t("subscriptionEdit.authRawOption")}</SelectItem>
                            </SelectContent>
                          </Select>
                        </div>
                        <div className="grid grid-cols-[120px_1fr] gap-3 items-start">
                          <Label className="mt-2">{t("subscriptionEdit.requiredHeaders")}</Label>
                          <div className="space-y-1">
                            <KeyValueEditor
                              value={requiredHeaders}
                              onChange={setRequiredHeaders}
                              keyPlaceholder={t("subscriptionEdit.requiredHeaderKeyPh")}
                              valuePlaceholder={t("subscriptionEdit.requiredHeaderValuePh")}
                              addLabel={t("subscriptionEdit.requiredHeadersAdd")}
                              removeAriaLabel={t("subscriptionEdit.requiredHeadersRemove")}
                            />
                            <p className="text-xs text-muted-foreground">
                              {t("subscriptionEdit.requiredHeadersHint")}
                            </p>
                          </div>
                        </div>
                      </>
                    )}

                    <div className="grid grid-cols-[120px_1fr] gap-3 items-center">
                      <Label>{t("subscriptionNew.field.note")}</Label>
                      <Input value={displayName} onChange={(e) => setDisplayName(e.target.value)} />
                    </div>
                    <div className="grid grid-cols-[120px_1fr] gap-3 items-center">
                      <Label>API Key</Label>
                      <div className="flex items-center gap-2">
                        <Input type="password" value="••••••••••••••" disabled />
                        <Button variant="outline" size="sm" onClick={() => setKeyDialog(true)}>
                          {t("common.modify")}
                        </Button>
                      </div>
                    </div>

                    {/* 内置订阅的自定义请求头 (自定义订阅在上方连接区编辑 required_headers) */}
                    {!isCustom && (
                      <div className="grid grid-cols-[120px_1fr] gap-3 items-start">
                        <Label className="mt-2">{t("subscriptionEdit.extraHeaders")}</Label>
                        <div className="space-y-1">
                          <KeyValueEditor
                            value={extraHeaders}
                            onChange={setExtraHeaders}
                            keyPlaceholder={t("subscriptionEdit.requiredHeaderKeyPh")}
                            valuePlaceholder={t("subscriptionEdit.requiredHeaderValuePh")}
                            addLabel={t("subscriptionEdit.requiredHeadersAdd")}
                            removeAriaLabel={t("subscriptionEdit.requiredHeadersRemove")}
                          />
                          <p className="text-xs text-muted-foreground">{t("subscriptionEdit.extraHeadersHint")}</p>
                        </div>
                      </div>
                    )}

                    {/* 仅 Anthropic 透传订阅: 客户端请求头透传策略 (翻译类协议不消费此字段) */}
                    {sub.auth_type === "api_key" && !isSystemOne && (
                      <div className="grid grid-cols-[120px_1fr] gap-3 items-start">
                        <Label className="mt-2">{t("subscriptionEdit.headerPolicy.label")}</Label>
                        <HeaderPolicyEditor
                          value={headerPolicy}
                          onChange={setHeaderPolicy}
                          subscriptionId={sub.id}
                          draftHeaders={draftHeaderMap(isCustom ? requiredHeaders : extraHeaders)}
                          draftAuth={isCustom ? { name: authHeaderName, format: authHeaderFormat } : undefined}
                        />
                      </div>
                    )}
                  </div>
                  <SubscriptionBalancePanel
                    subscription={sub}
                    onChanged={() => {
                      queryClient.invalidateQueries({ queryKey: ["subscription", id] });
                      queryClient.invalidateQueries({ queryKey: ["subscriptions"] });
                    }}
                  />
                </div>
              </CardContent>
            </Card>

            <Card>
              <CardHeader className="slot-card-head flex-row flex-wrap items-start justify-between gap-x-5 gap-y-3 space-y-0">
                <div className="min-w-0 space-y-1.5">
                  <CardTitle>{t("subscriptionEdit.modelSlotsTitle")}</CardTitle>
                  {!isSystemOne && (
                    <p className="text-sm text-muted-foreground">{t("schedule.edit.cardDesc")}</p>
                  )}
                </div>
                {!isSystemOne && (
                  <ModelListModeBar
                    label={t("schedule.edit.modelListLabel")}
                    mode={modelMode}
                    onChoose={chooseModelMode}
                    onRefresh={refreshModels}
                    loading={fetchingModels}
                  />
                )}
              </CardHeader>
              <CardContent>
                {!isSystemOne && (
                  <div className="mb-6 border-b border-dashed border-(--line) pb-6">
                    <SlotScheduleEditor
                      slots={slots}
                      onSlotsChange={setSlots}
                      schedules={schedules}
                      onSchedulesChange={setSchedules}
                      models={modelIds}
                      manual={modelMode === "manual"}
                      initialSlot={initialSlot}
                      efforts={slotEfforts}
                      onEffortsChange={setSlotEfforts}
                      effortDisabled={sub.auth_type === "kiro_oauth"}
                      effortDisabledReason={t("slotEffort.unsupportedKiro")}
                    />
                  </div>
                )}
                <ModelSlotPicker
                  value={slots}
                  onChange={setSlots}
                  efforts={slotEfforts}
                  onEffortsChange={setSlotEfforts}
                  effortDisabled={sub.auth_type === "kiro_oauth"}
                  effortDisabledReason={t("slotEffort.unsupportedKiro")}
                  models={models}
                  loading={fetchingModels}
                  error={modelError}
                  onRefresh={isSystemOne ? undefined : refreshModels}
                  exampleModels={
                    isSystemOne
                      ? provider?.endpoints.find((e) => e.id === endpointId)?.example_models ?? []
                      : sub.model_discovery.example_models
                  }
                  systemone={isSystemOne}
                  jevRequired={isSystemOne && sub.is_user_defined}
                  hideMainSlots={!isSystemOne}
                  mode={isSystemOne ? undefined : modelMode}
                />
                {sub.model_cache && !isSystemOne && (
                  <div className="mt-3 text-xs text-muted-foreground">
                    {t("subscriptionEdit.modelCacheUpdated")}{new Date(sub.model_cache.fetched_at).toLocaleString()}
                  </div>
                )}
              </CardContent>
            </Card>

            <SubscriptionQuotaCard
              subscription={sub}
              onChanged={() => {
                queryClient.invalidateQueries({ queryKey: ["subscription", id] });
                queryClient.invalidateQueries({ queryKey: ["subscriptions"] });
              }}
            />
          </div>
        </div>
      </div>

      {/* 底部常驻操作区: 与顶栏对称地挂在 .page-flow 之外, 不随内容滚动。
          反馈紧挨着触发它的按钮 —— 结果若跟着内容滚动, 用户在内容区底部点保存就看不见。 */}
      <div className="page-foot">
        {(testResult || saveError) && (
          <div className="page-foot-feedback">
            <div className="page-col space-y-2">
              {testResult && (
                <Alert variant={testResult.ok ? "default" : "destructive"}>
                  {/* 失败必须有明确标题: 只靠边框颜色区分成败, 用户看不出来 (issue #63) */}
                  {!testResult.ok && <AlertTitle>{t("testConnection.failed")}</AlertTitle>}
                  <AlertDescription>
                    <div>{testResultText(testResult, t)}</div>
                    {testResult.model_used && (
                      <div className="mt-1 text-xs opacity-80">
                        {t("subscriptionEdit.testModel")}<code className="font-mono">{testResult.model_used}</code>
                      </div>
                    )}
                    {testResult.state_reset && (
                      <div className="mt-1 text-xs">{t("subscriptionEdit.stateReset")}</div>
                    )}
                  </AlertDescription>
                </Alert>
              )}
              {saveError && (
                <Alert variant="destructive">
                  <AlertDescription>{saveError}</AlertDescription>
                </Alert>
              )}
            </div>
          </div>
        )}
        <div className="page-foot-bar">
          <div className="page-col page-foot-inner">
            <Label className="text-sm">{t("subscriptionEdit.enabled")}</Label>
            <Switch
              checked={sub.enabled}
              onCheckedChange={(checked) =>
                enabledMut.mutate({ id: sub.id, enabled: checked })
              }
            />
            <div className="page-foot-actions">
              <Button variant="outline" size="sm" onClick={testConnection}>
                {t("subscriptionEdit.testConn")}
              </Button>
              <Button variant="destructive" size="sm" onClick={() => setDeleteDialog(true)}>
                {t("common.delete")}
              </Button>
              <Button size="sm" onClick={save} disabled={saving}>
                {saving && <LoaderCircle className="h-4 w-4 animate-spin" />}
                {t("common.save")}
              </Button>
            </div>
          </div>
        </div>
      </div>

      <Dialog open={keyDialog} onOpenChange={setKeyDialog}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("subscriptionEdit.keyDialog.title")}</DialogTitle>
            <DialogDescription>{t("subscriptionEdit.keyDialog.desc")}</DialogDescription>
          </DialogHeader>
          <Input
            type="password"
            value={newKey}
            onChange={(e) => setNewKey(e.target.value)}
            placeholder={t("subscriptionEdit.keyDialog.placeholder")}
          />
          <DialogFooter>
            <Button variant="ghost" onClick={() => setKeyDialog(false)}>
              {t("common.cancel")}
            </Button>
            <Button onClick={confirmUpdateKey} disabled={!newKey}>
              {t("common.save")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={deleteDialog} onOpenChange={setDeleteDialog}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              <div className="flex items-center gap-2">
                <TriangleAlert className="h-4 w-4 text-destructive" />
                {t("subscriptionEdit.deleteDialog.titlePrefix")}{sub.display_name}{t("subscriptionEdit.deleteDialog.titleSuffix")}
              </div>
            </DialogTitle>
            <DialogDescription>
              {sub.referenced_by.length > 0 ? (
                <>
                  {t("subscriptionEdit.deleteDialog.refByPrefix")}{sub.referenced_by.length}{t("subscriptionEdit.deleteDialog.refBySuffix")}
                  <ul className="mt-2 list-disc pl-5">
                    {sub.referenced_by.map((name) => (
                      <li key={name}>{name}</li>
                    ))}
                  </ul>
                  <p className="mt-2">{t("subscriptionEdit.deleteDialog.refByOutro")}</p>
                </>
              ) : (
                t("subscriptionEdit.deleteDialog.noRef")
              )}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="ghost" onClick={() => setDeleteDialog(false)}>
              {t("common.cancel")}
            </Button>
            <Button variant="destructive" onClick={confirmDelete}>
              {t("subscriptionEdit.deleteDialog.confirm")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
