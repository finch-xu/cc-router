//! 订阅可达性探测 helper。
//!
//! 用最小 prompt 真实打 messages 端点验证连接,被 `commands::test_connection` 和
//! `recheck_worker` 共用。
//!
//! 设计:
//! - 不打 /models — 避免 URL 拼接歧义和 enabled:false provider 无法测的限制
//! - **与生产请求路径完全等价**:`probe_subscription` 按 `auth_type` 分流到真实管线同款的
//!   `dispatch_*_attempt` 函数(OAuth 刷 token / Gemini 替 `{model}` / 各家协议翻译全复用),
//!   只看返回的 `Ok`(连通) / `Err`(失败带 status+message)。纯 Anthropic 透传家族(`ApiKey`)
//!   仍走轻量 `probe`(直接打 messages 端点)。
//! - 失败原因都编码到 ProbeResult.message, 不返回 Result(调用方逻辑简单)
//!
//! snapshot 模型: 不再需要 provider/endpoint, 全部连接信息从订阅 row 自身字段读。

use axum::http::HeaderMap;
use reqwest::header::{
    HeaderMap as ReqHeaderMap, HeaderName as ReqHeaderName, HeaderValue as ReqHeaderValue,
    CONTENT_TYPE,
};
use serde_json::Value;
use uuid::Uuid;

use crate::provider::model::AuthType;
use crate::proxy::gemini_dispatch::dispatch_gemini_attempt;
use crate::proxy::gemini_interactions_dispatch::dispatch_gemini_interactions_attempt;
use crate::proxy::oauth_dispatch::{
    dispatch_kiro_attempt, dispatch_oauth_attempt, OAuthDispatchError,
};
use crate::proxy::openai_chat_completions_dispatch::dispatch_openai_chat_completions_attempt;
use crate::proxy::openai_responses_dispatch::dispatch_openai_responses_attempt;
use crate::proxy::pipeline::provider_reasoning_defaults;
use crate::proxy::transform::gemini::{resolve_thinking_budget, GeminiExtras};
use crate::proxy::transform::gemini_interactions::{resolve_thinking_level, InteractionsExtras};
use crate::proxy::transform::openai::{resolve_reasoning_effort, OpenAiResponsesExtras};
use crate::proxy::transform::openai_chat_completions::ChatCompletionsExtras;
use crate::proxy::transform::openai_responses::CodexExtras;
use crate::state::AppState;
use crate::subscription::model::SubscriptionRow;
use crate::virtual_model::SubscriptionSlot;

#[derive(Debug)]
pub struct ProbeResult {
    pub ok: bool,
    pub http_status: Option<u16>,
    pub message: String,
}

/// 探测用的最小 Anthropic Messages 请求体。
///
/// `max_tokens` 用 16 而非 1:翻译类 provider 会把它映射成 `max_output_tokens`,
/// 取 1 时部分 reasoning 模型直接 400("max tokens too small for thinking")。
/// 对 Anthropic 透传家族无影响(本就只是 ping)。
fn ping_body(model: &str) -> Value {
    serde_json::json!({
        "model": model,
        "max_tokens": 16,
        "messages": [{"role": "user", "content": "ping"}]
    })
}

/// 把 `dispatch_*_attempt` 的 `Result` 收敛成 `ProbeResult`。
/// `Ok` 一定意味着上游 2xx(各 dispatch 内部已把 4xx/5xx/网络错都转成 `Err`)。
fn dispatch_result_to_probe<T>(res: Result<T, OAuthDispatchError>) -> ProbeResult {
    match res {
        Ok(_) => ProbeResult {
            ok: true,
            http_status: Some(200),
            message: "连接正常".into(),
        },
        // access_token 拿不到 / refresh 失败: 对 OAuth 订阅就是"需要重新登录", 是正确的测试结果。
        Err(OAuthDispatchError::Auth(message)) => ProbeResult {
            ok: false,
            http_status: Some(401),
            message,
        },
        Err(OAuthDispatchError::Upstream { status, message }) => ProbeResult {
            ok: false,
            http_status: status,
            message,
        },
    }
}

