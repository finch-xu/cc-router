//! ChatGPT Plus/Pro OAuth 管理 (复刻自 cc-switch `codex_oauth_auth.rs`, 补充标准浏览器 PKCE 登录).
//!
//! ## 设计要点
//! - **双登录方式**:
//!   - 首选: 浏览器 OAuth 2.0 PKCE + 本地回环回调 (`http://localhost:1455/auth/callback`),
//!     与官方 Codex CLI 桌面端一致, 不依赖账号安全设置里的「设备码授权」开关。
//!   - 备选: Device Code (`auth.openai.com/codex/device`), 适用于 1455 端口被占用或无本地浏览器环境。
//!   两种方式完成后都将 `PolledTokens` 写入同一个 `completed` 缓存, 统一由 `create_chatgpt_oauth_subscription` 消费。
//! - **每个订阅一份 refresh_token**: 持久化在 `subscriptions.oauth_metadata` 列;
//!   access_token 仅放进程内存, 重启后下一次请求时再 refresh 拿一次。
//! - **60s 提前刷新**: id_token JWT 的 `exp` 减 60s 视为「该刷新了」。
//! - **per-subscription 互斥锁**: 高并发请求同一订阅时, 只允许一条线 refresh,
//!   其他人在锁外 double-check 缓存。
//! - **refresh_token rotation**: OpenAI 偶尔会下发新的 refresh_token,
//!   收到后合并保留原有 account_id/email 写回 SQLite (`store::update_oauth_metadata`),
//!   并同步到内存 `rotated_refresh_tokens` 避免内存态 runtime 沿用失效旧 token。
//!
//! ## 与 ~/.codex/auth.json 的关系
//! cc-router 不读 / 不写 ~/.codex/auth.json. 用户在 cc-router UI 内独立完成 OAuth 登录,
//! 凭据存在 cc-router 自己的 SQLite 里。这是「方案 A: 完全隔离」, 见 plan.md。

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::extract::Query;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::Router;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use chrono::Utc;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, Mutex, RwLock};
use tracing::{info, warn};
use uuid::Uuid;

use crate::subscription::model::OAuthMetadata;
use crate::subscription::store;

/// OpenAI 官方 Codex CLI 用的 OAuth client_id.
/// 与 cc-switch / codex-rs 实测一致, 可直接打 ChatGPT 后端 (chatgpt.com/backend-api/codex).
const CHATGPT_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

const OAUTH_AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const DEVICE_CODE_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
const DEVICE_POLL_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
const OAUTH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const DEVICE_VERIFICATION_URI: &str = "https://auth.openai.com/codex/device";
const DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
/// 与官方 Codex CLI 保持一致的本地 OAuth PKCE 回调端口与路径 (OpenAI 仅白名单该 redirect_uri).
pub const PKCE_CALLBACK_PORT: u16 = 1455;
const PKCE_CALLBACK_PATH: &str = "/auth/callback";
const PKCE_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const PKCE_SCOPE: &str = "openid profile email offline_access";
const PKCE_FLOW_TTL_SECS: u64 = 600;

/// 仿官方 codex_cli_rs 的 originator HTTP header 值.
/// OpenAI 风控会校验这个 header, 缺失或值不匹配时更可能触发手机号验证 / 二次确认.
/// 我们用真实官方值, 但通过 User-Agent 后缀显式标识 cc-router 身份 (build_codex_ua).
pub(crate) const CODEX_ORIGINATOR: &str = "codex_cli_rs";
/// Header name carrying [`CODEX_ORIGINATOR`].
pub(crate) const CODEX_ORIGINATOR_HEADER: &str = "originator";

/// 仿官方 codex_cli_rs UA 但**显式标识 cc-router**, 既降低 OpenAI 风控误判
/// 又对外明示我们不是真 codex CLI。形如 `codex_cli_rs/1.7.0 (macos; aarch64) cc-router/1.7.0`.
///
/// 不引入 `sys-info` / `os_info` 等新 crate, std::env::consts 给的 OS / ARCH 已经够 OpenAI
/// 识别成「类 codex CLI 客户端」. 如果以后发现 OpenAI 真的对 kernel version 字段有校验, 再单独补依赖.
pub(crate) fn build_codex_ua() -> &'static str {
    static UA: OnceLock<String> = OnceLock::new();
    UA.get_or_init(|| {
        let os = std::env::consts::OS; // "macos" / "windows" / "linux"
        let arch = std::env::consts::ARCH; // "x86_64" / "aarch64"
        let ver = env!("CARGO_PKG_VERSION");
        format!("codex_cli_rs/{ver} ({os}; {arch}) cc-router/{ver}")
    })
    .as_str()
}

/// 用于构造请求 header 时避免每次都 `HeaderValue::from_str` 重新校验.
pub(crate) const CHATGPT_ACCOUNT_ID_HEADER: &str = "ChatGPT-Account-Id";

/// access_token 提前 60s 视为过期.
const TOKEN_REFRESH_BUFFER_MS: i64 = 60_000;
/// access_token 解 JWT 失败时, 假定它的剩余寿命 (默认 4 分钟, 强制下一次请求 refresh).
const FALLBACK_ACCESS_TOKEN_TTL_MS: i64 = 4 * 60 * 1000;
/// Device Code 默认有效期 (秒), OpenAI 文档约定 15min.
const DEVICE_CODE_DEFAULT_EXPIRES_IN: u64 = 900;

