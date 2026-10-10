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
    /// Jev 决策模型 (System One 协议): 只服务 `POST /v1/systemone`, 只绑端点协议为 systemone 的订阅。
    /// 与另外五个互相隔离 (见 [`VirtualModelName::accepts`])。
    #[serde(rename = "model-jev")]
    Jev,
}

impl VirtualModelName {
    pub fn as_str(&self) -> &'static str {
        match self {
            VirtualModelName::Fable => "model-fable",
            VirtualModelName::Opus => "model-opus",
            VirtualModelName::Sonnet => "model-sonnet",
            VirtualModelName::Haiku => "model-haiku",
            VirtualModelName::Fallback => "model-fallback",
            VirtualModelName::Jev => "model-jev",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        // LiteLLM-style 厂商前缀 (issue #5): anthropic/ 用于 Anthropic 兼容入口,
        // openai/ 用于 OpenAI Responses 兼容入口 (POST /v1/responses, v2.3+).
        let s = s.strip_prefix("anthropic/").unwrap_or(s);
        let s = s.strip_prefix("openai/").unwrap_or(s);

        // 虚拟模型名: 精确映射.
        match s {
            "model-fable" => return Some(Self::Fable),
            "model-opus" => return Some(Self::Opus),
            "model-sonnet" => return Some(Self::Sonnet),
            "model-haiku" => return Some(Self::Haiku),
            "model-fallback" => return Some(Self::Fallback),
            "model-jev" => return Some(Self::Jev),
            _ => {}
        }

        // ChatGPT 四档命名模糊匹配: gpt-*-astra / gpt-*-sol / gpt-*-terra / gpt-*-luna,
        // 与 fable / opus / sonnet / haiku 一一对应, 含其他版本号与日期后缀变种
        // (gpt-6.1-sol / gpt-6-astra-20261201)。按 '-' 分段精确比对档位段, 避免
        // "gpt-6-solaris" 前缀撞名误命中。只认档位名: 版本号不是档位, 纯版本号
        // (gpt-5.5) 与 mini 后缀都不映射, 落 fallback。
        if s.starts_with("gpt-") {
            for seg in s.split('-') {
                match seg {
                    "astra" => return Some(Self::Fable),
                    "sol" => return Some(Self::Opus),
                    "terra" => return Some(Self::Sonnet),
                    "luna" => return Some(Self::Haiku),
                    _ => {}
                }
            }
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

    /// fallback / jev 不对应任何四槽 slot。为保持签名这里返回 sonnet, 正确的调用路径
    /// 应该先判断 `is_fallback()` / `is_jev()`。
    pub fn slot(self) -> SubscriptionSlot {
        match self {
            VirtualModelName::Fable => SubscriptionSlot::Fable,
            VirtualModelName::Opus => SubscriptionSlot::Opus,
            VirtualModelName::Sonnet | VirtualModelName::Fallback | VirtualModelName::Jev => {
                SubscriptionSlot::Sonnet
            }
            VirtualModelName::Haiku => SubscriptionSlot::Haiku,
        }
    }

    pub fn is_fallback(self) -> bool {
        matches!(self, VirtualModelName::Fallback)
    }

    pub fn is_jev(self) -> bool {
        matches!(self, VirtualModelName::Jev)
    }

    /// 绑定隔离: model-jev 只收 systemone 订阅, 其余五个只收对话类 (messages) 订阅。
    /// 写入 (绑定命令)、导入、运行时三处都用它判断。
    pub fn accepts(self, protocol: crate::provider::model::EndpointProtocol) -> bool {
        use crate::provider::model::EndpointProtocol;
        match protocol {
            EndpointProtocol::Systemone => self.is_jev(),
            EndpointProtocol::Messages => !self.is_jev(),
        }
    }

    /// 不能绑定时给出原因 (中文, 直接上屏)。能绑定返回 None。
    pub fn binding_rejection(
        self,
        protocol: crate::provider::model::EndpointProtocol,
        sub_name: &str,
    ) -> Option<String> {
        if self.accepts(protocol) {
            return None;
        }
        Some(if self.is_jev() {
            format!("订阅「{sub_name}」不是 System One 端点, 不能绑定到 model-jev")
        } else {
            format!("订阅「{sub_name}」是 System One 端点, 只能绑定到 model-jev")
        })
    }

    pub fn all() -> [VirtualModelName; 6] {
        [
            Self::Fable,
            Self::Opus,
            Self::Sonnet,
            Self::Haiku,
            Self::Fallback,
            Self::Jev,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// 会话亲和: 同一会话钉住同一订阅, 新会话按轮询分配 (见 proxy::session_key / virtual_model::affinity)
    Sticky,
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
    fn parse_strips_openai_prefix() {
        assert_eq!(VirtualModelName::parse("openai/gpt-6-astra"), Some(VirtualModelName::Fable));
        assert_eq!(VirtualModelName::parse("openai/gpt-6-sol"), Some(VirtualModelName::Opus));
        assert_eq!(VirtualModelName::parse("openai/gpt-6-terra"), Some(VirtualModelName::Sonnet));
        assert_eq!(VirtualModelName::parse("openai/gpt-6-luna"), Some(VirtualModelName::Haiku));
        // 交叉前缀 (openai/ 把 model- 别名也带过来) - 仍然能 parse, 因为 openai/ 只是被 strip 掉
        assert_eq!(VirtualModelName::parse("openai/model-sonnet"), Some(VirtualModelName::Sonnet));
    }

    #[test]
    fn parse_recognizes_gpt_tier_suffixes() {
        // ChatGPT 四档命名, 与 fable / opus / sonnet / haiku 一一对应
        assert_eq!(VirtualModelName::parse("gpt-6-astra"), Some(VirtualModelName::Fable));
        assert_eq!(VirtualModelName::parse("gpt-6-sol"), Some(VirtualModelName::Opus));
        assert_eq!(VirtualModelName::parse("gpt-6-terra"), Some(VirtualModelName::Sonnet));
        assert_eq!(VirtualModelName::parse("gpt-6-luna"), Some(VirtualModelName::Haiku));
        // 档位只看名字不看代际: 5.6 代的旗舰 sol 同样落 opus
        assert_eq!(VirtualModelName::parse("gpt-5.6-sol"), Some(VirtualModelName::Opus));
        assert_eq!(VirtualModelName::parse("gpt-5.6-terra"), Some(VirtualModelName::Sonnet));
        assert_eq!(VirtualModelName::parse("gpt-5.6-luna"), Some(VirtualModelName::Haiku));
        // 其他版本号 / 日期后缀变种
        assert_eq!(VirtualModelName::parse("gpt-6.1-sol"), Some(VirtualModelName::Opus));
        assert_eq!(
            VirtualModelName::parse("gpt-6-astra-20261201"),
            Some(VirtualModelName::Fable)
        );
        // 边界: 档位段必须整段相等, 前缀撞名不误命中
        assert_eq!(VirtualModelName::parse("gpt-6-solaris"), None);
        assert_eq!(VirtualModelName::parse("gpt-6-astral"), None);
        // 边界: 非 gpt- 开头不进模糊匹配
        assert_eq!(VirtualModelName::parse("mistral-sol-7b"), None);
        // 边界: 未知厂商前缀不被 strip → 不以 gpt- 开头 → fallback(None)
        assert_eq!(VirtualModelName::parse("google/gpt-6-sol"), None);
    }

    #[test]
    fn parse_rejects_gpt_names_without_a_tier() {
        // 版本号不是档位: 纯版本号与 mini 后缀都不映射, 落 fallback
        for name in ["gpt-5.6", "gpt-5.5", "gpt-5.4", "gpt-6", "gpt-5.4-mini", "gpt-6-mini"] {
            assert_eq!(VirtualModelName::parse(name), None, "{name}");
            assert_eq!(VirtualModelName::parse(&format!("openai/{name}")), None, "openai/{name}");
        }
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

    #[test]
    fn jev_round_trips_and_is_the_sixth_model() {
        assert_eq!(VirtualModelName::parse("model-jev"), Some(VirtualModelName::Jev));
        assert_eq!(VirtualModelName::Jev.as_str(), "model-jev");
        assert_eq!(serde_json::to_string(&VirtualModelName::Jev).unwrap(), "\"model-jev\"");
        assert_eq!(VirtualModelName::all().len(), 6);
        assert!(VirtualModelName::Jev.is_jev() && !VirtualModelName::Jev.is_fallback());
        // jev-latest 这类客户端模型名不能被识别成 jev (路由由路径决定, 不由 body 决定)
        assert_eq!(VirtualModelName::parse("jev-latest"), None);
    }

    #[test]
    fn jev_only_accepts_systemone_and_others_only_accept_messages() {
        use crate::provider::model::EndpointProtocol::{Messages, Systemone};
        assert!(VirtualModelName::Jev.accepts(Systemone));
        assert!(!VirtualModelName::Jev.accepts(Messages));
        for vm in VirtualModelName::all().into_iter().filter(|v| !v.is_jev()) {
            assert!(vm.accepts(Messages), "{vm:?}");
            assert!(!vm.accepts(Systemone), "{vm:?}");
        }
        assert!(VirtualModelName::Jev.binding_rejection(Systemone, "a").is_none());
        assert!(VirtualModelName::Jev.binding_rejection(Messages, "智谱").unwrap().contains("智谱"));
        assert!(VirtualModelName::Opus.binding_rejection(Systemone, "x").unwrap().contains("model-jev"));
    }
}
