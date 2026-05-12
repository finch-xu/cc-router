//! Kiro IDE / AWS Builder ID OAuth 管理.
//!
//! 凭据来源有两种, 都汇入同一个 `KiroOAuthExtras`:
//! - JSON 文件 / 粘贴: 来自 Kiro IDE 落盘的 `~/.aws/sso/cache/kiro-auth-token.json` 或同结构 JSON
//! - AWS SSO OIDC Device Authorization Flow: register-client → start-device-auth → poll-token
//!
//! 凭据带 `client_id + client_secret` → `auth_method=Idc`, 否则 → `Social` (Kiro IDE 桌面登录).
//! 两种 auth_method 走不同 refresh endpoint:
//! - `Social`: POST `https://prod.{region}.auth.desktop.kiro.dev/refreshToken`, body `{"refreshToken": "..."}`
//! - `Idc`:    POST `https://oidc.{region}.amazonaws.com/token`, body OIDC 标准

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::SqlitePool;
use tokio::sync::{Mutex, RwLock};
use tracing::{info, warn};
use uuid::Uuid;

use crate::subscription::model::{
    KiroAuthMethod, KiroDisguise, KiroOAuthExtras, OAuthMetadata,
};
use crate::subscription::store;

use super::chatgpt::OAuthError;

// =====================================================================
// 常量
// =====================================================================

/// AWS SSO OIDC 服务端点模板. 用于 IdC (含 AWS Builder ID) 的 register-client / device-auth / token endpoints.
const SSO_OIDC_BASE_TEMPLATE: &str = "https://oidc.{region}.amazonaws.com";

/// Kiro IDE 桌面登录 refresh endpoint 模板.
const KIRO_DESKTOP_REFRESH_TEMPLATE: &str = "https://prod.{region}.auth.desktop.kiro.dev/refreshToken";

pub const DEFAULT_KIRO_REGION: &str = "us-east-1";

/// access_token 提前 60s 视为过期.
const TOKEN_REFRESH_BUFFER_MS: i64 = 60_000;
/// Device code 默认有效期 (秒). AWS SSO OIDC 一般给 600s.
const DEVICE_CODE_DEFAULT_EXPIRES_IN: u64 = 600;
/// Polled tokens 缓存 10min 等前端 consume.
const COMPLETED_POLL_TTL_MS: i64 = 10 * 60 * 1000;

/// AWS SSO OIDC register-client 的 clientName 字段. 用于 OIDC 流程仿装客户端身份.
/// 选用一个有辨识度但不冲击官方 codex_cli_rs / kiro_cli 的标识.
const SSO_CLIENT_NAME: &str = "cc-router-kiro";

/// register-client 的 clientType. AWS OIDC 文档要求是 "public" (Builder ID 免费场景).
const SSO_CLIENT_TYPE: &str = "public";

/// Scope 列表 (kiro-cli 实测有效).
const SSO_SCOPES: &[&str] = &["codewhisperer:completions"];

/// Kiro / CodeWhisperer 调用上游时必带的 header 名 + AWS SDK 仿装 UA 模板.
/// `oauth_dispatch.rs::dispatch_kiro_attempt` 引用; 提常量是为了和 `compatibility_notes`
/// / yaml `required_headers` / 实际请求保持「事实唯一源」.
pub const KIRO_OPTOUT_HEADER: &str = "x-amzn-codewhisperer-optout";
pub const KIRO_AGENT_MODE_HEADER: &str = "x-amzn-kiro-agent-mode";
pub const KIRO_AMZ_UA_HEADER: &str = "x-amz-user-agent";
pub const KIRO_AMZ_INVOCATION_HEADER: &str = "amz-sdk-invocation-id";

// =====================================================================
// HTTP 协议结构
// =====================================================================

/// AWS SSO OIDC register-client response.
#[derive(Debug, Deserialize)]
struct RegisterClientResp {
    #[serde(rename = "clientId")]
    client_id: String,
    #[serde(rename = "clientSecret")]
    client_secret: String,
    #[allow(dead_code)]
    #[serde(rename = "clientIdIssuedAt", default)]
    client_id_issued_at: Option<i64>,
    #[allow(dead_code)]
    #[serde(rename = "clientSecretExpiresAt", default)]
    client_secret_expires_at: Option<i64>,
}

