import type { CustomVirtualModelDto, CustomVirtualModelInput } from "@/types";
import { runtime } from "@/runtime";

const invoke = runtime.invoke;
import type {
  InstalledPlugin,
  RebuildPreview,
  CostSummaryDto,
  PluginContent,
  StagedPlugin,
  RequestHeaders,
  HeaderPreview,
  HeaderPolicy,
  AuthHeaderFormat,
  BreakdownBy,
  BreakdownDto,
  BrowserFlowStart,
  ChatGptAccount,
  ClaudeCodeInspectResult,
  ClaudeCodeReadResult,
  ClaudeCodeWriteOutcome,
  ClientActivityDto,
  LiveRequest,
  RequestDetailDto,
  RequestFacetsDto,
  RequestNeighborsDto,
  RequestSummary,
  CreateChatGptOAuthSubscriptionInput,
  CreateKiroSubscriptionInput,
  CreateSubscriptionInput,
  DailySeriesPointDto,
  DeviceFlowStart,
  EventFilters,
  ExportSummary,
  HeatmapDayDto,
  HostClockDto,
  ImportOptions,
  ImportPreview,
  ImportReport,
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
  ReleaseNotesDto,
  RefreshBalanceResult,
  RefreshModelListResult,
  RequestLogFilters,
  RestartProxyResult,
  Settings,
  SettingsPatch,
  SlotSchedules,
  StatsRange,
  StorageStatsDto,
  SubscriptionDto,
  SubscriptionPatch,
  TestConnectionResult,
  TlsStatus,
  TokenQuotas,
  ToolBreakdownDto,
  TuiInstallOutcome,
  TuiLaunchInfo,
  UpdateInfo,
  UpdateVirtualModelInput,
  VirtualModelDto,
  VirtualModelName,
  RequestMixDto,
  SubscriptionActivityDto,
} from "@/types";