#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    #[error("等待用户在浏览器完成授权")]
    AuthorizationPending,
    #[error("用户拒绝了授权")]
    AccessDenied,
    #[error("登录链接已过期, 请重新发起")]
    DeviceCodeExpired,
    #[error("OAuth 服务返回错误: {0}")]
    UpstreamError(String),
    #[error("Refresh Token 已失效, 需要重新登录")]
    RefreshTokenInvalid,
    #[error("网络错误: {0}")]
    Network(String),
    #[error("解析错误: {0}")]
    Parse(String),
    #[error("找不到对应的 device_code, 请重新发起登录流程")]
    PendingDeviceNotFound,
    #[error("订阅 {0} 没有 OAuth 凭据")]
    NoCredentials(Uuid),
    #[error("数据库错误: {0}")]
    Storage(String),
    #[error("本地回调端口 {0} 已被占用")]
    CallbackPortInUse(u16),
}

impl From<reqwest::Error> for OAuthError {
    fn from(err: reqwest::Error) -> Self {
        OAuthError::Network(err.to_string())
    }
}
impl From<serde_json::Error> for OAuthError {
    fn from(err: serde_json::Error) -> Self {
        OAuthError::Parse(err.to_string())
    }
}
impl From<sqlx::Error> for OAuthError {
    fn from(err: sqlx::Error) -> Self {
        OAuthError::Storage(err.to_string())
    }
}
impl From<crate::error::AppError> for OAuthError {
    fn from(err: crate::error::AppError) -> Self {
        OAuthError::Storage(err.to_string())
    }
}

// =====================================================================
// HTTP 协议结构 (OpenAI 私有协议, 字段对齐 cc-switch / codex-rs)
// =====================================================================

#[derive(Debug, Deserialize)]
struct DeviceCodeResp {
    device_auth_id: String,
    user_code: String,
    #[serde(default)]
    #[allow(dead_code)] // 协议字段, 当前不用. 留着便于以后实现自适应轮询间隔.
    interval: Option<serde_json::Value>,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct DevicePollSuccess {
    authorization_code: String,
    code_verifier: String,
}

#[derive(Debug, Deserialize)]
struct OAuthTokenResp {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

/// 暴露给前端的 device flow 启动结果.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceFlowStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    /// 秒
    pub expires_in: u64,
}

/// 暴露给前端的浏览器 PKCE 登录启动结果.
#[derive(Debug, Clone, Serialize)]
pub struct BrowserFlowStart {
    /// 与 OAuth state 一致; 授权成功后也作为 completed 缓存键传给 create_chatgpt_oauth_subscription.
    pub flow_id: String,
    pub authorize_url: String,
    /// 秒
    pub expires_in: u64,
}

/// 完成 device flow 后的账户信息, 不含 refresh_token.
#[derive(Debug, Clone, Serialize)]
pub struct ChatGptAccount {
    pub account_id: String,
    pub email: Option<String>,
    pub authenticated_at: i64,
}

// =====================================================================
// JWT 解析
// =====================================================================

#[derive(Debug, Default, Deserialize)]
struct IdTokenClaims {
    #[serde(default)]
    exp: Option<i64>,
    #[serde(default)]
    chatgpt_account_id: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default, rename = "https://api.openai.com/auth")]
    openai_auth: Option<OpenAiAuthClaim>,
}

#[derive(Debug, Default, Deserialize)]
struct OpenAiAuthClaim {
    #[serde(default)]
    chatgpt_account_id: Option<String>,
}

/// 解析 JWT payload (中间段). 不验签, 因为我们假定 OpenAI 域名下的响应是可信的.
fn decode_jwt_payload(token: &str) -> Result<IdTokenClaims, OAuthError> {
    let mut parts = token.split('.');
    let _header = parts.next();
    let payload = parts.next().ok_or_else(|| OAuthError::Parse("JWT 缺少 payload".into()))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|e| OAuthError::Parse(format!("JWT base64 解码失败: {e}")))?;
    let claims: IdTokenClaims = serde_json::from_slice(&bytes)?;
    Ok(claims)
}

fn extract_account_and_email(token: &OAuthTokenResp) -> (Option<String>, Option<String>, Option<i64>) {
    let Some(id_token) = token.id_token.as_deref() else {
        return (None, None, None);
    };
    let claims = match decode_jwt_payload(id_token) {
        Ok(c) => c,
        Err(e) => {
            warn!(?e, "解 id_token JWT 失败, 沿用 expires_in 推算");
            return (None, None, None);
        }
    };
    let account_id = claims
        .chatgpt_account_id
        .clone()
        .or_else(|| claims.openai_auth.as_ref().and_then(|a| a.chatgpt_account_id.clone()));
    (account_id, claims.email, claims.exp)
}

// =====================================================================
// Manager
// =====================================================================

/// 内存里的 access_token 缓存 (单订阅).
#[derive(Debug, Clone)]
struct CachedToken {
    access_token: String,
    /// Unix ms
    expires_at_ms: i64,
}

impl CachedToken {
    fn is_expiring_soon(&self) -> bool {
        Utc::now().timestamp_millis() + TOKEN_REFRESH_BUFFER_MS >= self.expires_at_ms
    }
}

/// 进行中的 Device Code 登录.
#[derive(Debug, Clone)]
struct PendingDevice {
    user_code: String,
    /// Unix ms
    expires_at_ms: i64,
}

/// 进行中 / 已完成的浏览器 OAuth PKCE 登录 (同时只能有一个, 因回调端口固定为 1455).
struct ActiveBrowserFlow {
    flow_id: String,
    code_verifier: String,
    /// Unix ms
    expires_at_ms: i64,
    status: BrowserFlowStatus,
    /// 触发后优雅停掉 1455 本地回调 listener.
    shutdown: Option<oneshot::Sender<()>>,
}

enum BrowserFlowStatus {
    Pending,
    Completed(ChatGptAccount),
    Failed(String),
}

