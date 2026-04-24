use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::virtual_model::model::SubscriptionSlot;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSlots {
    pub opus: String,
    pub sonnet: String,
    pub haiku: String,
}

impl ModelSlots {
    pub fn get(&self, slot: SubscriptionSlot) -> &str {
        match slot {
            SubscriptionSlot::Opus => &self.opus,
            SubscriptionSlot::Sonnet => &self.sonnet,
            SubscriptionSlot::Haiku => &self.haiku,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// 订阅的持久化字段。对应 `subscriptions` 表。
/// `api_key` 明文存储——类比 Claude Code 的 settings.json 做法，安全边界由 OS 进程隔离提供。
#[derive(Debug, Clone)]
pub struct SubscriptionRow {
    pub id: Uuid,
    pub provider_id: String,
    pub endpoint_id: String,
    pub display_name: String,
    pub api_key: String,
    pub model_slots: ModelSlots,
    pub enabled: bool,
    pub is_auth_failed: bool,
    pub last_error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 订阅健康状态枚举（设计稿 §6.1）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionState {
    Healthy,
    RateLimited,
    QuotaExhausted,
    TransientError,
    AuthFailed,
    Disabled,
}

impl SubscriptionState {
    pub fn is_dispatchable(self) -> bool {
        matches!(self, SubscriptionState::Healthy)
    }
}

/// 订阅的全量运行时视图（持久化字段 + 运行时状态）。
/// 使用 `Arc<RwLock<SubscriptionRuntime>>` 放入 `AppState`，单订阅一把锁（§6.4）。
#[derive(Debug, Clone)]
pub struct SubscriptionRuntime {
    pub row: SubscriptionRow,
    pub state: SubscriptionState,
    pub state_entered_at: DateTime<Utc>,
    pub cooldown_until: Option<DateTime<Utc>>,
    pub consecutive_errors: u32,
    pub transient_error_level: u32,
    pub last_error_message: Option<String>,
    pub model_cache: Option<ModelCache>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCache {
    pub fetched_at: DateTime<Utc>,
    pub models: Vec<ModelInfo>,
}

impl SubscriptionRuntime {
    pub fn from_row(row: SubscriptionRow) -> Self {
        let state = if !row.enabled {
            SubscriptionState::Disabled
        } else if row.is_auth_failed {
            SubscriptionState::AuthFailed
        } else {
            SubscriptionState::Healthy
        };
        let last_error_message = row.last_error_message.clone();
        Self {
            row,
            state,
            state_entered_at: Utc::now(),
            cooldown_until: None,
            consecutive_errors: 0,
            transient_error_level: 0,
            last_error_message,
            model_cache: None,
        }
    }

    pub fn is_dispatchable(&self, now: DateTime<Utc>) -> bool {
        if !self.row.enabled {
            return false;
        }
        if !self.state.is_dispatchable() {
            return false;
        }
        if let Some(until) = self.cooldown_until {
            if until > now {
                return false;
            }
        }
        true
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionDto {
    pub id: Uuid,
    pub provider_id: String,
    pub endpoint_id: String,
    pub display_name: String,
    pub model_slots: ModelSlots,
    pub enabled: bool,
    pub state: SubscriptionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_until: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error_message: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub referenced_by: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_cache: Option<ModelCacheDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCacheDto {
    pub fetched_at: i64,
    pub models: Vec<ModelInfo>,
}

impl SubscriptionDto {
    pub fn from_runtime(rt: &SubscriptionRuntime, referenced_by: Vec<String>) -> Self {
        Self {
            id: rt.row.id,
            provider_id: rt.row.provider_id.clone(),
            endpoint_id: rt.row.endpoint_id.clone(),
            display_name: rt.row.display_name.clone(),
            model_slots: rt.row.model_slots.clone(),
            enabled: rt.row.enabled,
            state: rt.state,
            cooldown_until: rt.cooldown_until.map(|t| t.timestamp_millis()),
            last_error_message: rt.last_error_message.clone(),
            created_at: rt.row.created_at.timestamp_millis(),
            updated_at: rt.row.updated_at.timestamp_millis(),
            referenced_by,
            model_cache: rt.model_cache.as_ref().map(|c| ModelCacheDto {
                fetched_at: c.fetched_at.timestamp_millis(),
                models: c.models.clone(),
            }),
        }
    }
}