/// AWS SSO OIDC start-device-authorization response.
#[derive(Debug, Deserialize)]
struct StartDeviceAuthResp {
    #[serde(rename = "deviceCode")]
    device_code: String,
    #[serde(rename = "userCode")]
    user_code: String,
    #[serde(rename = "verificationUri", default)]
    verification_uri: Option<String>,
    #[serde(rename = "verificationUriComplete", default)]
    verification_uri_complete: Option<String>,
    #[serde(rename = "expiresIn", default)]
    expires_in: Option<u64>,
    #[allow(dead_code)]
    #[serde(rename = "interval", default)]
    interval: Option<u64>,
}

/// AWS SSO OIDC create-token response.
#[derive(Debug, Deserialize)]
struct CreateTokenResp {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "refreshToken", default)]
    refresh_token: Option<String>,
    #[serde(rename = "tokenType", default)]
    #[allow(dead_code)]
    token_type: Option<String>,
    #[serde(rename = "expiresIn", default)]
    expires_in: Option<i64>,
}

/// Kiro 桌面 refresh endpoint response.
#[derive(Debug, Deserialize)]
struct KiroDesktopRefreshResp {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "refreshToken", default)]
    refresh_token: Option<String>,
    #[serde(rename = "expiresIn", default)]
    expires_in: Option<i64>,
}

/// `~/.aws/sso/cache/kiro-auth-token.json` 的反序列化目标. clientId/clientSecret 同时存在则视为 IdC.
#[derive(Debug, Clone, Deserialize)]
pub struct KiroCredentialJson {
    #[serde(rename = "accessToken", default)]
    pub access_token: Option<String>,
    #[serde(rename = "refreshToken")]
    pub refresh_token: String,
    #[serde(rename = "expiresAt", default)]
    pub expires_at: Option<String>,
    #[serde(rename = "profileArn", default)]
    pub profile_arn: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(rename = "clientId", default)]
    pub client_id: Option<String>,
    #[serde(rename = "clientSecret", default)]
    pub client_secret: Option<String>,
}

/// 暴露给前端的导入结果. 不含 refresh_token.
#[derive(Debug, Clone, Serialize)]
pub struct KiroImportPreview {
    pub auth_method: KiroAuthMethod,
    pub region: String,
    pub has_profile_arn: bool,
    pub has_access_token: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct KiroDeviceFlowStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub region: String,
    pub expires_in: u64,
}

/// 完成 Device Flow 后给前端的账户摘要.
#[derive(Debug, Clone, Serialize)]
pub struct KiroAccount {
    pub auth_method: KiroAuthMethod,
    pub region: String,
    pub authenticated_at: i64,
}

// =====================================================================
// 内存缓存类型
// =====================================================================

#[derive(Debug, Clone)]
struct CachedToken {
    access_token: String,
    expires_at_ms: i64,
}

impl CachedToken {
    fn is_expiring_soon(&self) -> bool {
        Utc::now().timestamp_millis() + TOKEN_REFRESH_BUFFER_MS >= self.expires_at_ms
    }
}

/// 进行中的 OIDC Device Flow.
#[derive(Debug, Clone)]
struct PendingDevice {
    /// 这次 device flow 用的 register-client 产物
    client_id: String,
    client_secret: String,
    region: String,
    expires_at_ms: i64,
}

/// 完成 device flow 后, 前端拿 device_code 调 consume_completed_poll 取走全套.
#[derive(Debug, Clone)]
pub struct KiroPolledTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub access_token_expires_at_ms: i64,
    pub authenticated_at_ms: i64,
    pub region: String,
    pub auth_method: KiroAuthMethod,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub profile_arn: Option<String>,
}

// =====================================================================
// Manager
// =====================================================================