/// 用户完成授权后, 后端把整套 token 缓存 10min 等前端调 create_subscription.
/// 这样 refresh_token 不会走前端.
const COMPLETED_POLL_TTL_MS: i64 = 10 * 60 * 1000;

/// ChatGPT OAuth 管理器单例. 放进 AppState 由全局共享.
pub struct ChatGptOAuthManager {
    pool: SqlitePool,
    http: Client,
    /// subscription_id -> 缓存的 access_token
    cache: Arc<RwLock<HashMap<Uuid, CachedToken>>>,
    /// subscription_id -> 最近一次 rotation 拿到的最新 refresh_token.
    /// 避免调用方 (pipeline / ping / models / usage) 从内存 SubscriptionRuntime 读到旧 refresh_token.
    rotated_refresh_tokens: Arc<RwLock<HashMap<Uuid, String>>>,
    /// subscription_id -> 互斥锁, 同一订阅同一时刻只允许一个 refresh
    locks: Arc<RwLock<HashMap<Uuid, Arc<Mutex<()>>>>>,
    /// device_auth_id -> 进行中的 device flow
    pending: Arc<RwLock<HashMap<String, PendingDevice>>>,
    /// 当前活跃的浏览器 PKCE 登录流程 (监听 127.0.0.1:1455).
    browser_flow: Arc<RwLock<Option<ActiveBrowserFlow>>>,
    /// device_auth_id 或 pkce flow_id -> 已经完成授权的 token 集合, 等前端调 consume_completed_poll 落盘.
    /// 这样 refresh_token 永远不暴露给前端 JS context.
    completed: Arc<RwLock<HashMap<String, (PolledTokens, i64)>>>,
}

