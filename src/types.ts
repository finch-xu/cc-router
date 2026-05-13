// 与 Rust 后端 DTO 对齐的类型定义。
// 约定：Rust 侧 serde 使用 serde(rename_all = "snake_case") 序列化。

export type Compatibility = "verified" | "partial" | "untested";
export type AuthType = "api_key" | "chatgpt_oauth" | "kiro_oauth";
export type AuthHeaderFormat = "raw" | "bearer";

// ===== Kiro OAuth DTO (与 oauth/kiro.rs + subscription/model.rs 对齐) =====

export type KiroAuthMethod = "social" | "idc";

/** 凭据来源预览, 不含 refresh_token. */
export interface KiroImportPreview {
  auth_method: KiroAuthMethod;
  region: string;
  has_profile_arn: boolean;
  has_access_token: boolean;
}

/** 凭据导入完成后的 session 句柄. 前端在创建订阅时回传 session_id. */
export interface KiroImportResult {
  session_id: string;
  preview: KiroImportPreview;
}

export interface KiroDeviceFlowStart {
  device_code: string;
  user_code: string;
  verification_uri: string;
  verification_uri_complete?: string;
  region: string;
  /** 秒 */
  expires_in: number;
}

export interface KiroAccount {
  auth_method: KiroAuthMethod;
  region: string;
  /** Unix ms */
  authenticated_at: number;
}

/** 风控伪装字段, 创建/编辑订阅时由 UI 提供 (None 走后端默认值). */
export interface KiroDisguise {
  /** 64 位十六进制 */
  machine_id: string;
  kiro_version: string;
  system_version: string;
  node_version: string;
}

export interface CreateKiroSubscriptionInput {
  /** 方案 A 凭据来源: cache_imported_session 的 session_id */
  session_id?: string;
  /** 方案 B 凭据来源: device flow 的 device_code */
  device_code?: string;
  provider_id: string;
  endpoint_id: string;
  display_name: string;
  model_slots: ModelSlots;
  disguise?: KiroDisguise;
  profile_arn_override?: string;
}

/**
 * Device Code 启动结果, 对应 Rust 侧 oauth::chatgpt::DeviceFlowStart.
 */
export interface DeviceFlowStart {
  device_code: string;
  user_code: string;
  verification_uri: string;
  /** 秒 */
  expires_in: number;
}

/**
 * ChatGPT 账号公开信息, 不含 refresh_token. 对应 Rust 侧 oauth::chatgpt::ChatGptAccount.
 */
export interface ChatGptAccount {
  account_id: string;
  email?: string;
  /** Unix ms */
  authenticated_at: number;
}

export interface CreateChatGptOAuthSubscriptionInput {
  device_code: string;
  provider_id: string;
  endpoint_id: string;
  display_name: string;
  model_slots: ModelSlots;
}
export type VirtualModelName =
  | "model-opus"
  | "model-sonnet"
  | "model-haiku"
  | "model-fallback";
export type SubscriptionSlot = "opus" | "sonnet" | "haiku";
export type RoutingMode = "sequential" | "round_robin";
export type SubscriptionState =
  | "healthy"
  | "rate_limited"
  | "quota_exhausted"
  | "transient_error"
  | "auth_failed"
  | "disabled";

export interface ProviderEndpointInfo {
  id: string;
  label: string;
  description?: string;
  base_url: string;
  messages_path: string;
  region?: string;
  billing?: string;
}

export interface ProviderInfo {
  id: string;
  display_name: string;
  description?: string;
  homepage?: string;
  docs_url?: string;
  api_key_url?: string;
  icon?: string;
  compatibility: Compatibility;
  compatibility_notes?: string;
  endpoints: ProviderEndpointInfo[];
  default_endpoint?: string;
  auth: {
    type: AuthType;
    header_name: string;
    header_format: AuthHeaderFormat;
  };
  model_discovery: {
    enabled: boolean;
    path: string;
    cache_ttl_hours: number;
    example_models: string[];
  };
}

export interface ModelSlots {
  opus: string;
  sonnet: string;
  haiku: string;
}

export interface ModelInfo {
  id: string;
  display_name?: string;
}

