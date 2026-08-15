//! 请求日志的「思考强度」三格 (客户端请求 / 实际发往上游 / 上游回显)。
//!
//! **硬约束**: 本模块所有函数拿不到值一律返回 `None`, 绝不 panic、绝不影响日志投递 ——
//! effort 只是观测信息, 解析失败不能让 `RequestLogEntry` 少写一条。
//!
//! 三个值的语义:
//! - `client`: 客户端 body 里表达的档位 (不含订阅槽位强制, 不含 yaml 默认)
//! - `effective`: cc-router 真正发往上游的档位
//! - `source`: `effective` 的来源 (`slot` / `client` / `yaml`)
//!
//! 上游回显 (`upstream_effort`) 只有 OpenAI Responses 系能拿到, 见
//! [`crate::proxy::transform::responses_common::ResponsesSseConverter::upstream_effort`]。
//!
//! **已知偏差 (`/v1/responses` 入站口, 不修)**: 该入口先把 OpenAI `reasoning.effort` 折成
//! Anthropic `thinking.budget_tokens`
//! ([`crate::proxy::transform::responses_inbound`] 的 `effort_to_budget_tokens`:
//! low=2048 / medium=8192 / high=16384), 而 `client_effort_of` 再用
//! [`ReasoningEffort::from_budget_tokens`] 的阈值 (<4096 minimal / <16384 low / <65536 medium)
//! 折回档位 —— 两张表不互逆, 于是 low/medium/high 各被降一档 (low→minimal, medium→low,
//! high→medium)。即这条入口的 `client_effort` 可能比客户端原文低一级。
//! 这是入站翻译既有的不对称, 两张映射表都被别处依赖, **刻意不改**;
//! `effective_effort` 取自真实 resolver, 不受影响。

use std::str::FromStr;

use serde_json::Value;

use crate::proxy::transform::openai::{
    output_config_effort, resolve_reasoning_effort, ReasoningEffort,
};

/// `effort_source` 的取值 — 与前端 `requestLogs.detail.effortSource.*` 的 key 对齐。
pub const SOURCE_SLOT: &str = "slot";
pub const SOURCE_CLIENT: &str = "client";
pub const SOURCE_YAML: &str = "yaml";

/// 一次 attempt 的思考强度三元组。全部可空 —— 任何一格拿不到就是 None。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffortLog {
    /// 客户端本次请求携带的档位
    pub client: Option<String>,
    /// cc-router 实际发往上游的档位
    pub effective: Option<String>,
    /// `effective` 的来源: slot / client / yaml
    pub source: Option<&'static str>,
}

/// 客户端请求 body 里表达的 effort (不含 slot 强制、不含 yaml 默认), 拿不到 → None。
///
/// 优先级与 [`resolve_reasoning_effort`] 的客户端层逐字一致:
/// `extra_body.reasoning_effort` → `output_config.effort` → `thinking.effort`
/// → `thinking.budget_tokens` 折算 ([`ReasoningEffort::from_budget_tokens`])。
///
/// 非法字符串按「缺失」继续往下找 (同 resolver), 全无 → None。
pub fn client_effort_of(body: &Value) -> Option<String> {
    if let Some(v) = body
        .get("extra_body")
        .and_then(|x| x.get("reasoning_effort"))
        .and_then(|x| x.as_str())
        .and_then(|s| ReasoningEffort::from_str(s).ok())
    {
        return Some(v.as_str().to_string());
    }
    if let Some(v) = output_config_effort(body).and_then(|s| ReasoningEffort::from_str(s).ok()) {
        return Some(v.as_str().to_string());
    }
    if let Some(thinking) = body.get("thinking") {
        if let Some(v) = thinking
            .get("effort")
            .and_then(|x| x.as_str())
            .and_then(|s| ReasoningEffort::from_str(s).ok())
        {
            return Some(v.as_str().to_string());
        }
        if let Some(bt) = thinking.get("budget_tokens").and_then(|x| x.as_u64()) {
            return Some(ReasoningEffort::from_budget_tokens(bt).as_str().to_string());
        }
    }
    None
}