impl ChatGptOAuthManager {
    pub fn new(pool: SqlitePool) -> Self {
        // 默认 headers: 给所有请求都带 originator: codex_cli_rs.
        // OpenAI 风控用这个 header 区分「真 Codex CLI」与未知客户端, 缺失更易触发手机验证.
        let mut default_headers = reqwest::header::HeaderMap::new();
        default_headers.insert(
            reqwest::header::HeaderName::from_static("originator"),
            reqwest::header::HeaderValue::from_static(CODEX_ORIGINATOR),
        );
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(build_codex_ua())
            .default_headers(default_headers)
            .build()
            .expect("无法构建 OAuth http client");
        Self {
            pool,
            http,
            cache: Arc::new(RwLock::new(HashMap::new())),
            rotated_refresh_tokens: Arc::new(RwLock::new(HashMap::new())),
            locks: Arc::new(RwLock::new(HashMap::new())),
            pending: Arc::new(RwLock::new(HashMap::new())),
            browser_flow: Arc::new(RwLock::new(None)),
            completed: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 启动浏览器 OAuth 2.0 PKCE 登录流程 (首选):
    /// 1. 释放上一轮未结束的 1455 监听
    /// 2. 绑定 127.0.0.1:1455 接收 `/auth/callback`
    /// 3. 返回授权页 URL, 前端打开浏览器后轮询 `poll_browser_flow`
    pub async fn start_browser_flow(&self) -> Result<BrowserFlowStart, OAuthError> {
        let had_previous = self.stop_active_browser_listener().await;

        let listener = bind_pkce_callback_listener(PKCE_CALLBACK_PORT, had_previous).await?;

        let code_verifier = random_base64url(32)?;
        let code_challenge = pkce_s256_challenge(&code_verifier);
        let flow_id = random_base64url(24)?;
        let authorize_url = build_pkce_authorize_url(&code_challenge, &flow_id)?;
        let expires_at_ms = Utc::now().timestamp_millis() + (PKCE_FLOW_TTL_SECS as i64) * 1000;

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        {
            let mut slot = self.browser_flow.write().await;
            *slot = Some(ActiveBrowserFlow {
                flow_id: flow_id.clone(),
                code_verifier,
                expires_at_ms,
                status: BrowserFlowStatus::Pending,
                shutdown: Some(shutdown_tx),
            });
        }

        let http = self.http.clone();
        let browser_flow = self.browser_flow.clone();
        let completed = self.completed.clone();
        let expected_flow_id = flow_id.clone();

        tokio::spawn(async move {
            let app = Router::new().route(
                PKCE_CALLBACK_PATH,
                get(move |Query(params): Query<HashMap<String, String>>| {
                    let http = http.clone();
                    let browser_flow = browser_flow.clone();
                    let completed = completed.clone();
                    let expected_flow_id = expected_flow_id.clone();
                    async move {
                        handle_pkce_callback(
                            http,
                            browser_flow,
                            completed,
                            expected_flow_id,
                            params,
                        )
                        .await
                    }
                }),
            );

            let serve = axum::serve(listener, app).with_graceful_shutdown(async move {
                tokio::select! {
                    _ = shutdown_rx => {}
                    _ = tokio::time::sleep(Duration::from_secs(PKCE_FLOW_TTL_SECS)) => {}
                }
            });
            if let Err(e) = serve.await {
                warn!(?e, "ChatGPT PKCE 本地回调服务退出异常");
            }
        });

        Ok(BrowserFlowStart {
            flow_id,
            authorize_url,
            expires_in: PKCE_FLOW_TTL_SECS,
        })
    }

    /// 轮询浏览器 PKCE 登录结果.
    pub async fn poll_browser_flow(
        &self,
        flow_id: &str,
    ) -> Result<Option<ChatGptAccount>, OAuthError> {
        let mut slot = self.browser_flow.write().await;
        let Some(active) = slot.as_mut() else {
            return Err(OAuthError::PendingDeviceNotFound);
        };
        if active.flow_id != flow_id {
            return Err(OAuthError::PendingDeviceNotFound);
        }
        match &active.status {
            BrowserFlowStatus::Completed(account) => Ok(Some(account.clone())),
            BrowserFlowStatus::Failed(msg) => Err(OAuthError::UpstreamError(msg.clone())),
            BrowserFlowStatus::Pending => {
                if active.expires_at_ms <= Utc::now().timestamp_millis() {
                    if let Some(tx) = active.shutdown.take() {
                        let _ = tx.send(());
                    }
                    *slot = None;
                    Err(OAuthError::DeviceCodeExpired)
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// 取消进行中的浏览器 PKCE 登录并立即释放 1455 端口.
    pub async fn cancel_browser_flow(&self, flow_id: Option<&str>) {
        let mut slot = self.browser_flow.write().await;
        let should_clear = match (slot.as_ref(), flow_id) {
            (Some(_), None) => true,
            (Some(active), Some(id)) => active.flow_id == id,
            (None, _) => false,
        };
        if should_clear {
            if let Some(mut active) = slot.take() {
                if let Some(tx) = active.shutdown.take() {
                    let _ = tx.send(());
                }
            }
        }
    }

    async fn stop_active_browser_listener(&self) -> bool {
        let mut slot = self.browser_flow.write().await;
        if let Some(mut prev) = slot.take() {
            if let Some(tx) = prev.shutdown.take() {
                let _ = tx.send(());
                return true;
            }
        }
        false
    }

    /// 启动 Device Code 流程. 前端拿到 user_code + verification_uri 后,
    /// 自己负责 shell::open 打开浏览器.
    pub async fn start_device_flow(&self) -> Result<DeviceFlowStart, OAuthError> {
        let resp = self
            .http
            .post(DEVICE_CODE_URL)
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({ "client_id": CHATGPT_CLIENT_ID }))
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(OAuthError::UpstreamError(format!("device code 请求失败: {status} {text}")));
        }
        let dr: DeviceCodeResp = resp.json().await?;
        let expires_in = dr.expires_in.unwrap_or(DEVICE_CODE_DEFAULT_EXPIRES_IN);
        let now_ms = Utc::now().timestamp_millis();
        let entry = PendingDevice {
            user_code: dr.user_code.clone(),
            expires_at_ms: now_ms + expires_in as i64 * 1000,
        };
        // 顺手清掉过期条目, 避免内存膨胀.
        let mut pending = self.pending.write().await;
        pending.retain(|_, v| v.expires_at_ms > now_ms);
        pending.insert(dr.device_auth_id.clone(), entry);

        Ok(DeviceFlowStart {
            device_code: dr.device_auth_id,
            user_code: dr.user_code,
            verification_uri: DEVICE_VERIFICATION_URI.to_string(),
            expires_in,
        })
    }

    /// 轮询 device code 状态. 返回 Ok(None) 表示用户还没完成授权 (前端继续轮询);
    /// 返回 Ok(Some(_)) 表示授权成功, 拿到了账号信息.
    /// 完整 token 集 (含 refresh_token) 在后端缓存里, 前端拿 device_code 调 consume_completed_poll 取走.
    pub async fn poll_device_code(
        &self,
        device_code: &str,
    ) -> Result<Option<ChatGptAccount>, OAuthError> {
        // 取出 pending 条目
        let pending = {
            let read = self.pending.read().await;
            read.get(device_code).cloned()
        };
        let pending = pending.ok_or(OAuthError::PendingDeviceNotFound)?;

        if pending.expires_at_ms <= Utc::now().timestamp_millis() {
            self.pending.write().await.remove(device_code);
            return Err(OAuthError::DeviceCodeExpired);
        }

        let resp = self
            .http
            .post(DEVICE_POLL_URL)
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "device_auth_id": device_code,
                "user_code": pending.user_code,
            }))
            .send()
            .await?;
        let status = resp.status();
        // 403 / 404 表示「还没授权」(OpenAI 用 404 而非 OAuth 标准的 authorization_pending)
        if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if status == reqwest::StatusCode::GONE {
            return Err(OAuthError::DeviceCodeExpired);
        }
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(OAuthError::UpstreamError(format!("device poll 失败: {status} {text}")));
        }

        let success: DevicePollSuccess = resp.json().await?;
        // 用 authorization_code + code_verifier 换 token
        let token = self.exchange_code_for_token(&success.authorization_code, &success.code_verifier).await?;

        // 清掉 pending
        self.pending.write().await.remove(device_code);

        let (account_id, email, exp) = extract_account_and_email(&token);
        let account_id = account_id.ok_or_else(|| OAuthError::Parse(
            "id_token 中找不到 chatgpt_account_id, 当前账号可能不是 ChatGPT Plus/Pro".into()
        ))?;
        let refresh_token = token.refresh_token.clone().ok_or_else(|| OAuthError::Parse(
            "OAuth 响应缺少 refresh_token".into()
        ))?;

        let now_ms = Utc::now().timestamp_millis();
        let expires_at_ms = compute_expires_at_ms(exp, token.expires_in);

        let polled = PolledTokens {
            access_token: token.access_token,
            refresh_token,
            account_id: account_id.clone(),
            email: email.clone(),
            authenticated_at_ms: now_ms,
            access_token_expires_at_ms: expires_at_ms,
        };
        let public = ChatGptAccount {
            account_id,
            email,
            authenticated_at: now_ms,
        };

        // 顺手清掉过期的 completed 条目, 然后塞入新的.
        let mut completed = self.completed.write().await;
        completed.retain(|_, (_, expires)| *expires > now_ms);
        completed.insert(device_code.to_string(), (polled, now_ms + COMPLETED_POLL_TTL_MS));

        Ok(Some(public))
    }

