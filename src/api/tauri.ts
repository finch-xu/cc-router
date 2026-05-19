import { invoke } from "@tauri-apps/api/core";
import type {
  BreakdownBy,
  BreakdownDto,
  ChatGptAccount,
  CreateChatGptOAuthSubscriptionInput,
  CreateKiroSubscriptionInput,
  CreateSubscriptionInput,
  DailySeriesPointDto,
  DeviceFlowStart,
  EventFilters,
  HeatmapDayDto,
  KiroAccount,
  KiroDeviceFlowStart,
  KiroDisguise,
  KiroImportResult,
  ListEventsResult,
  ListRequestsResult,
  OnboardingState,
  OverallStatsDto,
  ProviderInfo,
  ProxyStatus,
  ReceiptDto,
  ReceiptRange,
  RefreshBalanceResult,
  RefreshModelListResult,
  RequestLogFilters,
  Settings,
  SettingsPatch,
  StatsRange,
  SubscriptionDto,
  SubscriptionPatch,
  TestConnectionResult,
  TlsStatus,
  UpdateInfo,
  UpdateVirtualModelInput,
  VirtualModelDto,
  VirtualModelName,
} from "@/types";

export const api = {
  // providers
  listProviders: () => invoke<ProviderInfo[]>("list_providers"),

  // subscriptions
  listSubscriptions: () => invoke<SubscriptionDto[]>("list_subscriptions"),
  getSubscription: (id: string) =>
    invoke<SubscriptionDto>("get_subscription", { id }),
  createSubscription: (input: CreateSubscriptionInput) =>
    invoke<SubscriptionDto>("create_subscription", { input }),
  updateSubscription: (id: string, patch: SubscriptionPatch) =>
    invoke<SubscriptionDto>("update_subscription", { id, patch }),
  updateSubscriptionKey: (id: string, newKey: string) =>
    invoke<void>("update_subscription_key", { id, newKey }),
  deleteSubscription: (id: string) =>
    invoke<void>("delete_subscription", { id }),
  setSubscriptionEnabled: (id: string, enabled: boolean) =>
    invoke<void>("set_subscription_enabled", { id, enabled }),
  testConnection: (id: string) =>
    invoke<TestConnectionResult>("test_connection", { id }),
  refreshModelList: (id: string) =>
    invoke<RefreshModelListResult>("refresh_model_list", { id }),
  refreshSubscriptionBalance: (id: string) =>
    invoke<RefreshBalanceResult>("refresh_subscription_balance", { id }),

  // ChatGPT OAuth (Phase 1, OpenAI Codex provider)
  startChatGptDeviceFlow: () =>
    invoke<DeviceFlowStart>("start_chatgpt_device_flow"),
  pollChatGptDeviceCode: (deviceCode: string) =>
    invoke<ChatGptAccount | null>("poll_chatgpt_device_code", { deviceCode }),
  createChatGptOAuthSubscription: (input: CreateChatGptOAuthSubscriptionInput) =>
    invoke<SubscriptionDto>("create_chatgpt_oauth_subscription", { input }),
  forgetChatGptOAuthCache: (subscriptionId: string) =>
    invoke<void>("forget_chatgpt_oauth_cache", { subscriptionId }),

  // Kiro OAuth (方案 A JSON 导入 + 方案 B AWS Builder ID OIDC Device Flow)
  importKiroCredentialsFromFile: (path: string) =>
    invoke<KiroImportResult>("import_kiro_credentials_from_file", { path }),
  importKiroCredentialsFromText: (json: string) =>
    invoke<KiroImportResult>("import_kiro_credentials_from_text", { json }),
  startKiroDeviceFlow: (region?: string) =>
    invoke<KiroDeviceFlowStart>("start_kiro_device_flow", { region }),
  pollKiroDeviceCode: (deviceCode: string) =>
    invoke<KiroAccount | null>("poll_kiro_device_code", { deviceCode }),
  createKiroSubscription: (input: CreateKiroSubscriptionInput) =>
    invoke<SubscriptionDto>("create_kiro_subscription", { input }),
  forgetKiroOAuthCache: (subscriptionId: string) =>
    invoke<void>("forget_kiro_oauth_cache", { subscriptionId }),
  updateKiroDisguiseFields: (subscriptionId: string, disguise: KiroDisguise) =>
    invoke<void>("update_kiro_disguise_fields", { subscriptionId, disguise }),

  // virtual models
  listVirtualModels: () => invoke<VirtualModelDto[]>("list_virtual_models"),
  updateVirtualModel: (name: VirtualModelName, input: UpdateVirtualModelInput) =>
    invoke<void>("update_virtual_model", { name, input }),

  // request logs
  listRequests: (
    page: number,
    pageSize: number,
    filters?: RequestLogFilters,
  ) =>
    invoke<ListRequestsResult>("list_requests", { page, pageSize, filters }),

  // statistics (聚合表查询, 跨范围全局)
  getOverallStats: (range: StatsRange) =>
    invoke<OverallStatsDto>("get_overall_stats", { range }),
  getDailySeries: (range: StatsRange) =>
    invoke<DailySeriesPointDto[]>("get_daily_series", { range }),
  getBreakdown: (range: StatsRange, by: BreakdownBy) =>
    invoke<BreakdownDto[]>("get_breakdown", { range, by }),
  getTokenHeatmap: (days: number) =>
    invoke<HeatmapDayDto[]>("get_token_heatmap", { days }),

  getReceiptSummary: (range: ReceiptRange) =>
    invoke<ReceiptDto>("get_receipt_summary", { range }),

  // event stream (kind=request / subscription_state_change / system_error)
  listEvents: (
    page: number,
    pageSize: number,
    filters?: EventFilters,
  ) =>
    invoke<ListEventsResult>("list_events", { page, pageSize, filters }),

  // settings / proxy
  getSettings: () => invoke<Settings>("get_settings"),
  updateSettings: (patch: SettingsPatch) =>
    invoke<Settings>("update_settings", { patch }),
  generateNewToken: () => invoke<Settings>("generate_new_token"),
  proxyStatus: () => invoke<ProxyStatus>("proxy_status"),
  envSnippet: () => invoke<string>("env_snippet"),

  // onboarding
  getOnboardingState: () => invoke<OnboardingState>("get_onboarding_state"),
  completeOnboarding: () => invoke<void>("complete_onboarding"),

  // app
  factoryReset: () => invoke<void>("factory_reset"),

  // 调试模式 dump 目录管理
  openDebugDumpDir: () => invoke<void>("open_debug_dump_dir"),
  clearDebugDumps: () => invoke<void>("clear_debug_dumps"),

  // updater (运行时按 settings.update_source 切换 manifest 源)
  checkForUpdate: () => invoke<UpdateInfo | null>("check_for_update"),
  downloadInstallUpdate: () => invoke<void>("download_install_update"),

  // TLS / HTTPS 证书 (proxy_mode 包含 https 时使用)
  tlsGetStatus: () => invoke<TlsStatus>("tls_get_status"),
  tlsGetCaPemPath: () => invoke<string>("tls_get_ca_pem_path"),
  tlsExportCaPem: (dest: string) => invoke<void>("tls_export_ca_pem", { dest }),
  tlsRegenerateLeaf: () => invoke<TlsStatus>("tls_regenerate_leaf"),
};
