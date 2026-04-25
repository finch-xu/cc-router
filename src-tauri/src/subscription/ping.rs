//! 订阅可达性探测 helper。
//!
//! 用最小 prompt 真实打 messages 端点验证连接,被 `commands::test_connection` 和
//! `recheck_worker` 共用。
//!
//! 设计:
//! - 不打 /models — 避免 URL 拼接歧义和 enabled:false provider 无法测的限制
//! - 与生产请求路径完全等价(URL/auth/required headers 都用同一份 helper)
//! - 失败原因都编码到 ProbeResult.message, 不返回 Result(调用方逻辑简单)

use reqwest::header::{
    HeaderMap as ReqHeaderMap, HeaderName as ReqHeaderName, HeaderValue as ReqHeaderValue,
    CONTENT_TYPE,
};

use crate::provider::model::{Provider, ProviderEndpoint};
use crate::subscription::model::ModelSlots;

#[derive(Debug)]
pub struct ProbeResult {
    pub ok: bool,
    pub http_status: Option<u16>,
    pub message: String,
}

/// 用最小 prompt 探测一次 messages 端点。
///
/// 调用方需自行准备好 provider / endpoint / api_key / model — probe 只负责发请求。
/// `client` 应使用 `state.probe_client`(30s 短超时单例)。
pub async fn probe(
    client: &reqwest::Client,
    provider: &Provider,
    endpoint: &ProviderEndpoint,
    api_key: &str,
    model: &str,
) -> ProbeResult {
    let url = endpoint.messages_url();

    let mut headers = ReqHeaderMap::new();
    if let (Ok(name), Ok(value)) = (
        ReqHeaderName::try_from(provider.auth.header_name.as_str()),
        ReqHeaderValue::from_str(&provider.auth.header_value(api_key)),
    ) {
        headers.insert(name, value);
    }
    for (k, v) in provider.required_headers.iter() {
        if let (Ok(name), Ok(value)) = (
            ReqHeaderName::try_from(k.as_str()),
            ReqHeaderValue::from_str(v),
        ) {
            headers.insert(name, value);
        }
    }
    headers.insert(CONTENT_TYPE, ReqHeaderValue::from_static("application/json"));

    let body = serde_json::json!({
        "model": model,
        "max_tokens": 1,
        "messages": [{"role": "user", "content": "ping"}]
    });

    match client.post(&url).headers(headers).json(&body).send().await {
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

/// 选一个 model 名用于探测请求。优先 sonnet → haiku → opus(CC 主用 sonnet),
/// 都为空时退到 provider.example_models[0]。
/// "(pending)" 是 SubscriptionNewPage 两步向导留下的占位, 跳过。
pub fn pick_test_model(slots: &ModelSlots, examples: &[String]) -> Option<String> {
    for s in [&slots.sonnet, &slots.haiku, &slots.opus] {
        let trimmed = s.trim();
        if !trimmed.is_empty() && trimmed != "(pending)" {
            return Some(trimmed.to_string());
        }
    }
    examples.first().cloned()
}