    /// 把 poll 后台缓存的完整 token 集合取出来 (一次性消费, 取完即删).
    /// 用于在前端拿 device_code 调 create_chatgpt_oauth_subscription 时, 后端落盘 refresh_token.
    pub async fn consume_completed_poll(
        &self,
        device_code: &str,
    ) -> Result<PolledTokens, OAuthError> {
        let mut completed = self.completed.write().await;
        let now_ms = Utc::now().timestamp_millis();
        // 顺手清过期
        completed.retain(|_, (_, expires)| *expires > now_ms);
        let (tokens, _) = completed
            .remove(device_code)
            .ok_or(OAuthError::PendingDeviceNotFound)?;
        Ok(tokens)
    }

    /// 把刚刚轮询拿到的 access_token 注入指定订阅的内存缓存,
    /// 后续 pipeline 会从这里读 (无需立即 refresh).
    pub async fn seed_cache(&self, sub_id: Uuid, access_token: String, expires_at_ms: i64) {
        let mut cache = self.cache.write().await;
        cache.insert(sub_id, CachedToken { access_token, expires_at_ms });
    }

    /// 主入口 (pipeline 每次请求前调). 优先吃缓存; 若过期或不存在, 用 refresh_token 刷.
    /// 失败原因若是 RefreshTokenInvalid, 调用方可把订阅置为 auth_failed.
    pub async fn get_valid_access_token(
        &self,
        sub_id: Uuid,
        refresh_token: &str,
    ) -> Result<String, OAuthError> {
        // 1. 先吃读锁查缓存
        {
            let cache = self.cache.read().await;
            if let Some(t) = cache.get(&sub_id) {
                if !t.is_expiring_soon() {
                    return Ok(t.access_token.clone());
                }
            }
        }

        // 2. 拿 per-subscription 锁
        let lock = {
            let mut locks = self.locks.write().await;
            locks.entry(sub_id).or_insert_with(|| Arc::new(Mutex::new(()))).clone()
        };
        let _guard = lock.lock().await;

        // 3. double-check
        {
            let cache = self.cache.read().await;
            if let Some(t) = cache.get(&sub_id) {
                if !t.is_expiring_soon() {
                    return Ok(t.access_token.clone());
                }
            }
        }

        // 4. refresh (若此前本进程已发生过 rotation, 优先用内存记录的最新 refresh_token)
        let effective_refresh_token = {
            let rotated = self.rotated_refresh_tokens.read().await;
            rotated
                .get(&sub_id)
                .cloned()
                .unwrap_or_else(|| refresh_token.to_string())
        };
        info!(%sub_id, "refreshing ChatGPT access_token");
        let new_tokens = self.refresh_with_token(&effective_refresh_token).await?;
        let (account_id, email, exp) = extract_account_and_email(&new_tokens);
        let expires_at_ms = compute_expires_at_ms(exp, new_tokens.expires_in);

        // 4a. refresh_token rotation: 合并保留原有 account_id / email 后落盘,
        //     防止 refresh 响应未返回 id_token 时把 account_id 覆写成空串.
        if let Some(new_rt) = new_tokens.refresh_token.as_deref() {
            if !new_rt.is_empty() && new_rt != effective_refresh_token {
                info!(%sub_id, "refresh_token rotated, persisting");
                let existing = match store::get_oauth_metadata(&self.pool, &sub_id).await {
                    Ok(meta) => meta,
                    Err(e) => {
                        warn!(%sub_id, error=?e, "读取现有 oauth_metadata 失败");
                        None
                    }
                };
                let metadata = merge_rotated_metadata(
                    existing.as_ref(),
                    new_rt,
                    account_id,
                    email,
                    Utc::now().timestamp_millis(),
                );
                if let Err(e) = store::update_oauth_metadata(&self.pool, &sub_id, &metadata).await {
                    warn!(%sub_id, error=?e, "落盘新 refresh_token 失败");
                }
                self.rotated_refresh_tokens
                    .write()
                    .await
                    .insert(sub_id, new_rt.to_string());
            }
        }

        // 4b. 写缓存
        let mut cache = self.cache.write().await;
        cache.insert(
            sub_id,
            CachedToken {
                access_token: new_tokens.access_token.clone(),
                expires_at_ms,
            },
        );

        Ok(new_tokens.access_token)
    }

    /// 主动撤销内存缓存 (前端点 disconnect 时调).
    pub async fn forget(&self, sub_id: Uuid) {
        self.cache.write().await.remove(&sub_id);
        self.rotated_refresh_tokens.write().await.remove(&sub_id);
        self.locks.write().await.remove(&sub_id);
    }

    // ------------------- 内部 ---------------------

    async fn exchange_code_for_token(
        &self,
        code: &str,
        code_verifier: &str,
    ) -> Result<OAuthTokenResp, OAuthError> {
        exchange_code_for_token_with_redirect(
            &self.http,
            code,
            code_verifier,
            DEVICE_REDIRECT_URI,
        )
        .await
    }

    async fn refresh_with_token(
        &self,
        refresh_token: &str,
    ) -> Result<OAuthTokenResp, OAuthError> {
        let resp = self
            .http
            .post(OAUTH_TOKEN_URL)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", CHATGPT_CLIENT_ID),
                ("scope", "openid profile email"),
            ])
            .send()
            .await?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(OAuthError::RefreshTokenInvalid);
        }
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(OAuthError::UpstreamError(format!("refresh 失败: {status} {text}")));
        }
        Ok(resp.json().await?)
    }
}

/// poll_device_code 成功时返回的全套字段, 由调用方决定怎么落盘.
/// (一般是: 创建一条 chatgpt_oauth 订阅, 写 oauth_metadata + seed_cache.)
#[derive(Debug, Clone)]
pub struct PolledTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub account_id: String,
    pub email: Option<String>,
    pub authenticated_at_ms: i64,
    pub access_token_expires_at_ms: i64,
}