export interface ModelDiscoveryDto {
  enabled: boolean;
  path: string;
  url?: string;
  cache_ttl_hours: number;
  example_models: string[];
}

export interface SubscriptionDto {
  id: string;
  /** 来源标记: 内置 yaml id 或 "__custom__" */
  provider_id: string;
  /** 来源 endpoint id, 自定义订阅写 "__custom__" */
  endpoint_id: string;
  display_name: string;
  model_slots: ModelSlots;
  enabled: boolean;
  state: SubscriptionState;
  cooldown_until?: number;
  last_error_message?: string;
  created_at: number;
  updated_at: number;
  /** 该订阅被哪些虚拟模型引用 */
  referenced_by: VirtualModelName[];
  /** 缓存的模型列表（可能为空，fetched_at = Unix ms） */
  model_cache?: {
    fetched_at: number;
    models: ModelInfo[];
  };

  // snapshot 字段
  base_url: string;
  messages_path: string;
  auth_header_name: string;
  auth_header_format: AuthHeaderFormat;
  required_headers: Record<string, string>;
  forward_headers: string[];
  model_discovery: ModelDiscoveryDto;
  provider_display_name: string;
  provider_icon: string;
  is_user_defined: boolean;

  /** 凭据来源类型. 默认 "api_key" 兼容老 DTO 消费者. */
  auth_type: AuthType;
  /** OAuth 账号信息 (仅 auth_type=chatgpt_oauth 有值, 不含 refresh_token) */
  oauth_account?: {
    account_id: string;
    email?: string;
    /** Unix ms */
    authenticated_at: number;
  };
}

/** 创建订阅时的 source: 内置 yaml 模板 vs 用户自定义 */
export type CreateSource =
  | {
      kind: "from_template";
      provider_id: string;
      endpoint_id: string;
    }
  | {
      kind: "custom";
      provider_display_name: string;
      base_url: string;
      messages_path: string;
      auth_header_name: string;
      auth_header_format: AuthHeaderFormat;
    };

export interface CreateSubscriptionInput {
  display_name: string;
  api_key: string;
  model_slots: ModelSlots;
  source: CreateSource;
}

/** 自定义订阅修改连接信息时的 patch */
export interface ConnectionPatch {
  base_url?: string;
  messages_path?: string;
  auth_header_name?: string;
  auth_header_format?: AuthHeaderFormat;
  provider_display_name?: string;
}

export interface SubscriptionPatch {
  display_name?: string;
  model_slots?: ModelSlots;
  /** 内置订阅: 切换 endpoint, 后端 re-snapshot */
  endpoint_id?: string;
  /** 自定义订阅: 改连接信息 */
  connection?: ConnectionPatch;
}

export interface TestConnectionResult {
  ok: boolean;
  message: string;
  /** 上游 HTTP 状态码;网络错误时为 undefined */
  http_status?: number;
  /** 实际用于测试的 model 名 */
  model_used?: string;
  /** 测试通过且触发了状态机复活 */
  state_reset: boolean;
}

export type RefreshModelListResult =
  | { kind: "auto"; models: ModelInfo[]; fetched_at: number }
  | { kind: "manual_fallback"; reason: string };

export interface VirtualModelDto {
  name: VirtualModelName;
  mode: RoutingMode;
  subscription_ids: string[];
}

export interface UpdateVirtualModelInput {
  mode: RoutingMode;
  subscription_ids: string[];
}

/** 代理监听协议组合 (Settings.proxy_mode). 默认 "http". */
export type ProxyMode = "http" | "https" | "both";