/// 用最小 prompt 探测一次 messages 端点(Anthropic 透传家族专用 leaf)。
///
/// 仅被 `probe_subscription` 的 `ApiKey` 分支调用,故保持私有;调用方需准备好订阅 row
/// (含 snapshot 连接信息) + 已完成改写的 body — probe 只负责发请求。
/// `client` 应使用 `state.probe_client`(30s 短超时单例)。
///
/// 收 `body` 而不是 `model`: 调用方会先对 body 做与真实 dispatch 同款的改写
/// (当前是订阅槽位级 forced effort 双写), 若这里自己重新造 body 那些改写就全丢了。
async fn probe(
    client: &reqwest::Client,
    row: &SubscriptionRow,
    body: &Value,
) -> ProbeResult {
    let url = row.messages_url();

    let mut headers = ReqHeaderMap::new();
    if let (Ok(name), Ok(value)) = (
        ReqHeaderName::try_from(row.auth_header_name.as_str()),
        ReqHeaderValue::from_str(&row.auth_header_value()),
    ) {
        headers.insert(name, value);
    }
    for (k, v) in row.required_headers.iter() {
        if let (Ok(name), Ok(value)) = (
            ReqHeaderName::try_from(k.as_str()),
            ReqHeaderValue::from_str(v),
        ) {
            headers.insert(name, value);
        }
    }
    headers.insert(CONTENT_TYPE, ReqHeaderValue::from_static("application/json"));

    match client.post(&url).headers(headers).json(body).send().await {
        Ok(r) => {
            let status = r.status();
            let status_u16 = status.as_u16();
            let body_text = r.text().await.unwrap_or_default();

            if status.is_success() {
                ProbeResult {
                    ok: true,
                    http_status: Some(status_u16),
                    message: format!("连接正常 (HTTP {status_u16})"),
                }
            } else {
                let snippet: String = body_text.chars().take(300).collect();
                let message = if snippet.is_empty() {
                    format!("HTTP {status_u16}")
                } else {
                    format!("HTTP {status_u16}: {snippet}")
                };
                ProbeResult {
                    ok: false,
                    http_status: Some(status_u16),
                    message,
                }
            }
        }
        Err(e) => ProbeResult {
            ok: false,
            http_status: None,
            message: format!("网络错误: {e}"),
        },
    }
}