/// id_token 的 exp 优先, 其次 expires_in, 都没有就给个 fallback.
fn compute_expires_at_ms(exp_seconds: Option<i64>, expires_in_seconds: Option<i64>) -> i64 {
    if let Some(exp) = exp_seconds {
        return exp * 1000;
    }
    let now = Utc::now().timestamp_millis();
    if let Some(secs) = expires_in_seconds {
        return now + secs * 1000;
    }
    now + FALLBACK_ACCESS_TOKEN_TTL_MS
}

/// 合并 refresh_token 轮换后的 OAuthMetadata: 新响应缺少 account_id / email 时保留既有值,
/// 避免把已保存的 `ChatGPT-Account-Id` 覆写成空串导致后续请求全部报错。
fn merge_rotated_metadata(
    existing: Option<&OAuthMetadata>,
    new_refresh_token: &str,
    new_account_id: Option<String>,
    new_email: Option<String>,
    now_ms: i64,
) -> OAuthMetadata {
    let account_id = new_account_id
        .filter(|s| !s.is_empty())
        .or_else(|| existing.map(|m| m.account_id.clone()).filter(|s| !s.is_empty()))
        .unwrap_or_default();
    let email = new_email
        .filter(|s| !s.is_empty())
        .or_else(|| existing.and_then(|m| m.email.clone()));
    let kiro = existing.and_then(|m| m.kiro.clone());
    OAuthMetadata {
        account_id,
        email,
        refresh_token: new_refresh_token.to_string(),
        authenticated_at: now_ms,
        kiro,
    }
}

async fn exchange_code_for_token_with_redirect(
    http: &Client,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<OAuthTokenResp, OAuthError> {
    let resp = http
        .post(OAUTH_TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", CHATGPT_CLIENT_ID),
            ("code_verifier", code_verifier),
        ])
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(OAuthError::UpstreamError(format!("code 换 token 失败: {status} {text}")));
    }
    Ok(resp.json().await?)
}