export const api = {
  // providers
  listProviders: () => invoke<ProviderInfo[]>("list_providers"),

  // subscriptions
  listSubscriptions: () => invoke<SubscriptionDto[]>("list_subscriptions"),
  getSubscription: (id: string) =>
    invoke<SubscriptionDto>("get_subscription", { id }),
  cloneSubscription: (id: string, displayName: string) =>
    invoke<SubscriptionDto>("clone_subscription", { id, displayName }),
  createSubscription: (input: CreateSubscriptionInput) =>
    invoke<SubscriptionDto>("create_subscription", { input }),
  updateSubscription: (id: string, patch: SubscriptionPatch) =>
    invoke<SubscriptionDto>("update_subscription", { id, patch }),
  /** 厂商描述 (内置 yaml / 厂商插件) 更新后: 预览 / 执行「按新描述重建连接」 */
  previewSubscriptionRebuild: (id: string) => invoke<RebuildPreview>("preview_subscription_rebuild", { id }),
  rebuildSubscriptionConnection: (id: string) => invoke<SubscriptionDto>("rebuild_subscription_connection", { id }),
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
  updateTokenQuotas: (id: string, quotas: TokenQuotas) =>
    invoke<SubscriptionDto>("update_token_quotas", { id, quotas }),
  resetTotalQuotaUsage: (id: string) =>
    invoke<SubscriptionDto>("reset_total_quota_usage", { id }),
  updateSlotSchedules: (id: string, schedules: SlotSchedules) =>
    invoke<SubscriptionDto>("update_slot_schedules", { id, schedules }),
  getHostClock: () => invoke<HostClockDto>("get_host_clock"),

  // ChatGPT OAuth (Phase 1, OpenAI Codex provider)
  startChatGptBrowserFlow: () =>
    invoke<BrowserFlowStart>("start_chatgpt_browser_flow"),
  pollChatGptBrowserFlow: (flowId: string) =>
    invoke<ChatGptAccount | null>("poll_chatgpt_browser_flow", { flowId }),
  cancelChatGptBrowserFlow: (flowId?: string) =>
    invoke<void>("cancel_chatgpt_browser_flow", { flowId: flowId ?? null }),
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
  listCustomVirtualModels: () => invoke<CustomVirtualModelDto[]>("list_custom_virtual_models"),
  createCustomVirtualModel: (model: CustomVirtualModelDto) =>
    invoke<void>("create_custom_virtual_model", { model }),
  updateCustomVirtualModel: (name: string, input: CustomVirtualModelInput) =>
    invoke<void>("update_custom_virtual_model", { name, input }),
  deleteCustomVirtualModel: (name: string) =>
    invoke<void>("delete_custom_virtual_model", { name }),
  listVirtualModels: () => invoke<VirtualModelDto[]>("list_virtual_models"),
  updateVirtualModel: (name: VirtualModelName, input: UpdateVirtualModelInput) =>
    invoke<void>("update_virtual_model", { name, input }),

  // request logs
  getRequestHeaders: (id: string) => invoke<RequestHeaders>("get_request_headers", { id }),
  /**
   * 按编辑页草稿 (未保存也算) 预览发给上游的请求头; headers 是用户可编辑的那张表。
   * auth 只对自定义订阅生效 (内置订阅的鉴权头不可编辑, 传了也被忽略)。
   */
  previewOutboundHeaders: (
    subscriptionId: string,
    headerPolicy: HeaderPolicy,
    headers: Record<string, string>,
    auth?: { name: string; format: AuthHeaderFormat },
  ) =>
    invoke<HeaderPreview>("preview_outbound_headers", {
      subscriptionId,
      headerPolicy,
      headers,
      authHeaderName: auth?.name ?? null,
      authHeaderFormat: auth?.format ?? null,
    }),
  listRequests: (
    page: number,
    pageSize: number,
    filters?: RequestLogFilters,
  ) =>
    invoke<ListRequestsResult>("list_requests", { page, pageSize, filters }),
  /** 按当前筛选导出 CSV 到 path (来自 save dialog), 返回导出行数 */
  exportRequestsCsv: (path: string, filters?: RequestLogFilters) =>
    invoke<number>("export_requests_csv", { path, filters }),
  /** 网页端导出: 返回 CSV 文本 (含 BOM), 由浏览器下载 */
  exportRequestsCsvText: (filters?: RequestLogFilters) =>
    invoke<string>("export_requests_csv_text", { filters }),
  /** Live Routing「客户端接入」: 按 client_tool 聚合的最近活动 (被动流量检测) */
  getClientActivity: () => invoke<ClientActivityDto[]>("get_client_activity"),
  getLiveRequests: (sinceMs?: number) => invoke<LiveRequest[]>("get_live_requests", { sinceMs }),
  /** 详情页; 还没写进库时为 null (批量写入, 最多 5 秒) */
  getRequestDetail: (id: string) => invoke<RequestDetailDto | null>("get_request_detail", { id }),
  listSessionRequests: (sessionId: string, aroundMs: number, radius?: number) =>
    invoke<RequestSummary[]>("list_session_requests", { sessionId, aroundMs, radius }),
  getRequestFacets: (filters?: RequestLogFilters) => invoke<RequestFacetsDto>("get_request_facets", { filters }),
  getRequestNeighbors: (id: string, filters?: RequestLogFilters) =>
    invoke<RequestNeighborsDto>("get_request_neighbors", { id, filters }),

  // config backup / import
  exportConfig: (path: string, password?: string) =>
    invoke<ExportSummary>("export_config", { path, password: password ?? null }),

  exportConfigText: () => invoke<string>("export_config_text"),

  previewConfigImport: (text: string) =>
    invoke<ImportPreview>("preview_config_import", { text }),

  applyConfigImport: (text: string, options: ImportOptions) =>
    invoke<ImportReport>("apply_config_import", { text, options }),

  // statistics (聚合表查询, 跨范围全局)
  getOverallStats: (range: StatsRange) =>
    invoke<OverallStatsDto>("get_overall_stats", { range }),
  /** 统计页费用卡片: 按当前价格表现算 (没有计价插件时 has_pricing = false) */
  getCostSummary: (range: StatsRange) => invoke<CostSummaryDto>("get_cost_summary", { range }),
  getDailySeries: (range: StatsRange) =>
    invoke<DailySeriesPointDto[]>("get_daily_series", { range }),
  getBreakdown: (range: StatsRange, by: BreakdownBy) =>
    invoke<BreakdownDto[]>("get_breakdown", { range, by }),
  getTokenHeatmap: (days: number) =>
    invoke<HeatmapDayDto[]>("get_token_heatmap", { days }),
  getToolBreakdown: (range: StatsRange, limit: number) =>
    invoke<ToolBreakdownDto[]>("get_tool_breakdown", { range, limit }),
  getSubscriptionActivity: () =>
    invoke<SubscriptionActivityDto[]>("get_subscription_activity"),
  getRequestMix: (range: StatsRange) =>
    invoke<RequestMixDto>("get_request_mix", { range }),

  getReceiptSummary: (range: ReceiptRange) =>
    invoke<ReceiptDto>("get_receipt_summary", { range }),

  // event stream (kind=subscription_state_change / system_error / quota_reached)
  listEvents: (
    page: number,
    pageSize: number,
    filters?: EventFilters,
  ) =>
    invoke<ListEventsResult>("list_events", { page, pageSize, filters }),

  // release notes
  getReleaseNotes: () => invoke<ReleaseNotesDto>("get_release_notes"),
  markReleaseNotesSeen: () => invoke<void>("mark_release_notes_seen"),

  // settings / proxy
  getSettings: () => invoke<Settings>("get_settings"),
  updateSettings: (patch: SettingsPatch) =>
    invoke<Settings>("update_settings", { patch }),
  generateNewToken: () => invoke<Settings>("generate_new_token"),
  proxyStatus: () => invoke<ProxyStatus>("proxy_status"),
  restartProxy: () => invoke<RestartProxyResult>("restart_proxy"),
  envSnippet: () => invoke<string>("env_snippet"),
  /** 局域网 IPv4 列表 (非回环), 设置页拼网页访问地址用 */
  listLanAddresses: () => invoke<string[]>("list_lan_addresses"),
  tuiLaunchInfo: () => invoke<TuiLaunchInfo>("tui_launch_info"),
  /** 仅桌面端可用; 网页端后端直接拒绝 */
  installTuiCommand: () => invoke<TuiInstallOutcome>("install_tui_command"),
  uninstallTuiCommand: () => invoke<TuiInstallOutcome>("uninstall_tui_command"),

  // plugins (下载 / 安装 / 卸载仅桌面端; 网页端后端直接拒绝)
  listPlugins: () => invoke<InstalledPlugin[]>("list_plugins"),
  loadPlugin: (id: string) => invoke<PluginContent>("load_plugin", { id }),
  stagePluginFromGithub: (link: string) => invoke<StagedPlugin>("stage_plugin_from_github", { link }),
  stagePluginFromDir: (path: string) => invoke<StagedPlugin>("stage_plugin_from_dir", { path }),
  installStagedPlugin: (stagingId: string, communitySha256?: string) =>
    invoke<InstalledPlugin>("install_staged_plugin", { stagingId, communitySha256: communitySha256 ?? null }),
  discardStagedPlugin: (stagingId: string) => invoke<void>("discard_staged_plugin", { stagingId }),
  uninstallPlugin: (id: string) => invoke<void>("uninstall_plugin", { id }),
  /** 启用系统插件: 仅桌面端 (网页端拒绝); 停用两边都能用 */
  enablePlugin: (id: string) => invoke<void>("enable_plugin", { id }),
  disablePlugin: (id: string) => invoke<void>("disable_plugin", { id }),
  /** 保存系统插件用户参数 (网页端也开放: 改参数本身不启用插件) */
  setPluginParams: (id: string, params: Record<string, unknown>) => invoke<void>("set_plugin_params", { id, params }),
  /** 通知插件测试推送 (仅桌面端; 网页端拒绝) */
  testNotifierPlugin: (id: string) => invoke<import("@/types").NotifierDeliveryRecord>("test_notifier_plugin", { id }),
  /** 查询通知插件最近发送记录 (内存最近 20 条, 不含请求体) */
  listNotifierDeliveries: (id: string) => invoke<import("@/types").NotifierDeliveryRecord[]>("list_notifier_deliveries", { id }),
  /** 发送系统桌面测试通知 (仅桌面端; 网页端拒绝) */
  testDesktopNotification: () => invoke<void>("test_desktop_notification"),
  /** 调整计价 / 规则插件的优先级顺序 (仅桌面端; 网页端拒绝) */
  reorderPlugins: (pluginType: "pricing" | "rules", orderedIds: string[]) =>
    invoke<void>("reorder_plugins", { pluginType, orderedIds }),
  /** 规则插件试运行 (抽最近 N 条请求日志模拟匹配, 纯只读) */
  dryRunRulesPlugin: (id: string, params?: Record<string, unknown>, limit?: number) =>
    invoke<import("@/types").RulesDryRunResult>("dry_run_rules_plugin", {
      id,
      params: params ?? null,
      limit: limit ?? null,
    }),
  fetchCommunityIndex: () => invoke<unknown>("fetch_community_index"),

  // onboarding
  getOnboardingState: () => invoke<OnboardingState>("get_onboarding_state"),
  completeOnboarding: () => invoke<void>("complete_onboarding"),

  // app
  factoryReset: () => invoke<void>("factory_reset"),
  getStorageStats: () => invoke<StorageStatsDto>("get_storage_stats"),

  // 调试模式 dump 目录管理
  openDebugDumpDir: () => invoke<void>("open_debug_dump_dir"),
  clearDebugDumps: () => invoke<void>("clear_debug_dumps"),

  // updater (运行时按 settings.update_source 切换 manifest 源)
  checkForUpdate: () => invoke<UpdateInfo | null>("check_for_update"),
  downloadInstallUpdate: () => invoke<void>("download_install_update"),
  /** 把检测到的新版本号推给托盘菜单 (null = 已是最新)。只在桌面运行时调用。 */
  setTrayUpdate: (version: string | null) => invoke<void>("set_tray_update", { version }),

  // TLS / HTTPS 证书 (proxy_mode 包含 https 时使用)
  tlsGetStatus: () => invoke<TlsStatus>("tls_get_status"),
  tlsGetCaPemPath: () => invoke<string>("tls_get_ca_pem_path"),
  tlsExportCaZip: (dest: string) => invoke<void>("tls_export_ca_zip", { dest }),
  /** 网页端导出: 返回 CA PEM 文本 */
  tlsGetCaZipBytes: () => invoke<number[]>("tls_get_ca_zip_bytes"),
  tlsRegenerateLeaf: () => invoke<TlsStatus>("tls_regenerate_leaf"),

  // Claude Code 集成 (~/.claude/settings.json)
  readClaudeCodeSettings: () =>
    invoke<ClaudeCodeReadResult>("read_claude_code_settings"),
  inspectClaudeCodeSettings: () =>
    invoke<ClaudeCodeInspectResult>("inspect_claude_code_settings"),
  writeClaudeCodeSettings: (newContent: string) =>
    invoke<ClaudeCodeWriteOutcome>("write_claude_code_settings", { newContent }),
};
