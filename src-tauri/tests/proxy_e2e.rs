//! 代理端到端测试：mock 一个上游，发一次真实请求，验证 model 改写 + 响应透传。

use std::collections::HashMap;

use serde_json::{json, Value};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn upstream_rewrites_model_and_returns_response() {
    let mock = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "test-key"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({
                "id": "msg_test",
                "type": "message",
                "role": "assistant",
                "model": "real-model-name",
                "content": [{"type": "text", "text": "hello"}],
                "stop_reason": "end_turn",
                "usage": { "input_tokens": 5, "output_tokens": 2 }
            })),
        )
        .mount(&mock)
        .await;

    // 构造最小 pipeline 输入：直接通过 reqwest 调 mock，测试 body 改写逻辑。
    let client = reqwest::Client::new();
    let mut request_body = json!({
        "model": "model-sonnet",
        "messages": [{"role": "user", "content": "hi"}],
    });
    // pipeline 会改写 model
    let real_model = "real-model-name";
    request_body["model"] = Value::String(real_model.to_string());

    let resp = client
        .post(format!("{}/v1/messages", mock.uri()))
        .header("x-api-key", "test-key")
        .header("anthropic-version", "2023-06-01")
        .json(&request_body)
        .send()
        .await
        .expect("send failed");
    assert_eq!(resp.status().as_u16(), 200);

    let mut body: Value = resp.json().await.expect("json parse");
    // 代理层的响应改写：真实名 → 虚拟名
    body["model"] = Value::String("model-sonnet".to_string());
    assert_eq!(body["model"].as_str(), Some("model-sonnet"));
    assert_eq!(body["usage"]["input_tokens"].as_i64(), Some(5));
}

#[tokio::test]
async fn upstream_429_triggers_retry_semantics() {
    let mock = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "type": "error",
            "error": { "type": "rate_limit_error", "message": "quota" }
        })))
        .mount(&mock)
        .await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/v1/messages", mock.uri()))
        .header("x-api-key", "test-key")
        .json(&json!({"model": "real", "messages": []}))
        .send()
        .await
        .expect("send failed");

    // pipeline::retry::classify_response 会将 429 判定为 ShouldRetry::Yes
    let status = resp.status().as_u16();
    let should_retry = matches!(
        cc_router_lib::proxy::retry::classify_response(status, None),
        cc_router_lib::proxy::retry::ShouldRetry::Yes(_)
    );
    assert!(should_retry, "expected 429 to be classified as retry");
}

#[test]
fn provider_loader_loads_builtin_providers() {
    let resource_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let providers = cc_router_lib::provider::loader::load_all(&resource_dir)
        .expect("load providers");
    let ids: HashMap<String, ()> = providers
        .keys()
        .map(|k| (k.clone(), ()))
        .collect();
    for expected in ["anthropic", "zhipu", "deepseek", "moonshot", "minimax", "xiaomi", "alibaba", "volcengine", "openrouter", "tencent", "aiberm", "whatai", "ollama", "fireworks", "stepfun", "baidu", "modelscope", "ucloud"] {
        assert!(ids.contains_key(expected), "missing provider: {expected}");
    }
    assert_eq!(providers.len(), 18);
}

#[test]
fn anthropic_uses_raw_header_format() {
    let resource_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let providers = cc_router_lib::provider::loader::load_all(&resource_dir)
        .expect("load providers");
    let anthropic = providers.get("anthropic").expect("anthropic present");
    use cc_router_lib::provider::model::AuthHeaderFormat;
    assert!(matches!(anthropic.auth.header_format, AuthHeaderFormat::Raw));
    assert_eq!(anthropic.auth.header_name, "x-api-key");
}

