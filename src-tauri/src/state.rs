use std::collections::HashMap;
use std::sync::Arc;

use tauri::AppHandle;
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

use crate::observability::events::EventEntry;
use crate::observability::request_log::RequestLogEntry;
use crate::provider::model::Provider;
use crate::settings::model::Settings;
use crate::subscription::model::SubscriptionRuntime;
use crate::virtual_model::model::{VirtualModelConfig, VirtualModelName};

/// 全局 app 状态。所有 Tauri commands、代理服务、后台任务都共享这份状态。
#[derive(Clone)]
pub struct AppState {
    pub db: sqlx::SqlitePool,
    pub providers: Arc<HashMap<String, Provider>>,
    pub subscriptions: Arc<RwLock<HashMap<Uuid, Arc<RwLock<SubscriptionRuntime>>>>>,
    pub virtual_models: Arc<RwLock<HashMap<VirtualModelName, VirtualModelConfig>>>,
    pub settings: Arc<RwLock<Settings>>,
    pub proxy_port: Arc<RwLock<u16>>,
    pub request_log_tx: mpsc::Sender<RequestLogEntry>,
    pub event_log_tx: mpsc::Sender<EventEntry>,
    pub http_client: reqwest::Client,
    /// 短超时(30s) 单例, 仅用于订阅可达性探测(测试连接 + 后台巡检)。
    /// 与 `http_client` 的 600s 上限分离: 探测期望快速判定, 慢响应等同失败。
    pub probe_client: reqwest::Client,
    pub app_handle: AppHandle,
}

impl AppState {
    pub async fn get_subscription(
        &self,
        id: &Uuid,
    ) -> Option<Arc<RwLock<SubscriptionRuntime>>> {
        let subs = self.subscriptions.read().await;
        subs.get(id).cloned()
    }
}