pub struct KiroOAuthManager {
    pool: SqlitePool,
    http: Client,
    cache: Arc<RwLock<HashMap<Uuid, CachedToken>>>,
    locks: Arc<RwLock<HashMap<Uuid, Arc<Mutex<()>>>>>,
    /// device_code → pending (方案 B 进行中)
    pending: Arc<RwLock<HashMap<String, PendingDevice>>>,
    /// device_code → (tokens, expires_at_ms) (方案 B 完成待消费)
    completed: Arc<RwLock<HashMap<String, (KiroPolledTokens, i64)>>>,
    /// 方案 A import session_id → (tokens, expires_at_ms) 的待消费缓存.
    /// 凭据导入后, 前端确认伪装字段并创建订阅时再 consume.
    imported: Arc<RwLock<HashMap<String, (KiroPolledTokens, i64)>>>,
}

impl KiroOAuthManager {
    pub fn new(pool: SqlitePool) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("无法构建 Kiro OAuth http client");
        Self {
            pool,
            http,
            cache: Arc::new(RwLock::new(HashMap::new())),
            locks: Arc::new(RwLock::new(HashMap::new())),
            pending: Arc::new(RwLock::new(HashMap::new())),
            completed: Arc::new(RwLock::new(HashMap::new())),
            imported: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 缓存刚解析出的凭据, 等前端创建订阅时 consume_imported_session 取走.
    pub async fn cache_imported_session(&self, session_id: &str, tokens: KiroPolledTokens) {
        let now_ms = Utc::now().timestamp_millis();
        let mut imp = self.imported.write().await;
        imp.retain(|_, (_, expires)| *expires > now_ms);
        imp.insert(session_id.to_string(), (tokens, now_ms + COMPLETED_POLL_TTL_MS));
    }

    /// 取走 import session, 一次性消费.
    pub async fn consume_imported_session(
        &self,
        session_id: &str,
    ) -> Result<KiroPolledTokens, OAuthError> {
        let mut imp = self.imported.write().await;
        let now_ms = Utc::now().timestamp_millis();
        imp.retain(|_, (_, expires)| *expires > now_ms);
        let (tokens, _) = imp
            .remove(session_id)
            .ok_or(OAuthError::PendingDeviceNotFound)?;
        Ok(tokens)
    }

    /// 解析 Kiro IDE 落盘 JSON / 用户粘贴文本, 转成内部凭据结构. 不落库, 由调用方决定后续创建订阅.
    pub fn parse_credential_json(
        &self,
        json_str: &str,
    ) -> Result<(KiroPolledTokens, KiroImportPreview), OAuthError> {
        let cred: KiroCredentialJson = serde_json::from_str(json_str)
            .map_err(|e| OAuthError::Parse(format!("Kiro 凭据 JSON 解析失败: {e}")))?;
        if cred.refresh_token.is_empty() {
            return Err(OAuthError::Parse("Kiro 凭据缺少 refreshToken".into()));
        }
        let auth_method = if cred.client_id.is_some() && cred.client_secret.is_some() {
            KiroAuthMethod::Idc
        } else {
            KiroAuthMethod::Social
        };
        let region = cred
            .region
            .clone()
            .unwrap_or_else(|| DEFAULT_KIRO_REGION.to_string());

        let now_ms = Utc::now().timestamp_millis();
        let expires_at_ms = cred
            .expires_at
            .as_deref()
            .and_then(parse_iso8601_ms)
            .unwrap_or(now_ms);

        let preview = KiroImportPreview {
            auth_method,
            region: region.clone(),
            has_profile_arn: cred.profile_arn.is_some(),
            has_access_token: cred.access_token.is_some(),
        };

        let tokens = KiroPolledTokens {
            access_token: cred.access_token.unwrap_or_default(),
            refresh_token: cred.refresh_token,
            access_token_expires_at_ms: expires_at_ms,
            authenticated_at_ms: now_ms,
            region,
            auth_method,
            client_id: cred.client_id,
            client_secret: cred.client_secret,
            profile_arn: cred.profile_arn,
        };

        Ok((tokens, preview))
    }

    /// 启动 AWS SSO OIDC Device Authorization Flow:
    /// register-client 拿 clientId/clientSecret → start-device-authorization 拿 deviceCode + userCode.
    pub async fn start_device_flow(
        &self,
        region: Option<&str>,
    ) -> Result<KiroDeviceFlowStart, OAuthError> {
        let region = region
            .map(str::to_string)
            .unwrap_or_else(|| DEFAULT_KIRO_REGION.to_string());
        let base = sso_oidc_base(&region);

        // Step 1: register-client
        let reg_url = format!("{}/client/register", base);
        let resp = self
            .http
            .post(&reg_url)
            .json(&json!({
                "clientName": SSO_CLIENT_NAME,
                "clientType": SSO_CLIENT_TYPE,
                "scopes": SSO_SCOPES,
            }))
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(OAuthError::UpstreamError(format!(
                "SSO register-client 失败: {status} {text}"
            )));
        }
        let reg: RegisterClientResp = resp.json().await?;

        // Step 2: start-device-authorization
        let da_url = format!("{}/device_authorization", base);
        let resp = self
            .http
            .post(&da_url)
            .json(&json!({
                "clientId": reg.client_id,
                "clientSecret": reg.client_secret,
                "startUrl": "https://view.awsapps.com/start", // Builder ID portal
            }))
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(OAuthError::UpstreamError(format!(
                "SSO start-device-authorization 失败: {status} {text}"
            )));
        }
        let da: StartDeviceAuthResp = resp.json().await?;
        let expires_in = da.expires_in.unwrap_or(DEVICE_CODE_DEFAULT_EXPIRES_IN);
        let now_ms = Utc::now().timestamp_millis();

