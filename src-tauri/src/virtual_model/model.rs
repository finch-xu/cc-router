use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum VirtualModelName {
    /// 最强模型 (Claude Fable 5), 对应 ANTHROPIC_DEFAULT_FABLE_MODEL, 用于最难/最长任务。
    #[serde(rename = "model-fable")]
    Fable,
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
            VirtualModelName::Fable => "model-fable",
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

        // 虚拟模型名 + OpenAI Responses 别名: 这些不以 claude- 开头, 精确映射.
        // gpt-5.6 给 fable: fable 能力在 opus(gpt-5.5) 之上, 故取更高档号.
        match s {
            "model-fable" | "gpt-5.6" => return Some(Self::Fable),
            "model-opus" | "gpt-5.5" => return Some(Self::Opus),
            "model-sonnet" | "gpt-5.4" => return Some(Self::Sonnet),
            "model-haiku" | "gpt-5.4-mini" => return Some(Self::Haiku),
            "model-fallback" => return Some(Self::Fallback),
            _ => {}
        }

        // 官方 Anthropic 模型写法及其变种 (含日期后缀): 前缀匹配 (issue #22).
        // claude-opus-4-7 / claude-opus-4-7-20250606 / claude-opus-4-1-... 等都归位,
        // 不必再逐个枚举版本号. 未知厂商前缀 (如 google/claude-opus-...) 不以 claude-
        // 开头, 自动落 fallback, 保留 issue #5 的厂商路由语义.
        if s.starts_with("claude-fable") {
            Some(Self::Fable)
        } else if s.starts_with("claude-opus") {
            Some(Self::Opus)
        } else if s.starts_with("claude-sonnet") {
            Some(Self::Sonnet)
        } else if s.starts_with("claude-haiku") {
            Some(Self::Haiku)
        } else {
            None
        }
    }

    /// fallback 不对应任何 slot（透传模式）。如果调用者不检查就用，这里默认返回 sonnet
    /// 以保持类型签名不变，但正确的调用路径应该先判断 `is_fallback()`。
    pub fn slot(self) -> SubscriptionSlot {
        match self {
            VirtualModelName::Fable => SubscriptionSlot::Fable,
            VirtualModelName::Opus => SubscriptionSlot::Opus,
            VirtualModelName::Sonnet | VirtualModelName::Fallback => SubscriptionSlot::Sonnet,
            VirtualModelName::Haiku => SubscriptionSlot::Haiku,
        }
    }

    pub fn is_fallback(self) -> bool {
        matches!(self, VirtualModelName::Fallback)
    }

    pub fn all() -> [VirtualModelName; 5] {
        [
            Self::Fable,
            Self::Opus,
            Self::Sonnet,
            Self::Haiku,
            Self::Fallback,
        ]
    }
}

#[derive(Debug, Clone, Copy)]
pub enum SubscriptionSlot {
    Fable,
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
        assert_eq!(VirtualModelName::parse("anthropic/model-fable"), Some(VirtualModelName::Fable));
        assert_eq!(VirtualModelName::parse("anthropic/model-opus"), Some(VirtualModelName::Opus));
        assert_eq!(VirtualModelName::parse("anthropic/model-sonnet"), Some(VirtualModelName::Sonnet));
        assert_eq!(VirtualModelName::parse("anthropic/model-haiku"), Some(VirtualModelName::Haiku));
        assert_eq!(VirtualModelName::parse("anthropic/model-fallback"), Some(VirtualModelName::Fallback));
    }

    #[test]
    fn parse_without_prefix_still_works() {
        assert_eq!(VirtualModelName::parse("model-fable"), Some(VirtualModelName::Fable));
        assert_eq!(VirtualModelName::parse("model-opus"), Some(VirtualModelName::Opus));
        assert_eq!(VirtualModelName::parse("model-sonnet"), Some(VirtualModelName::Sonnet));
        assert_eq!(VirtualModelName::parse("model-haiku"), Some(VirtualModelName::Haiku));
        assert_eq!(VirtualModelName::parse("model-fallback"), Some(VirtualModelName::Fallback));
    }

    #[test]
    fn parse_recognizes_versioned_model_aliases() {
        // 无前缀
        assert_eq!(VirtualModelName::parse("claude-fable-5"), Some(VirtualModelName::Fable));
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
    fn parse_recognizes_openai_responses_aliases() {
        // 无前缀
        assert_eq!(VirtualModelName::parse("gpt-5.6"), Some(VirtualModelName::Fable));
        assert_eq!(VirtualModelName::parse("gpt-5.5"), Some(VirtualModelName::Opus));
        assert_eq!(VirtualModelName::parse("gpt-5.4"), Some(VirtualModelName::Sonnet));
        assert_eq!(VirtualModelName::parse("gpt-5.4-mini"), Some(VirtualModelName::Haiku));
        // openai/ 前缀
        assert_eq!(VirtualModelName::parse("openai/gpt-5.6"), Some(VirtualModelName::Fable));
        assert_eq!(VirtualModelName::parse("openai/gpt-5.5"), Some(VirtualModelName::Opus));
        assert_eq!(VirtualModelName::parse("openai/gpt-5.4"), Some(VirtualModelName::Sonnet));
        assert_eq!(VirtualModelName::parse("openai/gpt-5.4-mini"), Some(VirtualModelName::Haiku));
        // 交叉前缀 (openai/ 把 model- 别名也带过来) - 仍然能 parse, 因为 openai/ 只是被 strip 掉
        assert_eq!(VirtualModelName::parse("openai/model-sonnet"), Some(VirtualModelName::Sonnet));
    }

    #[test]
    fn parse_matches_official_model_variants() {
        // issue #22: 带日期后缀的官方 ID 被前缀匹配兜住
        assert_eq!(VirtualModelName::parse("claude-fable-5-20260101"), Some(VirtualModelName::Fable));
        assert_eq!(VirtualModelName::parse("claude-haiku-4-5-20251001"), Some(VirtualModelName::Haiku));
        assert_eq!(VirtualModelName::parse("claude-opus-4-7-20250606"), Some(VirtualModelName::Opus));
        assert_eq!(VirtualModelName::parse("claude-sonnet-4-6-20250514"), Some(VirtualModelName::Sonnet));
        // 未来 / 其他版本号同样命中, 无需枚举
        assert_eq!(VirtualModelName::parse("claude-opus-4-1-20250805"), Some(VirtualModelName::Opus));
        // anthropic/ 前缀 + 日期后缀
        assert_eq!(
            VirtualModelName::parse("anthropic/claude-haiku-4-5-20251001"),
            Some(VirtualModelName::Haiku)
        );
        // 边界: 未知厂商前缀不以 claude- 开头 → fallback(None), 不被前缀匹配误捕
        assert_eq!(VirtualModelName::parse("google/claude-opus-4-7"), None);
        // 边界: 非 claude 家族 → fallback(None)
        assert_eq!(VirtualModelName::parse("deepseek-chat"), None);
        assert_eq!(VirtualModelName::parse("gpt-4o"), None);
    }
}