/// 按 `auth_type` 探测一条订阅的真实可达性。
///
/// 与真实请求管线 [`crate::proxy::pipeline::dispatch`] 共用同一批 `dispatch_*_attempt`:
/// 透传家族(`ApiKey`)走轻量 [`probe`];OAuth / 翻译类 provider 走各自 dispatch
/// (复用 token 刷新 + `{model}` 替换 + 协议 body 翻译),保证"测过=真能用"。
///
/// `is_streaming=false` → 各 dispatch 走非流式 collector;无客户端转发头,故
/// `forward_headers`/`client_headers` 传空。`sub_id` 取自 `row.id`。
///
/// `slot`: `model` 来自哪个订阅槽位 (由 [`pick_test_model`] 给出)。用来取该槽位的 forced
/// effort, 让探测的 effort 与真实 dispatch 一致 —— 否则用户给"只接受某一档的模型"固定了档位,
/// 探测却发 yaml 默认档触发上游 400, UI 就会谎报"连接不通"。example_models 兜底时传 None。
pub async fn probe_subscription(
    state: &AppState,
    row: &SubscriptionRow,
    model: &str,
    slot: Option<SubscriptionSlot>,
) -> ProbeResult {
    let sub_id = row.id;
    let mut body = ping_body(model);
    let forced_effort: Option<String> = slot
        .and_then(|s| row.slot_efforts.get(s))
        .map(str::to_string);

    match row.auth_type {
        // Anthropic 透传家族: body + auth 与上游兼容, 直接打 messages 端点。
        AuthType::ApiKey => {
            // 与真实 dispatch 的透传分支同款改写。ping_body 既没有 output_config 也没有
            // thinking, 所以只会写出 output_config.effort (thinking 不凭空创建) —— 与真实
            // 请求的行为规则一致。
            if let Some(e) = forced_effort.as_deref() {
                crate::proxy::sanitize::force_anthropic_effort(&mut body, e);
            }
            probe(&state.probe_client, row, &body).await
        }

        // ChatGPT OAuth (codex): refresh access_token + Anthropic→Responses 翻译。
        AuthType::ChatgptOauth => {
            let (expose, default_effort) = provider_reasoning_defaults(state, &row.provider_id);
            let extras = CodexExtras {
                reasoning_effort: resolve_reasoning_effort(&body, forced_effort.as_deref(), default_effort.as_deref()),
                expose_reasoning: expose,
            };
            let res = dispatch_oauth_attempt(
                &state.probe_client,
                state.chatgpt_oauth.clone(),
                sub_id,
                row.oauth_metadata.refresh_token.clone(),
                row.oauth_metadata.account_id.clone(),
                row.messages_url(),
                &body,
                false,
                Vec::new(),
                HeaderMap::new(),
                row.required_headers.clone(),
                extras,
            )
            .await;
            dispatch_result_to_probe(res)
        }

        // Kiro OAuth: OIDC/social refresh + Anthropic→CodeWhisperer 翻译。
        AuthType::KiroOauth => {
            let res = dispatch_kiro_attempt(
                &state.probe_client,
                state.kiro_oauth.clone(),
                sub_id,
                row.oauth_metadata.clone(),
                row.messages_url(),
                &body,
                false,
                Vec::new(),
                HeaderMap::new(),
                row.required_headers.clone(),
            )
            .await;
            dispatch_result_to_probe(res)
        }

        // Google AI Studio: api key + Gemini generateContent 翻译, model 嵌 URL ({model} 占位符)。
        AuthType::GeminiApiKey => {
            let (expose, default_effort) = provider_reasoning_defaults(state, &row.provider_id);
            let extras = GeminiExtras {
                thinking_budget: resolve_thinking_budget(&body, forced_effort.as_deref(), default_effort.as_deref()),
                include_thoughts: expose,
            };
            let res = dispatch_gemini_attempt(
                &state.probe_client,
                row.auth_header_value(),
                row.auth_header_name.clone(),
                row.messages_url(),
                model.to_string(),
                &body,
                false,
                Vec::new(),
                HeaderMap::new(),
                row.required_headers.clone(),
                extras,
            )
            .await;
            dispatch_result_to_probe(res)
        }

        // Gemini Interactions: api key + /v1beta/interactions 翻译, model 在 body。
        AuthType::GeminiInteractionsApiKey => {
            let (expose, default_effort) = provider_reasoning_defaults(state, &row.provider_id);
            let extras = InteractionsExtras {
                thinking_level: resolve_thinking_level(&body, forced_effort.as_deref(), default_effort.as_deref()),
                include_thoughts: expose,
            };
            let res = dispatch_gemini_interactions_attempt(
                &state.probe_client,
                row.auth_header_value(),
                row.auth_header_name.clone(),
                row.messages_url(),
                model.to_string(),
                &body,
                false,
                Vec::new(),
                HeaderMap::new(),
                row.required_headers.clone(),
                extras,
                Uuid::new_v4(),
                None,
            )
            .await;
            dispatch_result_to_probe(res)
        }

        // OpenAI Responses (api key): Bearer + Anthropic→/v1/responses 翻译。
        AuthType::OpenaiResponsesApiKey => {
            let (expose, default_effort) = provider_reasoning_defaults(state, &row.provider_id);
            let extras = OpenAiResponsesExtras {
                reasoning_effort: resolve_reasoning_effort(&body, forced_effort.as_deref(), default_effort.as_deref()),
                expose_reasoning: expose,
            };
            let res = dispatch_openai_responses_attempt(
                &state.probe_client,
                row.api_key.clone(),
                row.auth_header_name.clone(),
                row.auth_header_format.clone(),
                row.messages_url(),
                &body,
                false,
                Vec::new(),
                HeaderMap::new(),
                row.required_headers.clone(),
                extras,
            )
            .await;
            dispatch_result_to_probe(res)
        }

        // OpenAI Chat Completions (api key): Bearer + Anthropic→/v1/chat/completions 翻译。
        AuthType::OpenaiChatCompletionsApiKey => {
            let (expose, default_effort) = provider_reasoning_defaults(state, &row.provider_id);
            let extras = ChatCompletionsExtras {
                reasoning_effort: resolve_reasoning_effort(&body, forced_effort.as_deref(), default_effort.as_deref()),
                expose_reasoning: expose,
            };
            let res = dispatch_openai_chat_completions_attempt(
                &state.probe_client,
                row.api_key.clone(),
                row.auth_header_name.clone(),
                row.auth_header_format.clone(),
                row.messages_url(),
                &body,
                false,
                Vec::new(),
                HeaderMap::new(),
                row.required_headers.clone(),
                extras,
            )
            .await;
            dispatch_result_to_probe(res)
        }
    }
}