/// `effective_effort` 的来源判定: slot > client > yaml > None。
///
/// `forced` / `yaml` 都要求「非空且能解析成合法档位」—— 脏数据 (空串 / 拼错的档位)
/// 在 [`resolve_reasoning_effort`] 里会被跳过, 这里同样跳过, 保证 source 与 effective 自洽。
pub fn effort_source(
    forced: Option<&str>,
    client: Option<&str>,
    yaml_default: Option<&str>,
) -> Option<&'static str> {
    if forced
        .filter(|s| !s.is_empty())
        .and_then(|s| ReasoningEffort::from_str(s).ok())
        .is_some()
    {
        return Some(SOURCE_SLOT);
    }
    if client.is_some() {
        return Some(SOURCE_CLIENT);
    }
    if yaml_default
        .filter(|s| !s.is_empty())
        .and_then(|s| ReasoningEffort::from_str(s).ok())
        .is_some()
    {
        return Some(SOURCE_YAML);
    }
    None
}

/// **OpenAI 家族**分支 (codex / openai responses / openai chat completions) 的 EffortLog。
///
/// `effective` 直接取 [`resolve_reasoning_effort`] 的结果 —— 这三条路径的翻译层自己调的就是
/// 同一个 resolver, 同源同参, 记下来的必然是真发出去的档位。
///
/// **Gemini 两条路径不能用本函数**: 它们分别走
/// [`crate::proxy::transform::gemini::resolve_thinking_budget`] 与
/// [`crate::proxy::transform::gemini_interactions::resolve_thinking_level`], 层序与
/// [`resolve_reasoning_effort`] 刻意不同 (见各自 doc), 同一个 body 可能算出不同档位。
/// 它们用 [`gemini_effort_log`] / [`gemini_interactions_effort_log`]。
pub fn translation_effort_log(
    body: &Value,
    forced: Option<&str>,
    yaml_default: Option<&str>,
) -> EffortLog {
    let client = client_effort_of(body);
    let effective =
        resolve_reasoning_effort(body, forced, yaml_default).map(|e| e.as_str().to_string());
    let source = effort_source(forced, client.as_deref(), yaml_default);
    EffortLog {
        client,
        effective,
        source,
    }
}

/// Gemini generateContent 分支的 EffortLog。
///
/// `budget` / `source` 必须来自
/// [`crate::proxy::transform::gemini::resolve_thinking_budget_with_source`] —— 即真正塞进
/// `generationConfig.thinkingConfig.thinkingBudget` 的那个值本身, 保证日志与线上实际一致。
///
/// `effective` 把整数预算折回档位语义 ([`ReasoningEffort::from_budget_tokens`]) 便于前端展示;
/// 负数预算 (`-1` = Gemini 的 dynamic 信号, 对应 `max`) 无法用无符号阈值表达, 记 None
/// 而不是瞎猜一个档位。
pub fn gemini_effort_log(
    body: &Value,
    budget: Option<i64>,
    source: Option<&'static str>,
) -> EffortLog {
    EffortLog {
        client: client_effort_of(body),
        // Gemini 用 -1 表示 dynamic thinking (effort_to_budget("max") 的编码), 其余负数不认识 → None
        effective: budget.and_then(|b| match b {
            -1 => Some("max".to_string()),
            b if b >= 0 => Some(
                ReasoningEffort::from_budget_tokens(b as u64)
                    .as_str()
                    .to_string(),
            ),
            _ => None,
        }),
        // effective 拿不到时 source 也没意义, 保持「effective 与 source 同 Some 同 None」
        source: source.filter(|_| budget.is_some_and(|b| b >= -1)),
    }
}

/// Gemini Interactions 分支的 EffortLog。
///
/// `level` / `source` 必须来自
/// [`crate::proxy::transform::gemini_interactions::resolve_thinking_level_with_source`] ——
/// 即真正塞进 `generation_config.thinking_level` 的那个值本身。level 本身就是档位字符串
/// (low / medium / high), 原样记入 `effective`。
pub fn gemini_interactions_effort_log(
    body: &Value,
    level: Option<&str>,
    source: Option<&'static str>,
) -> EffortLog {
    EffortLog {
        client: client_effort_of(body),
        effective: level.map(str::to_string),
        source,
    }
}

