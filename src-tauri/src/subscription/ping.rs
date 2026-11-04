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
use serde::Serialize;
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
use crate::proxy::transform::openai::{
    detect_responses_dialect, resolve_reasoning_effort, OpenAiResponsesExtras,
};
use crate::proxy::transform::openai_chat_completions::ChatCompletionsExtras;
use crate::proxy::transform::openai_responses::CodexExtras;
use crate::state::AppState;
use crate::subscription::model::SubscriptionRow;
use crate::subscription::schedule::WallClock;
use crate::virtual_model::SubscriptionSlot;

#[derive(Debug)]
pub struct ProbeResult {
    pub ok: bool,
    pub http_status: Option<u16>,
    pub message: String,
    /// `message` 是 cc-router 自己写的固定文案时, 这里给出它的结构化形式,
    /// 供桌面端按界面语言显示; 上游原文 (HTTP 错误体 / OAuth 报错) 为 None。
    pub note: Option<ProbeNote>,
}

/// 测试连接结果里 cc-router 自己的固定文案。`message` 字段仍保留中文原文 (TUI 原样显示)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum ProbeNote {
    Ok,
    Network { detail: String },
    /// 订阅没有任何 model 槽位, 也没有 example_models 可兜底。
    NoTestModel,
}

impl ProbeNote {
    pub fn message(&self, http_status: Option<u16>) -> String {
        match self {
            Self::Ok => match http_status {
                Some(s) => format!("连接正常 (HTTP {s})"),
                None => "连接正常".into(),
            },
            Self::Network { detail } => format!("网络错误: {detail}"),
            Self::NoTestModel => "订阅未配置任何 model 槽位, 且未提供 example_models, 无法测试".into(),
        }
    }
}

impl ProbeResult {
    /// 固定文案的结果: `message` 由 `note` 生成, 两者不会不一致。
    pub fn noted(ok: bool, http_status: Option<u16>, note: ProbeNote) -> Self {
        Self { ok, http_status, message: note.message(http_status), note: Some(note) }
    }
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
        // 历史上这条路径只写「连接正常」不带状态码; 统一成带 (HTTP 200) 与透传家族一致。
        Ok(_) => ProbeResult::noted(true, Some(200), ProbeNote::Ok),
        // access_token 拿不到 / refresh 失败: 对 OAuth 订阅就是"需要重新登录", 是正确的测试结果。
        Err(OAuthDispatchError::Auth(message)) => ProbeResult {
            ok: false,
            http_status: Some(401),
            message,
            note: None,
        },
        // 有状态码 = 上游 HTTP 错误, message 是原始错误体 (可能为空): 与透传家族同款格式化。
        // 无状态码 = cc-router 自己写的网络 / 翻译错误文案, 原样保留。
        Err(OAuthDispatchError::Upstream { status: Some(s), message }) => ProbeResult {
            ok: false,
            http_status: Some(s),
            message: http_failure_message(s, &message),
            note: None,
        },
        Err(OAuthDispatchError::Upstream { status: None, message }) => ProbeResult {
            ok: false,
            http_status: None,
            message,
            note: None,
        },
    }
}

/// 上游非 2xx 时给用户看的失败文案: `HTTP 4xx: <错误体前 300 字>`, 错误体为空时只有 `HTTP 4xx`。
/// 三条探测路径 (透传 / 翻译类 dispatch / System One) 共用, 保证失败原因永远不为空 (issue #63)。
fn http_failure_message(status: u16, body: &str) -> String {
    let prefix = format!("HTTP {status}");
    let snippet: String = body.trim().chars().take(300).collect();
    // 部分 dispatch 在上游没给错误体时已自行填了 "HTTP 4xx", 不重复拼。
    if snippet.is_empty() || snippet == prefix {
        prefix
    } else {
        format!("{prefix}: {snippet}")
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
    for (k, v) in row.resolved_required_headers().iter() {
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
                ProbeResult::noted(true, Some(status_u16), ProbeNote::Ok)
            } else {
                ProbeResult {
                    ok: false,
                    http_status: Some(status_u16),
                    message: http_failure_message(status_u16, &body_text),
                    note: None,
                }
            }
        }
        Err(e) => ProbeResult::noted(false, None, ProbeNote::Network { detail: e.to_string() }),
    }
}

