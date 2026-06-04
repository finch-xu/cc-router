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

/// Strip trailing date suffix from Anthropic model IDs.
/// "claude-haiku-4-5-20251001" → "claude-haiku-4-5"
/// "claude-opus-4-7-20250606" → "claude-opus-4-7"
/// "model-opus" → "model-opus" (unchanged)
fn strip_date_suffix(s: &str) -> &str {
    if let Some((prefix, suffix)) = s.rsplit_once('-') {
        if suffix.len() == 8 && suffix.bytes().all(|b| b.is_ascii_digit()) {
            return prefix;
        }
    }
    s
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
        // LiteLLM-style 厂商前缀 (issue #5): anthropic/ 用于 Anthropic 兼容入口,
        // openai/ 用于 OpenAI Responses 兼容入口 (POST /v1/responses, v2.3+).
        let s = s.strip_prefix("anthropic/").unwrap_or(s);
        let s = s.strip_prefix("openai/").unwrap_or(s);
        // Anthropic 官方模型 ID 可能带日期后缀 (如 claude-haiku-4-5-20251001),
        // 先剥掉再匹配别名表.
        let s = strip_date_suffix(s);
        match s {
            "model-opus" | "claude-opus-4-7" | "gpt-5.5" => Some(Self::Opus),
            "model-sonnet" | "claude-sonnet-4-6" | "gpt-5.4" => Some(Self::Sonnet),
            "model-haiku" | "claude-haiku-4-5" | "gpt-5.4-mini" => Some(Self::Haiku),
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
        assert_eq!(VirtualModelName::parse("openai/unknown"), None);
        assert_eq!(VirtualModelName::parse("google/model-opus"), None);
        assert_eq!(VirtualModelName::parse("model-unknown"), None);
    }

    #[test]
    fn parse_strips_date_suffix() {
        assert_eq!(VirtualModelName::parse("claude-haiku-4-5-20251001"), Some(VirtualModelName::Haiku));
        assert_eq!(VirtualModelName::parse("claude-opus-4-7-20250606"), Some(VirtualModelName::Opus));
        assert_eq!(VirtualModelName::parse("claude-sonnet-4-6-20250514"), Some(VirtualModelName::Sonnet));
        // 带 anthropic/ 前缀 + 日期后缀
        assert_eq!(
            VirtualModelName::parse("anthropic/claude-haiku-4-5-20251001"),
            Some(VirtualModelName::Haiku)
        );
        assert_eq!(
            VirtualModelName::parse("anthropic/claude-opus-4-7-20250606"),
            Some(VirtualModelName::Opus)
        );
        // 带 openai/ 前缀 + 日期后缀
        assert_eq!(
            VirtualModelName::parse("openai/claude-sonnet-4-6-20250514"),
            Some(VirtualModelName::Sonnet)
        );
        // 虚拟模型名本身无日期后缀, 不受影响
        assert_eq!(VirtualModelName::parse("model-opus"), Some(VirtualModelName::Opus));
    }

    #[test]
    fn strip_date_suffix_works() {
        assert_eq!(strip_date_suffix("claude-haiku-4-5-20251001"), "claude-haiku-4-5");
        assert_eq!(strip_date_suffix("claude-opus-4-7-20250606"), "claude-opus-4-7");
        assert_eq!(strip_date_suffix("model-opus"), "model-opus");
        // 非 8 位数字后缀不剥
        assert_eq!(strip_date_suffix("claude-haiku-4-5-v2"), "claude-haiku-4-5-v2");
        assert_eq!(strip_date_suffix("gpt-5.5"), "gpt-5.5");
    }

    #[test]
    fn parse_recognizes_openai_responses_aliases() {
        // 无前缀
        assert_eq!(VirtualModelName::parse("gpt-5.5"), Some(VirtualModelName::Opus));
        assert_eq!(VirtualModelName::parse("gpt-5.4"), Some(VirtualModelName::Sonnet));
        assert_eq!(VirtualModelName::parse("gpt-5.4-mini"), Some(VirtualModelName::Haiku));
        // openai/ 前缀
        assert_eq!(VirtualModelName::parse("openai/gpt-5.5"), Some(VirtualModelName::Opus));
        assert_eq!(VirtualModelName::parse("openai/gpt-5.4"), Some(VirtualModelName::Sonnet));
        assert_eq!(VirtualModelName::parse("openai/gpt-5.4-mini"), Some(VirtualModelName::Haiku));
        // 交叉前缀 (openai/ 把 model- 别名也带过来) - 仍然能 parse, 因为 openai/ 只是被 strip 掉
        assert_eq!(VirtualModelName::parse("openai/model-sonnet"), Some(VirtualModelName::Sonnet));
    }
}