export interface Settings {
  proxy_port: number;
  /** 代理监听协议组合, 默认 "http"; 切换需重启 app */
  proxy_mode: ProxyMode;
  /** HTTPS 端口, 默认 23457; 仅当 proxy_mode 包含 https 时使用 */
  https_port: number;
  /** 用户配置的额外 SAN (IP/hostname). 内置 localhost/127.0.0.1/::1 永远在 leaf 证书里, 此项追加. */
  tls_extra_sans: string[];
  /** true: 监听 0.0.0.0（局域网可访问）；false: 仅 127.0.0.1 */
  listen_all: boolean;
  autostart: boolean;
  log_retention_days: number;
  db_size_limit_mb: number;
  /** true: 校验 token;false: 任何请求放行 */
  auth_enabled: boolean;
  /** 当前 token 明文,通过 generate_new_token 命令重新生成 */
  auth_token: string;
  /** true: 响应附加 CORS 头;false: 浏览器跨域调用会被拦截 */
  cors_enabled: boolean;
  /** Access-Control-Allow-Origin 值,默认 "*" */
  cors_allow_origin: string;
  /** 前端 UI 语言偏好: "system" 跟随系统 / "zh" / "en" / "ja"。默认 system */
  preferred_language: "system" | "zh" | "en" | "ja";
  /**
   * 更新源选择: null=未设置(走 tauri.conf.json 默认 GitHub),
   * "international"=国际(GitHub) / "china"=中国大陆(阿里云 OSS)。
   * 首次启动后前端按 navigator.language 自动写入。
   */
  update_source: UpdateSource | null;
  /**
   * 调试模式: 开启后每次出站 attempt 把客户端请求体 / cc-router 出站请求体 /
   * 上游响应体三段写入 app_data_dir/debug-dumps/ 下 .txt 文件,排查协议适配类问题.
   * 默认关闭。
   */
  debug_mode: boolean;
}

export type UpdateSource = "international" | "china";

export interface SettingsPatch {
  proxy_port?: number;
  proxy_mode?: ProxyMode;
  https_port?: number;
  tls_extra_sans?: string[];
  listen_all?: boolean;
  autostart?: boolean;
  log_retention_days?: number;
  db_size_limit_mb?: number;
  auth_enabled?: boolean;
  cors_enabled?: boolean;
  cors_allow_origin?: string;
  preferred_language?: "system" | "zh" | "en" | "ja";
  update_source?: UpdateSource;
  debug_mode?: boolean;
  // 注意: auth_token 不在 patch 里,必须通过 generateNewToken() 改
}

/** Rust 侧 commands::updater::UpdateInfo */
export interface UpdateInfo {
  version: string;
  current_version: string;
  body?: string;
}

/** updater://progress 事件 payload */
export type UpdaterProgressEvent =
  | { phase: "started"; content_length: number | null }
  | { phase: "progress"; chunk_length: number }
  | { phase: "finished" };

export interface ProxyStatus {
  /** 兼容字段: HTTP 端口 (HTTPS-only 模式下回退到 HTTPS 端口) */
  port: number;
  running: boolean;
  mode: ProxyMode;
  /** HTTP listener 实际端口, 未启用为 null */
  http_port: number | null;
  /** HTTPS listener 实际端口, 未启用为 null */
  https_port: number | null;
  /** 监听地址 (0.0.0.0 vs 127.0.0.1) */
  listen_all: boolean;
}

/** TLS 状态 (cc-router 自签 CA 信息). 对应 Rust 侧 tls::TlsStatus */
export interface TlsStatus {
  ca_exists: boolean;
  /** SHA-256 hex 全小写, 64 字符. ca_exists=false 时为 null */
  ca_fingerprint_sha256: string | null;
  /** CA 公钥 PEM 文件绝对路径. ca_exists=false 时为 null */
  ca_pem_path: string | null;
}

export interface OnboardingState {
  completed: boolean;
}

export type RequestStatus = "success" | "error" | "timeout";

export interface RequestLogDto {
  id: string;
  timestamp: number;
  virtual_model_name: VirtualModelName;
  subscription_id: string;
  provider_id: string;
  endpoint_id: string;
  real_model_name: string;
  /** 上游响应里的 message.model 原值;错误/超时为 undefined */
  response_model_name?: string;
  is_streaming: boolean;
  status: RequestStatus;
  http_status?: number;
  total_latency_ms?: number;
  input_tokens?: number;
  output_tokens?: number;
  cache_creation_tokens?: number;
  cache_read_tokens?: number;
  error_message?: string;
  /** 上游错误响应 body 截断(最多 4KB), 仅错误路径有值 */
  upstream_response_body?: string;
}

export interface ListRequestsResult {
  items: RequestLogDto[];
  total: number;
}

export interface RequestLogFilters {
  virtual_model_name?: VirtualModelName;
  provider_id?: string;
  status?: RequestStatus;
  subscription_id?: string;
}