/// System One 订阅的探测: 与真实 dispatch 共用 [`crate::proxy::systemone::build_outbound`]
/// (同一套出站头与 model 改写规则), 发一道最小 noul 题。
async fn probe_systemone(client: &reqwest::Client, row: &SubscriptionRow, model: &str) -> ProbeResult {
    use crate::proxy::systemone::{build_outbound, normalize_response, probe_body, AttemptResult};
    let raw = bytes::Bytes::from(serde_json::to_vec(&probe_body(model)).unwrap_or_default());
    let out = match build_outbound(row, &raw, model) {
        Ok(o) => o,
        Err(e) => return ProbeResult::noted(false, None, ProbeNote::Network { detail: e }),
    };
    let wire = out.wire;
    let r = match crate::proxy::upstream::send(client, &out.url, out.body.to_vec(), out.headers, false).await {
        Ok(crate::proxy::upstream::UpstreamResponse::NonStreaming { status, body, body_text, .. }) => {
            AttemptResult::Http { status, body, body_text }
        }
        Ok(_) => return ProbeResult::noted(true, None, ProbeNote::Ok),
        Err(e) => return ProbeResult::noted(false, None, ProbeNote::Network { detail: e.to_string() }),
    };
    match normalize_response(wire, r) {
        AttemptResult::Http { status, body_text, .. } => {
            let s = status.as_u16();
            if status.is_success() {
                ProbeResult::noted(true, Some(s), ProbeNote::Ok)
            } else {
                let message = http_failure_message(s, &body_text.unwrap_or_default());
                ProbeResult { ok: false, http_status: Some(s), message, note: None }
            }
        }
        AttemptResult::Network(detail) => ProbeResult::noted(false, None, ProbeNote::Network { detail }),
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
/// `slot`: `model` 来自哪个订阅槽位 (由 [`pick_test_model_at`] 给出)。用来取该槽位的 forced
/// effort, 让探测的 effort 与真实 dispatch 一致 —— 否则用户给"只接受某一档的模型"固定了档位,
/// 探测却发 yaml 默认档触发上游 400, UI 就会谎报"连接不通"。example_models 兜底时传 None。
pub async fn probe_subscription(
    state: &AppState,
    row: &SubscriptionRow,
    model: &str,
    slot: Option<SubscriptionSlot>,
    at: WallClock,
) -> ProbeResult {
    let sub_id = row.id;
    let mut body = ping_body(model);
    let forced_effort: Option<String> =
        slot.and_then(|s| probe_effort(row, s, model, at));

    // System One 订阅的 auth_type 也是 ApiKey, 必须先于 match 分流, 否则会被当成 Anthropic 发 ping_body。
    if row.endpoint_protocol == crate::provider::model::EndpointProtocol::Systemone {
        return probe_systemone(&state.probe_client, row, model).await;
    }

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
                row.resolved_required_headers(),
                extras,
                &mut crate::proxy::header_log::UpstreamHeaderCapture::default(), // 探测不写请求日志
                false,
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
                row.resolved_required_headers(),
                &mut crate::proxy::header_log::UpstreamHeaderCapture::default(), // 探测不写请求日志
                false,
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
                row.resolved_required_headers(),
                extras,
                &mut crate::proxy::header_log::UpstreamHeaderCapture::default(), // 探测不写请求日志
                false,
                "",
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
                row.resolved_required_headers(),
                extras,
                Uuid::new_v4(),
                None,
                &mut crate::proxy::header_log::UpstreamHeaderCapture::default(), // 探测不写请求日志
                false,
                "",
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
                // 与 pipeline 真实 dispatch 同一方言判定 (探测必须复用真实 dispatch 行为)
                dialect: detect_responses_dialect(&row.messages_url(), model),
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
                row.resolved_required_headers(),
                extras,
                &mut crate::proxy::header_log::UpstreamHeaderCapture::default(), // 探测不写请求日志
                false,
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
                row.resolved_required_headers(),
                extras,
                &mut crate::proxy::header_log::UpstreamHeaderCapture::default(), // 探测不写请求日志
                false,
            )
            .await;
            dispatch_result_to_probe(res)
        }
    }
}

