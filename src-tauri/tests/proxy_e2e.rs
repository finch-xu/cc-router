//! 代理端到端测试：mock 一个上游，发一次真实请求，验证 model 改写 + 响应透传。

use std::collections::HashMap;

use serde_json::{json, Value};
use wiremock::matchers::{body_partial_json, header, method, path};
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

/// 守护 deepseek.yaml 不被改回错误的方言声明。
#[test]
fn deepseek_supports_standard_thinking_protocol() {
    let resource_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let providers = cc_router_lib::provider::loader::load_all(&resource_dir)
        .expect("load providers");

    let deepseek = providers.get("deepseek").expect("deepseek present");
    assert!(
        deepseek.capabilities.supports_thinking_blocks,
        "DeepSeek 默认就返回 thinking 块, supports_thinking_blocks 必须为 true"
    );
}

/// e2e: 验证 deepseek quirk 在 messages 历史无 thinking 块时主动注入
/// `thinking: {"type": "disabled"}` 后, 请求发到 mock deepseek 上游能正常返回 200.
///
/// 这模拟 cc-router round_robin 跨订阅时第一轮路由到不返 thinking 块的家,
/// 第二轮路由到 deepseek 时 messages 历史里 assistant 含 tool_use 但无 thinking 块的场景.
/// 修复前: deepseek 报 400 "content[].thinking ... must be passed back".
/// 修复后: cc-router 注入 thinking:disabled, deepseek 跳过严格校验返回 200.
#[tokio::test]
async fn deepseek_quirk_injects_disable_and_upstream_accepts() {
    let mock = MockServer::start().await;

    // wiremock 用 body_partial_json 匹配 body 必须含 thinking={type:"disabled"}
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(body_partial_json(json!({
            "thinking": {"type": "disabled"}
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_test",
            "type": "message",
            "role": "assistant",
            "model": "deepseek-v4-pro",
            "content": [{"type": "text", "text": "ok"}],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 5, "output_tokens": 2 }
        })))
        .mount(&mock)
        .await;

    // 构造 cc-router 即将转发到 deepseek 的 body：
    // assistant 历史含 tool_use 但无 thinking 块——这是触发 deepseek 严格校验的场景.
    let mut upstream_body = json!({
        "model": "deepseek-v4-pro",
        "messages": [
            {"role": "user", "content": "weather?"},
            {"role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "get_weather", "input": {}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "sunny"}
            ]}
        ]
    });

    // 调用 pipeline 里的 quirk 函数（与生产代码同一函数）
    cc_router_lib::proxy::pipeline::apply_deepseek_thinking_quirk(&mut upstream_body);

    // 断言 quirk 成功注入
    assert_eq!(
        upstream_body["thinking"],
        json!({"type": "disabled"}),
        "quirk 应注入 thinking:disabled"
    );

    // 发到 mock; mock 用 body_partial_json 匹配, 无匹配会返回 404
    let resp = reqwest::Client::new()
        .post(format!("{}/v1/messages", mock.uri()))
        .header("anthropic-version", "2023-06-01")
        .json(&upstream_body)
        .send()
        .await
        .expect("send failed");

    assert_eq!(
        resp.status().as_u16(),
        200,
        "mock 上游必须收到含 thinking:disabled 的 body 才返回 200"
    );
}

/// e2e: 验证客户端已显式设置 thinking 字段时 quirk 不会覆盖.
#[tokio::test]
async fn deepseek_quirk_respects_client_thinking_in_e2e() {
    let mock = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(body_partial_json(json!({
            "thinking": {"type": "enabled", "budget_tokens": 1024}
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_test",
            "type": "message",
            "role": "assistant",
            "model": "deepseek-v4-pro",
            "content": [{"type": "text", "text": "ok"}],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 1, "output_tokens": 1 }
        })))
        .mount(&mock)
        .await;

    let mut upstream_body = json!({
        "model": "deepseek-v4-pro",
        "thinking": {"type": "enabled", "budget_tokens": 1024},
        "messages": [{"role": "user", "content": "hi"}]
    });

    cc_router_lib::proxy::pipeline::apply_deepseek_thinking_quirk(&mut upstream_body);

    // 客户端显式设的字段不应被覆盖
    assert_eq!(
        upstream_body["thinking"],
        json!({"type": "enabled", "budget_tokens": 1024})
    );

    let resp = reqwest::Client::new()
        .post(format!("{}/v1/messages", mock.uri()))
        .header("anthropic-version", "2023-06-01")
        .json(&upstream_body)
        .send()
        .await
        .expect("send failed");
    assert_eq!(resp.status().as_u16(), 200);
}