// ===== 统计聚合 (commands/statistics.rs) =====

export type StatsRange =
  | "today"
  | "last7_days"
  | "last30_days"
  | "last90_days"
  | "all_time";

export type BreakdownBy = "virtual_model" | "subscription";

export interface OverallStatsDto {
  total_requests: number;
  success_count: number;
  error_count: number;
  timeout_count: number;
  /** 0–100 */
  success_rate_pct: number;
  avg_duration_ms?: number;
  p95_duration_ms?: number;
  total_input_tokens: number;
  total_output_tokens: number;
  total_cache_creation_tokens: number;
  total_cache_read_tokens: number;
}

export interface DailySeriesPointDto {
  /** UTC 0 点 ms */
  date_utc: number;
  request_count: number;
  success_count: number;
  error_count: number;
  timeout_count: number;
  total_input_tokens: number;
  total_output_tokens: number;
  avg_duration_ms?: number;
}

export interface BreakdownDto {
  /** virtual_model_name 或 subscription_id (UUID 字符串) */
  key: string;
  /** 显示名 (虚拟模型字面量 / 订阅 alias / "(已删除订阅)") */
  label: string;
  request_count: number;
  success_count: number;
  error_count: number;
  timeout_count: number;
  total_input_tokens: number;
  total_output_tokens: number;
  avg_duration_ms?: number;
}

export interface HeatmapDayDto {
  /** UTC 0 点 ms */
  date_utc: number;
  /** input + output tokens */
  total_tokens: number;
  request_count: number;
}

// ===== Receipts (commands/receipts.rs) =====
// 与 StatsRange 故意分开: Receipts 直接查 requests 原始表, 支持 24h 滚动窗口

export type ReceiptRange =
  | "last_24_hours"
  | "last7_days"
  | "last30_days"
  | "last_year"
  | "all_time";

export interface ReceiptTotalsDto {
  request_count: number;
  input_tokens: number;
  output_tokens: number;
  cache_creation_tokens: number;
  cache_read_tokens: number;
}

export interface ReceiptSubItemDto {
  subscription_id: string;
  /** undefined 表示订阅已被删除; 前端按 i18n 显示兜底文案 */
  subscription_display_name?: string;
  provider_id: string;
  provider_display_name: string;
  real_model_name: string;
  totals: ReceiptTotalsDto;
}

export interface ReceiptVirtualModelItemDto {
  /** "model-opus" | "model-sonnet" | "model-haiku" — fallback 不出现 */
  virtual_model_name: string;
  subtotal: ReceiptTotalsDto;
  sub_items: ReceiptSubItemDto[];
}

export interface ReceiptDto {
  range: ReceiptRange;
  range_start_ms: number;
  range_end_ms: number;
  generated_at_ms: number;
  /** 8 位大写 hex 单号 */
  slip_no: string;
  /** 始终 3 项: opus / sonnet / haiku, 顺序固定 */
  items: ReceiptVirtualModelItemDto[];
  grand_total: ReceiptTotalsDto;
}

export type EventKind = "request" | "subscription_state_change" | "system_error";
export type EventSeverity = "info" | "warn" | "error";

export interface EventDto {
  id: string;
  timestamp: number;
  kind: EventKind;
  severity: EventSeverity;
  subscription_id?: string;
  request_id?: string;
  summary: string;
  /** 解析后的结构化对象, 后端已 JSON.parse */
  payload?: unknown;
}

export interface ListEventsResult {
  items: EventDto[];
  total: number;
}

export interface EventFilters {
  kind?: EventKind;
  subscription_id?: string;
  severity?: EventSeverity;
}

/** subscription_state_change 事件的 payload 反序列化形态 */
export interface StateChangePayload {
  from: SubscriptionState;
  to: SubscriptionState;
  reason: string;
  last_error?: string | null;
}

// 路由实时事件(对应 proxy/pipeline.rs 里 emit 的 payload)
export interface RouteAttemptStartedEvent {
  subscription_id: string;
  virtual_model: VirtualModelName;
}

export interface RouteAttemptFinishedEvent {
  subscription_id: string;
  virtual_model: VirtualModelName;
  success: boolean;
}

export type RouteFlashKind = "attempt" | "success" | "error";