        let pending = PendingDevice {
            client_id: reg.client_id,
            client_secret: reg.client_secret,
            region: region.clone(),
            expires_at_ms: now_ms + expires_in as i64 * 1000,
        };
        {
            let mut p = self.pending.write().await;
            p.retain(|_, v| v.expires_at_ms > now_ms);
            p.insert(da.device_code.clone(), pending);
        }

        let verification_uri = da
            .verification_uri
            .clone()
            .unwrap_or_else(|| "https://view.awsapps.com/start".to_string());

        Ok(KiroDeviceFlowStart {
            device_code: da.device_code,
            user_code: da.user_code,
            verification_uri,
            verification_uri_complete: da.verification_uri_complete,
            region,
            expires_in,
        })
    }

    /// 轮询 device code. None = 用户尚未授权, Some = 完成.
    pub async fn poll_device_code(
        &self,
        device_code: &str,
    ) -> Result<Option<KiroAccount>, OAuthError> {
        let pending = {
            let r = self.pending.read().await;
            r.get(device_code).cloned()
        };
        let pending = pending.ok_or(OAuthError::PendingDeviceNotFound)?;

        if pending.expires_at_ms <= Utc::now().timestamp_millis() {
            self.pending.write().await.remove(device_code);
            return Err(OAuthError::DeviceCodeExpired);
        }

        let token_url = format!("{}/token", sso_oidc_base(&pending.region));
        let resp = self
            .http
            .post(&token_url)
            .json(&json!({
                "clientId": pending.client_id,
                "clientSecret": pending.client_secret,
                "grantType": "urn:ietf:params:oauth:grant-type:device_code",
                "deviceCode": device_code,
            }))
            .send()
            .await?;
        let status = resp.status();
        if status == reqwest::StatusCode::BAD_REQUEST {
            // AWS 用 400 + body {"error":"authorization_pending"} 表示「还没授权」
            let text = resp.text().await.unwrap_or_default();
            if text.contains("authorization_pending") || text.contains("slow_down") {
                return Ok(None);
            }
            if text.contains("expired_token") || text.contains("invalid_grant") {
                self.pending.write().await.remove(device_code);
                return Err(OAuthError::DeviceCodeExpired);
            }
            return Err(OAuthError::UpstreamError(format!("SSO token 400: {text}")));
        }
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(OAuthError::UpstreamError(format!(
                "SSO token 失败: {status} {text}"
            )));
        }

        let tok: CreateTokenResp = resp.json().await?;
        let refresh_token = tok.refresh_token.clone().ok_or_else(|| {
            OAuthError::Parse("SSO token 响应缺少 refreshToken".into())
        })?;
        let now_ms = Utc::now().timestamp_millis();
        let expires_at_ms = now_ms + tok.expires_in.unwrap_or(3600) * 1000;

        let polled = KiroPolledTokens {
            access_token: tok.access_token,
            refresh_token,
            access_token_expires_at_ms: expires_at_ms,
            authenticated_at_ms: now_ms,
            region: pending.region.clone(),
            auth_method: KiroAuthMethod::Idc,
            client_id: Some(pending.client_id.clone()),
            client_secret: Some(pending.client_secret.clone()),
            profile_arn: None,
        };
        let public = KiroAccount {
            auth_method: KiroAuthMethod::Idc,
            region: pending.region,
            authenticated_at: now_ms,
        };

        self.pending.write().await.remove(device_code);
        {
            let mut completed = self.completed.write().await;
            completed.retain(|_, (_, expires)| *expires > now_ms);
            completed.insert(
                device_code.to_string(),
                (polled, now_ms + COMPLETED_POLL_TTL_MS),
            );
        }
        Ok(Some(public))
    }

    /// 取走 device flow 已完成的 polled tokens (一次性消费).
    pub async fn consume_completed_poll(
        &self,
        device_code: &str,
    ) -> Result<KiroPolledTokens, OAuthError> {
        let mut completed = self.completed.write().await;
        let now_ms = Utc::now().timestamp_millis();
        completed.retain(|_, (_, expires)| *expires > now_ms);
        let (tokens, _) = completed
            .remove(device_code)
            .ok_or(OAuthError::PendingDeviceNotFound)?;
        Ok(tokens)
    }

    // ----------------------- 共用 -----------------------

    /// 把 polled token 集合 → OAuthMetadata (供 store 写订阅). 自动注入默认伪装字段.
    pub fn polled_to_metadata(
        &self,
        polled: &KiroPolledTokens,
        override_disguise: Option<KiroDisguise>,
    ) -> OAuthMetadata {
        let disguise = override_disguise.unwrap_or_default();
        OAuthMetadata {
            account_id: String::new(),
            email: None,
            refresh_token: polled.refresh_token.clone(),
            authenticated_at: polled.authenticated_at_ms,
            kiro: Some(KiroOAuthExtras {
                auth_method: polled.auth_method,
                region: polled.region.clone(),
                profile_arn: polled.profile_arn.clone(),
                client_id: polled.client_id.clone(),
                client_secret: polled.client_secret.clone(),
                disguise,
            }),
        }
    }

    /// 把 access_token 注入指定订阅缓存. 用于刚导入凭据或刚 device flow 完成时跳过首次 refresh.
    pub async fn seed_cache(&self, sub_id: Uuid, access_token: String, expires_at_ms: i64) {
        if access_token.is_empty() {
            return;
        }
        let mut cache = self.cache.write().await;
        cache.insert(
            sub_id,
            CachedToken {
                access_token,
                expires_at_ms,
            },
        );
    }

    /// 主入口: pipeline 每次请求前调. 优先缓存, 否则按 auth_method 走对应 refresh.
    pub async fn get_valid_access_token(
        &self,
        sub_id: Uuid,
        metadata: &OAuthMetadata,
    ) -> Result<String, OAuthError> {
        let extras = metadata
            .kiro
            .as_ref()
            .ok_or_else(|| OAuthError::Parse("订阅缺少 Kiro OAuth 元数据".into()))?;

        // 1. 缓存检查
        {
            let cache = self.cache.read().await;
            if let Some(t) = cache.get(&sub_id) {
                if !t.is_expiring_soon() {
                    return Ok(t.access_token.clone());
                }
            }
        }

        // 2. per-sub 锁
        let lock = {
            let mut locks = self.locks.write().await;
            locks
                .entry(sub_id)
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
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

        // 4. refresh (按 auth_method 分支)
        info!(%sub_id, method = ?extras.auth_method, "refreshing Kiro access_token");
        let (new_access, new_refresh, new_expires_at_ms) =
            self.refresh_by_method(&metadata.refresh_token, extras).await?;

        // 5. refresh_token rotation 落盘
        if let Some(rt) = new_refresh.as_deref() {
            if rt != metadata.refresh_token {
                info!(%sub_id, "Kiro refresh_token rotated, persisting");
                let mut new_meta = metadata.clone();
                new_meta.refresh_token = rt.to_string();
                if let Err(e) = store::update_oauth_metadata(&self.pool, &sub_id, &new_meta).await {
                    warn!(%sub_id, error = ?e, "落盘新 Kiro refresh_token 失败");
                }
            }
        }

        // 6. 写缓存
        let mut cache = self.cache.write().await;
        cache.insert(
            sub_id,
            CachedToken {
                access_token: new_access.clone(),
                expires_at_ms: new_expires_at_ms,
            },
        );

        Ok(new_access)
    }

    pub async fn forget(&self, sub_id: Uuid) {
        self.cache.write().await.remove(&sub_id);
        self.locks.write().await.remove(&sub_id);
    }

    /// 按 auth_method 分支调对应 refresh endpoint. 返回 (新 access_token, 可能轮转的 refresh_token, 过期 ms).
    async fn refresh_by_method(
        &self,
        refresh_token: &str,
        extras: &KiroOAuthExtras,
    ) -> Result<(String, Option<String>, i64), OAuthError> {
        match extras.auth_method {
            KiroAuthMethod::Social => self.refresh_social(refresh_token, &extras.region).await,
            KiroAuthMethod::Idc => {
                let client_id = extras.client_id.as_deref().ok_or_else(|| {
                    OAuthError::Parse("IdC auth_method 缺少 client_id".into())
                })?;
                let client_secret = extras.client_secret.as_deref().ok_or_else(|| {
                    OAuthError::Parse("IdC auth_method 缺少 client_secret".into())
                })?;
                self.refresh_idc(refresh_token, &extras.region, client_id, client_secret).await
            }
        }
    }

    async fn refresh_social(
        &self,
        refresh_token: &str,
        region: &str,
    ) -> Result<(String, Option<String>, i64), OAuthError> {
        let url = KIRO_DESKTOP_REFRESH_TEMPLATE.replace("{region}", region);
        let resp = self
            .http
            .post(&url)
            .json(&json!({ "refreshToken": refresh_token }))
            .send()
            .await?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(OAuthError::RefreshTokenInvalid);
        }
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(OAuthError::UpstreamError(format!(
                "Kiro Social refresh 失败: {status} {text}"
            )));
        }
        let body: KiroDesktopRefreshResp = resp.json().await?;
        let now_ms = Utc::now().timestamp_millis();
        let expires_at = now_ms + body.expires_in.unwrap_or(3600) * 1000;
        Ok((body.access_token, body.refresh_token, expires_at))
    }

    async fn refresh_idc(
        &self,
        refresh_token: &str,
        region: &str,
        client_id: &str,
        client_secret: &str,
    ) -> Result<(String, Option<String>, i64), OAuthError> {
        let url = format!("{}/token", sso_oidc_base(region));
        let resp = self
            .http
            .post(&url)
            .json(&json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": "refresh_token",
                "refreshToken": refresh_token,
            }))
            .send()
            .await?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(OAuthError::RefreshTokenInvalid);
        }
        if status == reqwest::StatusCode::BAD_REQUEST {
            let text = resp.text().await.unwrap_or_default();
            if text.contains("invalid_grant") || text.contains("expired_token") {
                return Err(OAuthError::RefreshTokenInvalid);
            }
            return Err(OAuthError::UpstreamError(format!(
                "Kiro IdC refresh 400: {text}"
            )));
        }
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(OAuthError::UpstreamError(format!(
                "Kiro IdC refresh 失败: {status} {text}"
            )));
        }
        let body: CreateTokenResp = resp.json().await?;
        let now_ms = Utc::now().timestamp_millis();
        let expires_at = now_ms + body.expires_in.unwrap_or(3600) * 1000;
        Ok((body.access_token, body.refresh_token, expires_at))
    }
}

