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
        // Accept LiteLLM-style `anthropic/<model-name>` prefix (issue #5).
        let s = s.strip_prefix("anthropic/").unwrap_or(s);
        match s {
            "model-opus" | "claude-opus-4-7" => Some(Self::Opus),
            "model-sonnet" | "claude-sonnet-4-6" => Some(Self::Sonnet),
            "model-haiku" | "claude-haiku-4-5" => Some(Self::Haiku),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_strips_anthropic_prefix() {
        assert_eq!(VirtualModelName::parse("anthropic/model-opus"), Some(VirtualModelName::Opus));
        assert_eq!(VirtualModelName::parse("anthropic/model-sonnet"), Some(VirtualModelName::Sonnet));
        assert_eq!(VirtualModelName::parse("anthropic/model-haiku"), Some(VirtualModelName::Haiku));
        assert_eq!(VirtualModelName::parse("anthropic/model-fallback"), Some(VirtualModelName::Fallback));
    }

    #[test]
    fn parse_without_prefix_still_works() {
        assert_eq!(VirtualModelName::parse("model-opus"), Some(VirtualModelName::Opus));
        assert_eq!(VirtualModelName::parse("model-sonnet"), Some(VirtualModelName::Sonnet));
        assert_eq!(VirtualModelName::parse("model-haiku"), Some(VirtualModelName::Haiku));
        assert_eq!(VirtualModelName::parse("model-fallback"), Some(VirtualModelName::Fallback));
    }

    #[test]
    fn parse_recognizes_versioned_model_aliases() {
        // 无前缀
        assert_eq!(VirtualModelName::parse("claude-opus-4-7"), Some(VirtualModelName::Opus));
        assert_eq!(VirtualModelName::parse("claude-sonnet-4-6"), Some(VirtualModelName::Sonnet));
        assert_eq!(VirtualModelName::parse("claude-haiku-4-5"), Some(VirtualModelName::Haiku));
        // anthropic/ 前缀
        assert_eq!(
            VirtualModelName::parse("anthropic/claude-opus-4-7"),
            Some(VirtualModelName::Opus)
        );
        assert_eq!(
            VirtualModelName::parse("anthropic/claude-sonnet-4-6"),
            Some(VirtualModelName::Sonnet)
        );
        assert_eq!(
            VirtualModelName::parse("anthropic/claude-haiku-4-5"),
            Some(VirtualModelName::Haiku)
        );
    }

    #[test]
    fn parse_unknown_returns_none() {
        assert_eq!(VirtualModelName::parse("anthropic/unknown"), None);
        // Only `anthropic/` is stripped; other vendor prefixes pass through unchanged.
        assert_eq!(VirtualModelName::parse("openai/model-opus"), None);
        assert_eq!(VirtualModelName::parse("model-unknown"), None);
    }
}