/// 探测 `model` 时该带的思考强度, 与真实请求同规则: 它是此刻生效的那条时段规则的模型 →
/// 规则自带的强度 (没有则槽位级); 它是 [`pick_test_model_at`] 退到的「第一条规则」的模型 →
/// 那条规则的强度 (没有则槽位级); 其余 (默认模型) → 槽位级。
fn probe_effort(row: &SubscriptionRow, slot: SubscriptionSlot, model: &str, at: WallClock) -> Option<String> {
    let now = crate::subscription::schedule::effective_choice(
        row.model_slots.get(slot),
        row.slot_schedules.get(slot),
        at,
    );
    let rule_effort = match now {
        Some((m, e)) if m == model => e,
        _ => row
            .slot_schedules
            .get(slot)
            .iter()
            .find(|r| r.model.trim() == model)
            .filter(|_| row.model_slots.get(slot).trim() != model)
            .and_then(|r| r.effort.as_deref()),
    };
    rule_effort.or_else(|| row.slot_efforts.get(slot)).map(str::to_string)
}

/// 选一个 model 名用于探测请求, 并一并返回它来自哪个槽位。优先 sonnet → haiku → opus
/// (CC 主用 sonnet), 都为空时退到订阅 snapshot 的 model_discovery.example_models[0]。
/// "(pending)" 是 SubscriptionNewPage 两步向导留下的占位, 跳过。
/// 每个槽位内依次取: 此刻生效模型 (时段规则 → 默认) → 默认模型 → 第一条规则的模型 ——
/// 探测验证的是连通性, 不该因为「现在不在时段内」而测不了。
///
/// 返回槽位是为了让 [`probe_subscription`] 能取该槽位的 forced effort —— 否则用户给"只接受
/// 某一档的模型"固定了档位, 探测却发 yaml 默认档, 上游 400, UI 谎报"连接不通"。
/// example_models 兜底时没有对应槽位, 返回 None。
///
/// 注: 刻意不遍历 fable (保持既有行为不变) —— 探测只验证连通性, 用最弱可用的槽位即可。
/// 代价是只给 fable 槽设了 effort 的订阅, 探测不会带上那个 effort。
///
/// `at`: 本机挂钟时刻, 调用方取一次后同时传给 [`probe_subscription`] (测试用固定值)。
pub fn pick_test_model_at(
    row: &SubscriptionRow,
    at: WallClock,
) -> Option<(String, Option<SubscriptionSlot>)> {
    // System One 订阅不用四槽: Jev 槽优先, 否则端点 example_models (创建时已快照进 model_discovery)。
    if row.endpoint_protocol == crate::provider::model::EndpointProtocol::Systemone {
        return row
            .model_slots
            .jev_model()
            .map(str::to_string)
            .or_else(|| row.model_discovery.example_models.first().cloned())
            .map(|m| (m, None));
    }

    let usable = |m: &str| {
        let m = m.trim();
        (!m.is_empty() && m != "(pending)").then(|| m.to_string())
    };
    for slot in [
        SubscriptionSlot::Sonnet,
        SubscriptionSlot::Haiku,
        SubscriptionSlot::Opus,
    ] {
        let picked = row
            .slot_model_at(slot, at)
            .and_then(usable)
            .or_else(|| usable(row.model_slots.get(slot)))
            .or_else(|| {
                row.slot_schedules
                    .get(slot)
                    .first()
                    .and_then(|r| usable(&r.model))
            });
        if let Some(m) = picked {
            return Some((m, Some(slot)));
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
    fn probe_note_serializes_as_tagged_code_and_drives_message() {
        let net = ProbeNote::Network { detail: "timeout".into() };
        assert_eq!(
            serde_json::to_value(&net).unwrap(),
            serde_json::json!({ "code": "network", "detail": "timeout" })
        );
        assert_eq!(serde_json::to_value(ProbeNote::NoTestModel).unwrap(), serde_json::json!({ "code": "no_test_model" }));
        let r = ProbeResult::noted(true, Some(201), ProbeNote::Ok);
        assert_eq!(r.message, "连接正常 (HTTP 201)");
        assert_eq!(r.note, Some(ProbeNote::Ok));
        assert_eq!(ProbeResult::noted(false, None, net).message, "网络错误: timeout");
    }

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
            pick_test_model_at(&row, noon()),
            Some(("b".to_string(), Some(SubscriptionSlot::Sonnet)))
        );
    }

    #[test]
    fn pick_test_model_falls_back_to_haiku_slot() {
        let mut row = SubscriptionRow::test_fixture("anthropic", "default");
        row.model_slots.sonnet = String::new();
        assert_eq!(
            pick_test_model_at(&row, noon()),
            Some(("c".to_string(), Some(SubscriptionSlot::Haiku)))
        );
    }

    #[test]
    fn pick_test_model_skips_pending_placeholder() {
        let mut row = SubscriptionRow::test_fixture("anthropic", "default");
        row.model_slots.sonnet = "(pending)".into();
        assert_eq!(
            pick_test_model_at(&row, noon()),
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
            pick_test_model_at(&row, noon()),
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
        assert_eq!(r.message, "HTTP 400: bad request");

        // 网络层错误 status=None 时透传 None, 文案是 cc-router 自己写的, 原样保留。
        let r2 = dispatch_result_to_probe::<()>(Err(OAuthDispatchError::Upstream {
            status: None,
            message: "网络错误".into(),
        }));
        assert!(!r2.ok);
        assert_eq!(r2.http_status, None);
        assert_eq!(r2.message, "网络错误");
    }

    /// issue #63: 翻译类上游回空错误体时, 失败原因不能是空串。
    #[test]
    fn dispatch_upstream_err_with_empty_body_still_explains_failure() {
        for body in ["", "  \n"] {
            let r = dispatch_result_to_probe::<()>(Err(OAuthDispatchError::Upstream {
                status: Some(502),
                message: body.into(),
            }));
            assert_eq!(r.message, "HTTP 502");
        }
    }

    #[test]
    fn http_failure_message_formats_and_dedupes_prefix() {
        assert_eq!(http_failure_message(404, "not found"), "HTTP 404: not found");
        // dispatch 在没有错误体时已填了 "HTTP 4xx", 不拼成 "HTTP 403: HTTP 403"。
        assert_eq!(http_failure_message(403, "HTTP 403"), "HTTP 403");
        // 错误体截到 300 个字符 (按字符, 不切坏多字节)。
        let long = "错".repeat(400);
        let m = http_failure_message(500, &long);
        assert_eq!(m.chars().count(), "HTTP 500: ".chars().count() + 300);
    }

    #[test]
    fn pick_test_model_for_systemone_prefers_jev_slot_then_examples() {
        let mut row = SubscriptionRow::test_fixture("ollama", "localhost_systemone");
        row.endpoint_protocol = crate::provider::model::EndpointProtocol::Systemone;
        row.model_slots.sonnet = "should-not-be-used".into();
        row.model_discovery.example_models = vec!["clef-flash".into()];
        assert_eq!(pick_test_model_at(&row, noon()), Some(("clef-flash".to_string(), None)));
        row.model_slots.jev = "clef-pro".into();
        assert_eq!(pick_test_model_at(&row, noon()), Some(("clef-pro".to_string(), None)));
    }

    // ---------- pick_test_model_at + slot time schedule ----------

    use crate::subscription::schedule::{ScheduleRule, WallClock};

    fn noon() -> WallClock {
        WallClock { weekday: 3, minute: 12 * 60 }
    }

    fn evening() -> WallClock {
        WallClock { weekday: 3, minute: 20 * 60 }
    }

    /// 探测的思考强度与真实请求同规则: 此刻命中的规则强度优先; 退到第一条规则的模型时用那条的强度。
    #[test]
    fn probe_effort_follows_rule_effort() {
        let mut row = SubscriptionRow::test_fixture("anthropic", "default");
        row.model_slots.sonnet = String::new();
        row.slot_efforts.sonnet = Some("low".into());
        let mut r = night_rule("night-model");
        r.effort = Some("max".into());
        row.slot_schedules.sonnet = vec![r];
        // 夜里命中规则 → 规则强度
        assert_eq!(probe_effort(&row, SubscriptionSlot::Sonnet, "night-model", evening()), Some("max".into()));
        // 白天没有生效模型, pick_test_model_at 退到第一条规则 → 仍用那条规则的强度
        assert_eq!(pick_test_model_at(&row, noon()), Some(("night-model".into(), Some(SubscriptionSlot::Sonnet))));
        assert_eq!(probe_effort(&row, SubscriptionSlot::Sonnet, "night-model", noon()), Some("max".into()));
        // 规则不带强度 → 槽位级
        row.slot_schedules.sonnet[0].effort = None;
        assert_eq!(probe_effort(&row, SubscriptionSlot::Sonnet, "night-model", evening()), Some("low".into()));
        // 默认模型生效 → 槽位级
        row.model_slots.sonnet = "day-model".into();
        row.slot_schedules.sonnet[0].effort = Some("max".into());
        assert_eq!(probe_effort(&row, SubscriptionSlot::Sonnet, "day-model", noon()), Some("low".into()));
    }

    fn night_rule(model: &str) -> ScheduleRule {
        ScheduleRule {
            days: vec![1, 2, 3, 4, 5, 6, 7],
            start: "18:00".into(),
            end: "08:00".into(),
            model: model.into(),
            effort: None,
        }
    }

    #[test]
    fn pick_test_model_uses_first_rule_when_outside_window_and_default_blank() {
        let mut row = SubscriptionRow::test_fixture("anthropic", "default");
        row.model_slots.sonnet = String::new();
        row.slot_schedules.sonnet = vec![night_rule("night-1"), night_rule("night-2")];
        // haiku still has a default ("c"), but sonnet is tried first and has a rule model
        assert_eq!(
            pick_test_model_at(&row, noon()),
            Some(("night-1".to_string(), Some(SubscriptionSlot::Sonnet)))
        );
    }

    #[test]
    fn pick_test_model_uses_rule_model_inside_window() {
        let mut row = SubscriptionRow::test_fixture("anthropic", "default");
        // default "b" is set, but the rule covers 20:00
        row.slot_schedules.sonnet = vec![night_rule("night")];
        assert_eq!(
            pick_test_model_at(&row, evening()),
            Some(("night".to_string(), Some(SubscriptionSlot::Sonnet)))
        );
        assert_eq!(
            pick_test_model_at(&row, noon()),
            Some(("b".to_string(), Some(SubscriptionSlot::Sonnet)))
        );
    }

    #[test]
    fn pick_test_model_skips_blank_slot_without_rules() {
        let mut row = SubscriptionRow::test_fixture("anthropic", "default");
        row.model_slots.sonnet = "  ".into();
        row.model_slots.haiku = String::new();
        assert_eq!(
            pick_test_model_at(&row, noon()),
            Some(("a".to_string(), Some(SubscriptionSlot::Opus)))
        );
    }
}