fn random_base64url(num_bytes: usize) -> Result<String, OAuthError> {
    let mut buf = vec![0u8; num_bytes];
    getrandom::fill(&mut buf)
        .map_err(|e| OAuthError::UpstreamError(format!("随机数生成失败: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(&buf))
}

fn pkce_s256_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

fn build_pkce_authorize_url(code_challenge: &str, state: &str) -> Result<String, OAuthError> {
    let url = reqwest::Url::parse_with_params(
        OAUTH_AUTHORIZE_URL,
        &[
            ("response_type", "code"),
            ("client_id", CHATGPT_CLIENT_ID),
            ("redirect_uri", PKCE_REDIRECT_URI),
            ("scope", PKCE_SCOPE),
            ("code_challenge", code_challenge),
            ("code_challenge_method", "S256"),
            ("id_token_add_organizations", "true"),
            ("codex_cli_simplified_flow", "true"),
            ("state", state),
            ("originator", CODEX_ORIGINATOR),
        ],
    )
    .map_err(|e| OAuthError::Parse(format!("构造授权 URL 失败: {e}")))?;
    Ok(url.into())
}

async fn bind_pkce_callback_listener(
    port: u16,
    wait_previous_release: bool,
) -> Result<TcpListener, OAuthError> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let attempts = if wait_previous_release { 10 } else { 1 };
    for attempt in 0..attempts {
        match TcpListener::bind(addr).await {
            Ok(l) => return Ok(l),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                if attempt + 1 < attempts {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    continue;
                }
                return Err(OAuthError::CallbackPortInUse(port));
            }
            Err(e) => {
                return Err(OAuthError::Network(format!(
                    "绑定本地回调端口 {port} 失败: {e}"
                )));
            }
        }
    }
    Err(OAuthError::CallbackPortInUse(port))
}

fn callback_html(ok: bool, title: &str, detail: &str) -> Html<String> {
    let color = if ok { "#16a34a" } else { "#dc2626" };
    let icon = if ok { "&#10003;" } else { "&#10007;" };
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    Html(format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><title>{title}</title>
<style>body{{font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif;display:flex;align-items:center;justify-content:center;min-height:100vh;margin:0;background:#0f172a;color:#e2e8f0}}
.card{{max-width:440px;padding:32px;border-radius:12px;background:#1e293b;border:1px solid #334155;text-align:center}}
.icon{{font-size:36px;color:{color};margin-bottom:12px}}h1{{font-size:18px;margin:0 0 8px}}p{{font-size:13px;color:#94a3b8;line-height:1.6;margin:0}}</style></head>
<body><div class="card"><div class="icon">{icon}</div><h1>{title}</h1><p>{detail}</p></div></body></html>"#,
        title = esc(title),
        detail = esc(detail),
    ))
}

async fn handle_pkce_callback(
    http: Client,
    browser_flow: Arc<RwLock<Option<ActiveBrowserFlow>>>,
    completed: Arc<RwLock<HashMap<String, (PolledTokens, i64)>>>,
    expected_flow_id: String,
    params: HashMap<String, String>,
) -> impl IntoResponse {
    // 1. 校验 state 防止 CSRF 或过期回调误触
    let state = params.get("state").map(String::as_str).unwrap_or("");
    if state != expected_flow_id {
        return (
            StatusCode::BAD_REQUEST,
            callback_html(
                false,
                "Invalid authorization state",
                "State mismatch. Please close this tab and restart sign-in from cc-router.",
            ),
        );
    }

    // 2. 上游返回错误 (如用户取消授权)
    if let Some(err) = params.get("error") {
        let desc = params
            .get("error_description")
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| err.clone());
        let mut slot = browser_flow.write().await;
        if let Some(active) = slot.as_mut().filter(|a| a.flow_id == expected_flow_id) {
            active.status = BrowserFlowStatus::Failed(desc.clone());
            if let Some(tx) = active.shutdown.take() {
                let _ = tx.send(());
            }
        }
        return (
            StatusCode::BAD_REQUEST,
            callback_html(false, "Authorization failed", &desc),
        );
    }

    let Some(code) = params.get("code").filter(|s| !s.is_empty()).cloned() else {
        return (
            StatusCode::BAD_REQUEST,
            callback_html(
                false,
                "Missing authorization code",
                "No authorization code was returned. Please retry from cc-router.",
            ),
        );
    };

    // 3. 短锁取出 code_verifier (绝不持锁跨越网络请求)
    let code_verifier = {
        let slot = browser_flow.read().await;
        match slot.as_ref() {
            Some(active)
                if active.flow_id == expected_flow_id
                    && matches!(active.status, BrowserFlowStatus::Pending) =>
            {
                active.code_verifier.clone()
            }
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    callback_html(
                        false,
                        "Authorization session expired",
                        "This sign-in session is no longer active. Please restart from cc-router.",
                    ),
                );
            }
        }
    };

    // 4. code 换 token
    let token_res = exchange_code_for_token_with_redirect(
        &http,
        &code,
        &code_verifier,
        PKCE_REDIRECT_URI,
    )
    .await;

    let token = match token_res {
        Ok(t) => t,
        Err(e) => {
            let msg = e.to_string();
            let mut slot = browser_flow.write().await;
            if let Some(active) = slot.as_mut().filter(|a| a.flow_id == expected_flow_id) {
                active.status = BrowserFlowStatus::Failed(msg.clone());
                if let Some(tx) = active.shutdown.take() {
                    let _ = tx.send(());
                }
            }
            return (
                StatusCode::BAD_GATEWAY,
                callback_html(false, "Token exchange failed", &msg),
            );
        }
    };

    let (account_id, email, exp) = extract_account_and_email(&token);
    let Some(account_id) = account_id.filter(|s| !s.is_empty()) else {
        let msg = "id_token 中找不到 chatgpt_account_id, 当前账号可能不是 ChatGPT Plus/Pro".to_string();
        let mut slot = browser_flow.write().await;
        if let Some(active) = slot.as_mut().filter(|a| a.flow_id == expected_flow_id) {
            active.status = BrowserFlowStatus::Failed(msg.clone());
            if let Some(tx) = active.shutdown.take() {
                let _ = tx.send(());
            }
        }
        return (
            StatusCode::BAD_REQUEST,
            callback_html(false, "Account unavailable", &msg),
        );
    };
    let Some(refresh_token) = token.refresh_token.clone().filter(|s| !s.is_empty()) else {
        let msg = "OAuth 响应缺少 refresh_token".to_string();
        let mut slot = browser_flow.write().await;
        if let Some(active) = slot.as_mut().filter(|a| a.flow_id == expected_flow_id) {
            active.status = BrowserFlowStatus::Failed(msg.clone());
            if let Some(tx) = active.shutdown.take() {
                let _ = tx.send(());
            }
        }
        return (
            StatusCode::BAD_REQUEST,
            callback_html(false, "Missing refresh token", &msg),
        );
    };

    let now_ms = Utc::now().timestamp_millis();
    let expires_at_ms = compute_expires_at_ms(exp, token.expires_in);
    let polled = PolledTokens {
        access_token: token.access_token,
        refresh_token,
        account_id: account_id.clone(),
        email: email.clone(),
        authenticated_at_ms: now_ms,
        access_token_expires_at_ms: expires_at_ms,
    };
    let public = ChatGptAccount {
        account_id,
        email,
        authenticated_at: now_ms,
    };

    {
        let mut done = completed.write().await;
        done.retain(|_, (_, expires)| *expires > now_ms);
        done.insert(expected_flow_id.clone(), (polled, now_ms + COMPLETED_POLL_TTL_MS));
    }
    {
        let mut slot = browser_flow.write().await;
        if let Some(active) = slot.as_mut().filter(|a| a.flow_id == expected_flow_id) {
            active.status = BrowserFlowStatus::Completed(public);
            if let Some(tx) = active.shutdown.take() {
                let _ = tx.send(());
            }
        }
    }

    (
        StatusCode::OK,
        callback_html(
            true,
            "Connected to cc-router",
            "Authorization complete. You can close this tab and return to cc-router.",
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn in_memory_pool() -> SqlitePool {
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    fn mk_jwt(payload: &serde_json::Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(b"{\"alg\":\"none\"}");
        let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(payload).unwrap());
        format!("{header}.{body}.signature")
    }

    #[test]
    fn jwt_payload_decode_picks_top_level_account_id() {
        let token = mk_jwt(&serde_json::json!({
            "exp": 1700000000_i64,
            "chatgpt_account_id": "acc-123",
            "email": "foo@bar.com",
        }));
        let claims = decode_jwt_payload(&token).unwrap();
        assert_eq!(claims.exp, Some(1700000000));
        assert_eq!(claims.chatgpt_account_id.as_deref(), Some("acc-123"));
        assert_eq!(claims.email.as_deref(), Some("foo@bar.com"));
    }

    #[test]
    fn jwt_payload_decode_falls_back_to_nested_claim() {
        let token = mk_jwt(&serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acc-nested"
            }
        }));
        let claims = decode_jwt_payload(&token).unwrap();
        let resp = OAuthTokenResp {
            access_token: "x".into(),
            refresh_token: None,
            id_token: Some(token),
            expires_in: Some(3600),
        };
        let (acc, _, _) = extract_account_and_email(&resp);
        assert_eq!(acc.as_deref(), Some("acc-nested"));
        assert!(claims.openai_auth.is_some());
    }

    #[test]
    fn cached_token_expiry_check_honors_buffer() {
        let now = Utc::now().timestamp_millis();
        let still_fresh = CachedToken { access_token: "x".into(), expires_at_ms: now + 5 * 60 * 1000 };
        let about_to_expire = CachedToken { access_token: "x".into(), expires_at_ms: now + 30 * 1000 };
        assert!(!still_fresh.is_expiring_soon());
        assert!(about_to_expire.is_expiring_soon());
    }

    /// 验证 OAuth token 响应解析: id_token claims 抽取 account_id / email / exp,
    /// 且 refresh_token rotation 字段存在. 不真正驱动 manager.get_valid_access_token,
    /// 因为 refresh URL 写死常量, 单测无法 swap host (可后续把 URL 改成 manager 字段)。
    #[tokio::test]
    async fn oauth_token_resp_parses_with_id_token() {
        let server = MockServer::start().await;

        // OpenAI OAuth Token URL
        let new_id_token = mk_jwt(&serde_json::json!({
            "exp": (Utc::now().timestamp() + 3600) as i64,
            "chatgpt_account_id": "acc-X",
            "email": "x@y.com",
        }));
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "fresh-token",
                "refresh_token": "rotated-refresh",
                "id_token": new_id_token,
                "expires_in": 3600,
            })))
            .mount(&server)
            .await;

        // 借助 manager 的内部 refresh_with_token 只是私有, 这里改测公开的 get_valid_access_token.
        // 但默认走真实 OpenAI URL, 我们重定义常量不太可能 -> 改用临时 client + 测内部函数等价.
        let pool = in_memory_pool().await;
        let manager = ChatGptOAuthManager::new(pool);
        // 替换内部 http 与 URL: 单测里我们直接调 refresh_with_token 但 URL 写死, 为了走 wiremock,
        // 这里用 manager.http 重新发请求, 验证响应解析路径正确即可.
        let resp = manager
            .http
            .post(format!("{}/oauth/token", server.uri()))
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", "old-refresh"),
                ("client_id", CHATGPT_CLIENT_ID),
            ])
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success());
        let parsed: OAuthTokenResp = resp.json().await.unwrap();
        assert_eq!(parsed.access_token, "fresh-token");
        assert_eq!(parsed.refresh_token.as_deref(), Some("rotated-refresh"));
        let (acc, email, exp) = extract_account_and_email(&parsed);
        assert_eq!(acc.as_deref(), Some("acc-X"));
        assert_eq!(email.as_deref(), Some("x@y.com"));
        assert!(exp.is_some());
    }

    #[test]
    fn merge_rotated_metadata_preserves_existing_account_and_email_when_missing() {
        let existing = OAuthMetadata {
            account_id: "acct-keep".into(),
            email: Some("keep@example.com".into()),
            refresh_token: "old-rt".into(),
            authenticated_at: 100,
            kiro: None,
        };
        let merged = merge_rotated_metadata(Some(&existing), "new-rt", None, None, 200);
        assert_eq!(merged.account_id, "acct-keep");
        assert_eq!(merged.email.as_deref(), Some("keep@example.com"));
        assert_eq!(merged.refresh_token, "new-rt");
        assert_eq!(merged.authenticated_at, 200);

        // 新响应返回空字符串时同样不覆盖既有值
        let merged_empty = merge_rotated_metadata(
            Some(&existing),
            "new-rt-2",
            Some(String::new()),
            Some(String::new()),
            300,
        );
        assert_eq!(merged_empty.account_id, "acct-keep");
        assert_eq!(merged_empty.email.as_deref(), Some("keep@example.com"));

        // 新响应带非空值时更新
        let merged_updated = merge_rotated_metadata(
            Some(&existing),
            "new-rt-3",
            Some("acct-new".into()),
            Some("new@example.com".into()),
            400,
        );
        assert_eq!(merged_updated.account_id, "acct-new");
        assert_eq!(merged_updated.email.as_deref(), Some("new@example.com"));
    }

    #[test]
    fn pkce_challenge_matches_rfc7636_and_authorize_url_has_required_params() {
        // RFC 7636 Appendix B test vector
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            pkce_s256_challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );

        let url_str = build_pkce_authorize_url("chal_123", "state_abc").unwrap();
        let parsed = reqwest::Url::parse(&url_str).unwrap();
        assert_eq!(parsed.origin().ascii_serialization(), "https://auth.openai.com");
        assert_eq!(parsed.path(), "/oauth/authorize");
        let qs: HashMap<String, String> = parsed.query_pairs().into_owned().collect();
        assert_eq!(qs.get("response_type").map(String::as_str), Some("code"));
        assert_eq!(qs.get("client_id").map(String::as_str), Some(CHATGPT_CLIENT_ID));
        assert_eq!(qs.get("redirect_uri").map(String::as_str), Some(PKCE_REDIRECT_URI));
        assert_eq!(qs.get("code_challenge").map(String::as_str), Some("chal_123"));
        assert_eq!(qs.get("code_challenge_method").map(String::as_str), Some("S256"));
        assert_eq!(qs.get("state").map(String::as_str), Some("state_abc"));
        assert_eq!(qs.get("codex_cli_simplified_flow").map(String::as_str), Some("true"));
        assert_eq!(qs.get("id_token_add_organizations").map(String::as_str), Some("true"));
        assert_eq!(qs.get("originator").map(String::as_str), Some(CODEX_ORIGINATOR));
    }
}