/// 选一个 model 名用于探测请求, 并一并返回它来自哪个槽位。优先 sonnet → haiku → opus
/// (CC 主用 sonnet), 都为空时退到订阅 snapshot 的 model_discovery.example_models[0]。
/// "(pending)" 是 SubscriptionNewPage 两步向导留下的占位, 跳过。
///
/// 返回槽位是为了让 [`probe_subscription`] 能取该槽位的 forced effort —— 否则用户给"只接受
/// 某一档的模型"固定了档位, 探测却发 yaml 默认档, 上游 400, UI 谎报"连接不通"。
/// example_models 兜底时没有对应槽位, 返回 None。
///
/// 注: 刻意不遍历 fable (保持既有行为不变) —— 探测只验证连通性, 用最弱可用的槽位即可。
/// 代价是只给 fable 槽设了 effort 的订阅, 探测不会带上那个 effort。
pub fn pick_test_model(row: &SubscriptionRow) -> Option<(String, Option<SubscriptionSlot>)> {
    for (s, slot) in [
        (&row.model_slots.sonnet, SubscriptionSlot::Sonnet),
        (&row.model_slots.haiku, SubscriptionSlot::Haiku),
        (&row.model_slots.opus, SubscriptionSlot::Opus),
    ] {
        let trimmed = s.trim();
        if !trimmed.is_empty() && trimmed != "(pending)" {
            return Some((trimmed.to_string(), Some(slot)));
        }
    }
    row.model_discovery
        .example_models
        .first()
        .cloned()
        .map(|m| (m, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_body_uses_max_tokens_16() {
        let b = ping_body("gemini-2.5-pro");
        assert_eq!(b["model"], "gemini-2.5-pro");
        // 不能是 1: 翻译成 max_output_tokens=1 会让部分 reasoning 模型 400。
        assert_eq!(b["max_tokens"], 16);
        assert_eq!(b["messages"][0]["role"], "user");
    }

    // ---------- pick_test_model 返回槽位 ----------

    #[test]
    fn pick_test_model_returns_sonnet_slot() {
        let row = SubscriptionRow::test_fixture("anthropic", "default");
        // fixture: fable="d", opus="a", sonnet="b", haiku="c"
        assert_eq!(
            pick_test_model(&row),
            Some(("b".to_string(), Some(SubscriptionSlot::Sonnet)))
        );
    }

    #[test]
    fn pick_test_model_falls_back_to_haiku_slot() {
        let mut row = SubscriptionRow::test_fixture("anthropic", "default");
        row.model_slots.sonnet = String::new();
        assert_eq!(
            pick_test_model(&row),
            Some(("c".to_string(), Some(SubscriptionSlot::Haiku)))
        );
    }

    #[test]
    fn pick_test_model_skips_pending_placeholder() {
        let mut row = SubscriptionRow::test_fixture("anthropic", "default");
        row.model_slots.sonnet = "(pending)".into();
        assert_eq!(
            pick_test_model(&row),
            Some(("c".to_string(), Some(SubscriptionSlot::Haiku)))
        );
    }

    /// example_models 兜底时没有对应槽位 → 探测不带 forced effort。
    #[test]
    fn pick_test_model_example_models_has_no_slot() {
        let mut row = SubscriptionRow::test_fixture("anthropic", "default");
        row.model_slots.fable = String::new();
        row.model_slots.opus = String::new();
        row.model_slots.sonnet = String::new();
        row.model_slots.haiku = String::new();
        row.model_discovery.example_models = vec!["example-model".into()];
        assert_eq!(
            pick_test_model(&row),
            Some(("example-model".to_string(), None))
        );
    }

    #[test]
    fn dispatch_ok_maps_to_success() {
        let r = dispatch_result_to_probe::<()>(Ok(()));
        assert!(r.ok);
        assert_eq!(r.http_status, Some(200));
    }

    #[test]
    fn dispatch_auth_err_maps_to_401() {
        let r = dispatch_result_to_probe::<()>(Err(OAuthDispatchError::Auth(
            "需要重新登录".into(),
        )));
        assert!(!r.ok);
        assert_eq!(r.http_status, Some(401));
        assert_eq!(r.message, "需要重新登录");
    }

    #[test]
    fn dispatch_upstream_err_preserves_status_and_message() {
        let r = dispatch_result_to_probe::<()>(Err(OAuthDispatchError::Upstream {
            status: Some(400),
            message: "bad request".into(),
        }));
        assert!(!r.ok);
        assert_eq!(r.http_status, Some(400));
        assert_eq!(r.message, "bad request");

        // 网络层错误 status=None 时透传 None。
        let r2 = dispatch_result_to_probe::<()>(Err(OAuthDispatchError::Upstream {
            status: None,
            message: "网络错误".into(),
        }));
        assert!(!r2.ok);
        assert_eq!(r2.http_status, None);
    }
}
