use std::collections::HashMap;
use std::sync::Arc;

use tauri::AppHandle;
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

use crate::oauth::chatgpt::ChatGptOAuthManager;
use crate::oauth::kiro::KiroOAuthManager;
use crate::observability::body_dump::BodyDumpEntry;
use crate::observability::events::EventEntry;
use crate::observability::request_log::RequestLogEntry;
use crate::provider::model::Provider;
use crate::proxy::web::auth::{LoginGuard, SessionStore};
use crate::proxy::web::events::UiEvent;
use crate::settings::model::Settings;
use crate::subscription::model::SubscriptionRuntime;
use crate::virtual_model::affinity::AffinityTable;
use crate::virtual_model::model::{VirtualModelConfig, VirtualModelName};

/// 全局 app 状态。所有 Tauri commands、代理服务、后台任务都共享这份状态。
#[derive(Clone)]
pub struct AppState {
    pub db: sqlx::SqlitePool,
    pub providers: Arc<HashMap<String, Provider>>,
    pub subscriptions: Arc<RwLock<HashMap<Uuid, Arc<RwLock<SubscriptionRuntime>>>>>,
    pub virtual_models: Arc<RwLock<HashMap<VirtualModelName, VirtualModelConfig>>>,
    pub settings: Arc<RwLock<Settings>>,
    /// HTTP listener 实际绑定到的端口. None=HTTP listener 未启用 (HTTPS-only 模式).
    /// 实际值在启动绑定完成后写入, 可能因端口冲突 +1 与 settings.proxy_port 不同.
    pub http_bound_port: Arc<RwLock<Option<u16>>>,
    /// HTTPS listener 实际绑定到的端口. None=HTTPS listener 未启用 (HTTP-only 模式).
    pub https_bound_port: Arc<RwLock<Option<u16>>>,
    /// rustls server config, 只有 HTTPS 模式启动时填值; HTTP-only 模式为 None.
    pub tls_config: Option<Arc<rustls::ServerConfig>>,
    pub request_log_tx: mpsc::Sender<RequestLogEntry>,
    pub event_log_tx: mpsc::Sender<EventEntry>,
    /// 调试模式 dump channel. 仅在 settings.debug_mode=true 时被插桩点投递,
    /// consumer 在 lib.rs::bootstrap 起的后台任务里把每条 entry 写成一个 .txt 文件.
    pub body_dump_tx: mpsc::Sender<BodyDumpEntry>,
    pub http_client: reqwest::Client,
    /// 短超时(30s) 单例, 仅用于订阅可达性探测(测试连接 + 后台巡检)。
    /// 与 `http_client` 的 600s 上限分离: 探测期望快速判定, 慢响应等同失败。
    pub probe_client: reqwest::Client,
    /// ChatGPT OAuth manager (Phase 1 接入). 全订阅共享一份, 内含每订阅 access_token 缓存与 refresh 锁.
    pub chatgpt_oauth: Arc<ChatGptOAuthManager>,
    pub kiro_oauth: Arc<KiroOAuthManager>,
    pub app_handle: AppHandle,
    /// sticky 模式的会话 → 订阅亲和表. 内存, 不持久化.
    pub session_affinity: Arc<std::sync::Mutex<AffinityTable>>,
    /// 网页界面会话表 (内存, 不持久化). generate_new_token 时清空.
    pub web_sessions: Arc<std::sync::Mutex<SessionStore>>,
    /// 网页登录按 IP 限流表.
    pub web_login_guard: Arc<std::sync::Mutex<LoginGuard>>,
    /// Tauri emit 事件的广播副本, 供 /ui/api/events SSE 订阅. 无订阅者时 send 返回 Err, 忽略即可.
    pub ui_events: tokio::sync::broadcast::Sender<UiEvent>,
    /// 「本机通行」密钥 (cc-router-tui 用). 每次启动重新生成, 只存在于内存和 runtime.json,
    /// 不进 settings / DB / 任何 DTO。读取方只有 proxy/server.rs (写文件) 与 proxy/web/ 的两个中间件。
    pub local_secret: Arc<String>,
}

impl AppState {
    pub async fn get_subscription(
        &self,
        id: &Uuid,
    ) -> Option<Arc<RwLock<SubscriptionRuntime>>> {
        let subs = self.subscriptions.read().await;
        subs.get(id).cloned()
    }

    /// 客户端工具应连接的 cc-router 本地代理 URL.
    /// 选择顺序: 实际绑定的 HTTP 端口 → 实际绑定的 HTTPS 端口 → settings.proxy_port (尚未起来时的 fallback).
    /// 单一来源, env_snippet / integrations / proxy_status 全部走它,避免 scheme 与端口漂移.
    pub async fn local_base_url(&self) -> String {
        let http_port = *self.http_bound_port.read().await;
        let https_port = *self.https_bound_port.read().await;
        if let Some(p) = http_port {
            format!("http://127.0.0.1:{p}")
        } else if let Some(p) = https_port {
            format!("https://127.0.0.1:{p}")
        } else {
            let configured = self.settings.read().await.proxy_port;
            format!("http://127.0.0.1:{configured}")
        }
    }
}
