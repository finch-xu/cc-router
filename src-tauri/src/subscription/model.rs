use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::provider::model::{join_base_path, AuthHeaderFormat, ModelDiscovery};
#[cfg(test)]
use crate::provider::model::ANTHROPIC_THINKING_FIELD;
use crate::virtual_model::model::SubscriptionSlot;

/// 自定义订阅的来源标记常量。
pub const CUSTOM_SOURCE_MARKER: &str = "__custom__";

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
///
/// snapshot 模型: 创建订阅时把 yaml 模板的连接信息全部拷贝下来,
/// pipeline 运行时只读 row 自己的字段, 不再反查 state.providers.
/// `api_key` 明文存储——类比 Claude Code 的 settings.json 做法。
#[derive(Debug, Clone)]
pub struct SubscriptionRow {
    pub id: Uuid,
    /// 来源标记: 内置 yaml id 或 `CUSTOM_SOURCE_MARKER`
    pub provider_id: String,
    /// 来源 endpoint id, 自定义订阅写 `CUSTOM_SOURCE_MARKER`
    pub endpoint_id: String,
    pub display_name: String,
    pub api_key: String,
    pub model_slots: ModelSlots,
    pub enabled: bool,
    pub is_auth_failed: bool,
    pub last_error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,

    pub base_url: String,
    pub messages_path: String,
    pub auth_header_name: String,
    pub auth_header_format: AuthHeaderFormat,
    pub required_headers: BTreeMap<String, String>,
    pub forward_headers: Vec<String>,
    pub model_discovery: ModelDiscovery,

    pub provider_display_name: String,
    pub provider_icon: String,
    pub is_user_defined: bool,
    /// 是否支持 Anthropic extended thinking 块。
    /// 创建订阅时从 provider yaml 的 capabilities.supports_thinking_blocks 拷贝默认值,
    /// 用户可在 UI 上覆盖。pipeline 转发前如果为 false,会剥离请求体里的 thinking 字段和块。
    pub supports_thinking_blocks: bool,
    /// thinking 块内部承载字段名 (Anthropic 标准 "thinking"; DeepSeek 兼容层用 "think")。
    /// 创建订阅时从 provider yaml 的 capabilities.thinking_block_field_name 拷贝。
    /// 仅当 supports_thinking_blocks=true 且值非 "thinking" 时, pipeline 在请求侧
    /// 把 thinking→自定义、响应侧把自定义→thinking 翻译, 让 CC 始终以 Anthropic 标准收发。
    pub thinking_block_field_name: String,
}

impl SubscriptionRow {
    pub fn messages_url(&self) -> String {
        join_base_path(&self.base_url, &self.messages_path)
    }

    pub fn auth_header_value(&self) -> String {
        self.auth_header_format.apply(&self.api_key)
    }
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

    pub base_url: String,
    pub messages_path: String,
    pub auth_header_name: String,
    pub auth_header_format: AuthHeaderFormat,
    pub required_headers: BTreeMap<String, String>,
    pub forward_headers: Vec<String>,
    pub model_discovery: ModelDiscovery,
    pub provider_display_name: String,
    pub provider_icon: String,
    pub is_user_defined: bool,
    pub supports_thinking_blocks: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCacheDto {
    pub fetched_at: i64,
    pub models: Vec<ModelInfo>,
}

#[cfg(test)]
impl SubscriptionRow {
    /// 测试用 fixture: 生成一个连接信息全空但字段齐全的 row, 调用方可后续覆盖关心的字段。
    pub fn test_fixture(provider_id: &str, endpoint_id: &str) -> Self {
        Self {
            id: Uuid::new_v4(),
            provider_id: provider_id.into(),
            endpoint_id: endpoint_id.into(),
            display_name: "test".into(),
            api_key: "test-key".into(),
            model_slots: ModelSlots {
                opus: "a".into(),
                sonnet: "b".into(),
                haiku: "c".into(),
            },
            enabled: true,
            is_auth_failed: false,
            last_error_message: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            base_url: String::new(),
            messages_path: String::new(),
            auth_header_name: String::new(),
            auth_header_format: AuthHeaderFormat::Bearer,
            required_headers: BTreeMap::new(),
            forward_headers: Vec::new(),
            model_discovery: ModelDiscovery::default(),
            provider_display_name: String::new(),
            provider_icon: String::new(),
            is_user_defined: false,
            supports_thinking_blocks: false,
            thinking_block_field_name: ANTHROPIC_THINKING_FIELD.to_string(),
        }
    }
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
            base_url: rt.row.base_url.clone(),
            messages_path: rt.row.messages_path.clone(),
            auth_header_name: rt.row.auth_header_name.clone(),
            auth_header_format: rt.row.auth_header_format.clone(),
            required_headers: rt.row.required_headers.clone(),
            forward_headers: rt.row.forward_headers.clone(),
            model_discovery: rt.row.model_discovery.clone(),
            provider_display_name: rt.row.provider_display_name.clone(),
            provider_icon: rt.row.provider_icon.clone(),
            is_user_defined: rt.row.is_user_defined,
            supports_thinking_blocks: rt.row.supports_thinking_blocks,
        }
    }
}