fn sso_oidc_base(region: &str) -> String {
    SSO_OIDC_BASE_TEMPLATE.replace("{region}", region)
}

/// 把 ISO8601 (RFC3339) 时间串转 unix ms. 失败返回 None.
fn parse_iso8601_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription::model::{default_kiro_version, default_node_version};
    use sqlx::sqlite::SqlitePoolOptions;

    async fn in_memory_pool() -> SqlitePool {
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn parse_credential_json_detects_social_method() {
        let pool = in_memory_pool().await;
        let mgr = KiroOAuthManager::new(pool);
        let json = r#"{
            "accessToken": "eyJa",
            "refreshToken": "eyJr",
            "expiresAt": "2099-01-01T00:00:00.000Z",
            "profileArn": "arn:aws:codewhisperer:us-east-1:111:profile/X",
            "region": "us-east-1"
        }"#;
        let (tokens, preview) = mgr.parse_credential_json(json).unwrap();
        assert_eq!(preview.auth_method, KiroAuthMethod::Social);
        assert_eq!(preview.region, "us-east-1");
        assert!(preview.has_profile_arn);
        assert!(preview.has_access_token);
        assert_eq!(tokens.refresh_token, "eyJr");
        assert!(tokens.access_token_expires_at_ms > 4_000_000_000_000);
    }

    #[tokio::test]
    async fn parse_credential_json_detects_idc_method() {
        let pool = in_memory_pool().await;
        let mgr = KiroOAuthManager::new(pool);
        let json = r#"{
            "refreshToken": "rt",
            "region": "us-west-2",
            "clientId": "ci",
            "clientSecret": "cs"
        }"#;
        let (tokens, preview) = mgr.parse_credential_json(json).unwrap();
        assert_eq!(preview.auth_method, KiroAuthMethod::Idc);
        assert_eq!(preview.region, "us-west-2");
        assert!(!preview.has_profile_arn);
        assert_eq!(tokens.client_id.as_deref(), Some("ci"));
        assert_eq!(tokens.client_secret.as_deref(), Some("cs"));
    }

    #[tokio::test]
    async fn parse_credential_json_rejects_missing_refresh_token() {
        let pool = in_memory_pool().await;
        let mgr = KiroOAuthManager::new(pool);
        let json = r#"{"accessToken": "x"}"#;
        let err = mgr.parse_credential_json(json).unwrap_err();
        assert!(matches!(err, OAuthError::Parse(_)));
    }

    #[test]
    fn sso_oidc_base_substitutes_region() {
        assert_eq!(sso_oidc_base("us-east-1"), "https://oidc.us-east-1.amazonaws.com");
        assert_eq!(sso_oidc_base("ap-southeast-1"), "https://oidc.ap-southeast-1.amazonaws.com");
    }

    #[test]
    fn kiro_desktop_refresh_url_substitutes_region() {
        let url = KIRO_DESKTOP_REFRESH_TEMPLATE.replace("{region}", "us-east-1");
        assert_eq!(url, "https://prod.us-east-1.auth.desktop.kiro.dev/refreshToken");
    }

    #[test]
    fn default_disguise_provides_valid_machine_id() {
        let d = KiroDisguise::default();
        assert_eq!(d.machine_id.len(), 64);
        assert!(d.machine_id.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(d.kiro_version, default_kiro_version());
        assert_eq!(d.node_version, default_node_version());
    }

    #[tokio::test]
    async fn polled_to_metadata_populates_kiro_extras() {
        let pool = in_memory_pool().await;
        let mgr = KiroOAuthManager::new(pool);
        let polled = KiroPolledTokens {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            access_token_expires_at_ms: 1_700_000_000_000,
            authenticated_at_ms: 1_699_000_000_000,
            region: "us-east-1".into(),
            auth_method: KiroAuthMethod::Idc,
            client_id: Some("ci".into()),
            client_secret: Some("cs".into()),
            profile_arn: None,
        };
        let meta = mgr.polled_to_metadata(&polled, None);
        assert_eq!(meta.refresh_token, "rt");
        assert_eq!(meta.authenticated_at, 1_699_000_000_000);
        let extras = meta.kiro.unwrap();
        assert_eq!(extras.auth_method, KiroAuthMethod::Idc);
        assert_eq!(extras.region, "us-east-1");
        assert_eq!(extras.client_id.as_deref(), Some("ci"));
        assert!(extras.profile_arn.is_none());
        assert_eq!(extras.disguise.machine_id.len(), 64);
    }
}