/// Anthropic 透传分支的 EffortLog。透传没有翻译层也没有 yaml 默认:
/// - [`crate::proxy::sanitize::force_anthropic_effort`] 真写进 body (返回值 > 0) → slot 生效
/// - 否则客户端的值原样出站 (若有)
pub fn passthrough_effort_log(
    client: Option<String>,
    forced: Option<&str>,
    effort_written: usize,
) -> EffortLog {
    if effort_written > 0 {
        if let Some(f) = forced.filter(|s| !s.is_empty()) {
            return EffortLog {
                client,
                effective: Some(f.to_string()),
                source: Some(SOURCE_SLOT),
            };
        }
    }
    let effective = client.clone();
    let source = effective.as_ref().map(|_| SOURCE_CLIENT);
    EffortLog {
        client,
        effective,
        source,
    }
}

/// Kiro (CodeWhisperer) 分支: 协议无 reasoning 字段, 只记客户端值。
pub fn kiro_effort_log(body: &Value) -> EffortLog {
    EffortLog {
        client: client_effort_of(body),
        effective: None,
        source: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn client_effort_reads_extra_body_first() {
        let body = json!({
            "extra_body": {"reasoning_effort": "max"},
            "output_config": {"effort": "low"},
            "thinking": {"effort": "medium", "budget_tokens": 100000}
        });
        assert_eq!(client_effort_of(&body), Some("max".to_string()));
    }

    #[test]
    fn client_effort_reads_output_config() {
        let body = json!({
            "output_config": {"effort": "xhigh"},
            "thinking": {"budget_tokens": 100000}
        });
        assert_eq!(client_effort_of(&body), Some("xhigh".to_string()));
    }

    #[test]
    fn client_effort_reads_thinking_effort() {
        let body = json!({"thinking": {"effort": "high"}});
        assert_eq!(client_effort_of(&body), Some("high".to_string()));
    }

    #[test]
    fn client_effort_maps_budget_tokens() {
        let body = json!({"thinking": {"budget_tokens": 20000}});
        assert_eq!(client_effort_of(&body), Some("medium".to_string()));
        let body = json!({"thinking": {"budget_tokens": 1024}});
        assert_eq!(client_effort_of(&body), Some("minimal".to_string()));
    }

    #[test]
    fn client_effort_none_when_absent() {
        assert_eq!(client_effort_of(&json!({"model": "x"})), None);
        assert_eq!(client_effort_of(&Value::Null), None);
    }

    #[test]
    fn client_effort_ignores_invalid_strings() {
        // 非法字符串视为缺失, 继续往下找; 全非法 → None
        let body = json!({"output_config": {"effort": "ultra"}, "thinking": {"effort": "bogus"}});
        assert_eq!(client_effort_of(&body), None);
        // 上层非法, 下层合法 → 取下层
        let body = json!({"extra_body": {"reasoning_effort": "nope"}, "thinking": {"effort": "low"}});
        assert_eq!(client_effort_of(&body), Some("low".to_string()));
    }

    #[test]
    fn effort_source_priority_slot_client_yaml_none() {
        assert_eq!(
            effort_source(Some("max"), Some("low"), Some("medium")),
            Some(SOURCE_SLOT)
        );
        assert_eq!(
            effort_source(None, Some("low"), Some("medium")),
            Some(SOURCE_CLIENT)
        );
        assert_eq!(effort_source(None, None, Some("medium")), Some(SOURCE_YAML));
        assert_eq!(effort_source(None, None, None), None);
    }

    #[test]
    fn effort_source_skips_dirty_slot_and_yaml_values() {
        // 空串 / 非法档位 都当没配 (与 resolve_reasoning_effort 一致)
        assert_eq!(effort_source(Some(""), None, None), None);
        assert_eq!(effort_source(Some("bogus"), None, None), None);
        assert_eq!(effort_source(None, None, Some("")), None);
        assert_eq!(effort_source(None, None, Some("bogus")), None);
        // slot 脏 → 落到 client
        assert_eq!(
            effort_source(Some("bogus"), Some("low"), None),
            Some(SOURCE_CLIENT)
        );
    }

    #[test]
    fn translation_log_slot_overrides_client() {
        let body = json!({"output_config": {"effort": "low"}});
        let log = translation_effort_log(&body, Some("max"), Some("medium"));
        assert_eq!(log.client, Some("low".to_string()));
        assert_eq!(log.effective, Some("max".to_string()));
        assert_eq!(log.source, Some(SOURCE_SLOT));
    }

    #[test]
    fn translation_log_falls_back_to_yaml() {
        let body = json!({"model": "x"});
        let log = translation_effort_log(&body, None, Some("medium"));
        assert_eq!(log.client, None);
        assert_eq!(log.effective, Some("medium".to_string()));
        assert_eq!(log.source, Some(SOURCE_YAML));
    }

    #[test]
    fn translation_log_all_empty() {
        let log = translation_effort_log(&json!({"model": "x"}), None, None);
        assert_eq!(log, EffortLog::default());
    }

    #[test]
    fn passthrough_log_slot_written() {
        let log = passthrough_effort_log(Some("low".into()), Some("max"), 2);
        assert_eq!(log.client, Some("low".to_string()));
        assert_eq!(log.effective, Some("max".to_string()));
        assert_eq!(log.source, Some(SOURCE_SLOT));
    }

    #[test]
    fn passthrough_log_slot_not_written_falls_back_to_client() {
        // 客户端 thinking.type=disabled 时 force_anthropic_effort 返回 0
        let log = passthrough_effort_log(Some("low".into()), Some("max"), 0);
        assert_eq!(log.effective, Some("low".to_string()));
        assert_eq!(log.source, Some(SOURCE_CLIENT));
    }

    #[test]
    fn passthrough_log_no_client_no_slot() {
        let log = passthrough_effort_log(None, None, 0);
        assert_eq!(log, EffortLog::default());
    }

    #[test]
    fn gemini_log_folds_budget_back_to_level() {
        let body = json!({"thinking": {"budget_tokens": 65536}});
        let log = gemini_effort_log(&body, Some(65536), Some(SOURCE_CLIENT));
        assert_eq!(log.client, Some("high".to_string()));
        assert_eq!(log.effective, Some("high".to_string()));
        assert_eq!(log.source, Some(SOURCE_CLIENT));
    }

    #[test]
    fn gemini_log_dynamic_budget_is_max_and_unknown_negative_is_none() {
        // -1 = Gemini dynamic thinking = effort_to_budget("max") 的编码 → 记 max
        let log = gemini_effort_log(&json!({"model": "x"}), Some(-1), Some(SOURCE_SLOT));
        assert_eq!(log.effective.as_deref(), Some("max"));
        assert_eq!(log.source, Some(SOURCE_SLOT));
        // 其他负数不认识 → effective/source 同为 None (不 panic)
        let log = gemini_effort_log(&json!({"model": "x"}), Some(-5), Some(SOURCE_CLIENT));
        assert_eq!(log.effective, None);
        assert_eq!(log.source, None);
        // 完全没解析出 budget 时也是 None
        let log = gemini_effort_log(&json!({"model": "x"}), None, None);
        assert_eq!(log, EffortLog::default());
    }

    #[test]
    fn gemini_interactions_log_records_level_verbatim() {
        let body = json!({"output_config": {"effort": "xhigh"}});
        let log = gemini_interactions_effort_log(&body, Some("high"), Some(SOURCE_CLIENT));
        // client 记客户端原文档位 (xhigh), effective 记真发出去的 level (high)
        assert_eq!(log.client, Some("xhigh".to_string()));
        assert_eq!(log.effective, Some("high".to_string()));
        assert_eq!(log.source, Some(SOURCE_CLIENT));
        assert_eq!(
            gemini_interactions_effort_log(&json!({"model": "x"}), None, None),
            EffortLog::default()
        );
    }

    #[test]
    fn kiro_log_records_client_only() {
        let log = kiro_effort_log(&json!({"thinking": {"effort": "high"}}));
        assert_eq!(log.client, Some("high".to_string()));
        assert_eq!(log.effective, None);
        assert_eq!(log.source, None);
    }
}
