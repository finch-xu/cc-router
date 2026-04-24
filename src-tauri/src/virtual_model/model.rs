use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum VirtualModelName {
    #[serde(rename = "model-opus")]
    Opus,
    #[serde(rename = "model-sonnet")]
    Sonnet,
    #[serde(rename = "model-haiku")]
    Haiku,
    /// 兜底：请求的 model 不是前三个虚拟名时走这里，透传原始 model 给订阅。
    #[serde(rename = "model-fallback")]
    Fallback,
}

impl VirtualModelName {
    pub fn as_str(&self) -> &'static str {
        match self {
            VirtualModelName::Opus => "model-opus",
            VirtualModelName::Sonnet => "model-sonnet",
            VirtualModelName::Haiku => "model-haiku",
            VirtualModelName::Fallback => "model-fallback",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "model-opus" => Some(Self::Opus),
            "model-sonnet" => Some(Self::Sonnet),
            "model-haiku" => Some(Self::Haiku),
            "model-fallback" => Some(Self::Fallback),
            _ => None,
        }
    }

    /// fallback 不对应任何 slot（透传模式）。如果调用者不检查就用，这里默认返回 sonnet
    /// 以保持类型签名不变，但正确的调用路径应该先判断 `is_fallback()`。
    pub fn slot(self) -> SubscriptionSlot {
        match self {
            VirtualModelName::Opus => SubscriptionSlot::Opus,
            VirtualModelName::Sonnet | VirtualModelName::Fallback => SubscriptionSlot::Sonnet,
            VirtualModelName::Haiku => SubscriptionSlot::Haiku,
        }
    }

    pub fn is_fallback(self) -> bool {
        matches!(self, VirtualModelName::Fallback)
    }

    pub fn all() -> [VirtualModelName; 4] {
        [Self::Opus, Self::Sonnet, Self::Haiku, Self::Fallback]
    }
}

#[derive(Debug, Clone, Copy)]
pub enum SubscriptionSlot {
    Opus,
    Sonnet,
    Haiku,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoutingMode {
    Sequential,
    RoundRobin,
}

#[derive(Debug, Clone)]
pub struct VirtualModelConfig {
    pub name: VirtualModelName,
    pub mode: RoutingMode,
    pub subscription_ids: Vec<Uuid>,
    /// 轮询模式专用，不持久化（§7.1）
    pub last_used_index: usize,
}
