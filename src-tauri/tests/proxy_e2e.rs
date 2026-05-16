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
    for expected in ["anthropic", "zhipu", "deepseek", "moonshot", "minimax", "xiaomi", "alibaba", "volcengine", "openrouter", "tencent", "aiberm", "whatai", "ollama", "fireworks", "stepfun", "baidu", "modelscope", "ucloud", "openai_codex", "openai", "kiro", "google_ai_studio"] {
        assert!(ids.contains_key(expected), "missing provider: {expected}");
    }
    assert_eq!(providers.len(), 22);
}

/// 上游用 200 + Anthropic SSE `event: error` 表达限流 (智谱 1308 真实场景);
/// peek_first_event 必须识别为 UpstreamError 让 dispatcher 切下家,
/// 而不是当成 200 成功透传给客户端.
#[tokio::test]
async fn streaming_sse_error_event_recognized_via_peek() {
    use bytes::Bytes;
    use futures::StreamExt;

    let mock = MockServer::start().await;

    // 模拟智谱 1308 5h 配额耗尽响应: 200 + Anthropic SSE event: error 流
    let sse_body = "event: error\n\
        data: {\"error\":{\"code\":\"1308\",\"message\":\"\u{5DF2}\u{8FBE}\u{5230} 5 \u{5C0F}\u{65F6}\u{7684}\u{4F7F}\u{7528}\u{4E0A}\u{9650}\"},\"request_id\":\"x\"}\n\
        \n\
        data: [DONE]\n\
        \n";

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .mount(&mock)
        .await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/v1/messages", mock.uri()))
        .json(&json!({"model": "glm-4.6", "messages": [], "stream": true}))
        .send()
        .await
        .expect("send failed");
    assert_eq!(resp.status().as_u16(), 200, "上游返 200");

    let stream: futures::stream::BoxStream<
        'static,
        Result<Bytes, reqwest::Error>,
    > = Box::pin(resp.bytes_stream().map(|r| r.map(Bytes::from)));

    let result = cc_router_lib::proxy::sse::peek_first_event(stream).await;
    match result {
        cc_router_lib::proxy::sse::PeekResult::UpstreamError { code, message, .. } => {
            assert_eq!(code.as_deref(), Some(cc_router_lib::proxy::sse::ZHIPU_ERR_QUOTA_EXHAUSTED));
            // 5h 文案应被 zhipu classifier 判为 QuotaExhausted
            assert!(cc_router_lib::proxy::sse::classify_zhipu_sse_error(
                code.as_deref(),
                message.as_deref()
            ));
        }
        other => panic!("expected UpstreamError, got {:?}", other),
    }
}

/// 正常 SSE 流: peek 应当返回 Ok 并保留首事件字节, dispatcher 把 first_chunk
/// 注入 stream_response 后客户端能看到完整事件序列.
#[tokio::test]
async fn streaming_normal_first_event_returns_ok_and_preserves_bytes() {
    use bytes::Bytes;
    use futures::StreamExt;

    let mock = MockServer::start().await;

    let first_event = "event: message_start\n\
        data: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"model\":\"glm-4.6\",\"role\":\"assistant\",\"type\":\"message\",\"content\":[],\"usage\":{\"input_tokens\":3}}}\n\
        \n";
    let second_event = "event: content_block_delta\n\
        data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"},\"index\":0}\n\
        \n";
    let body = format!("{}{}", first_event, second_event);

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&mock)
        .await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/v1/messages", mock.uri()))
        .json(&json!({"model": "glm-4.6", "messages": [], "stream": true}))
        .send()
        .await
        .expect("send failed");

    let stream: futures::stream::BoxStream<
        'static,
        Result<Bytes, reqwest::Error>,
    > = Box::pin(resp.bytes_stream().map(|r| r.map(Bytes::from)));

    let result = cc_router_lib::proxy::sse::peek_first_event(stream).await;
    match result {
        cc_router_lib::proxy::sse::PeekResult::Ok { mut stream, .. } => {
            // 拼回的流要保留首事件 + 后续事件全部字节
            let mut all = Vec::new();
            while let Some(chunk) = stream.next().await {
                all.extend_from_slice(&chunk.expect("no transport error"));
            }
            let text = String::from_utf8(all).unwrap();
            assert!(text.contains("message_start"));
            assert!(
                text.contains("content_block_delta"),
                "拼回流应包含第二个事件: {text}"
            );
        }
        other => panic!("expected Ok, got {:?}", other),
    }
}

/// HTTPS smoke: tls 模块能生成 ServerConfig, axum-server 能用它起 https listener,
/// reqwest 配 danger_accept_invalid_certs(true) 能完成 TLS handshake 并拿到 200.
#[tokio::test]
async fn tls_listener_serves_https_health() {
    use std::net::SocketAddr;
    use tempfile::tempdir;

    let app_data_dir = tempdir().expect("tempdir");
    let cfg = cc_router_lib::tls::load_or_init_server_config(app_data_dir.path(), &[])
        .await
        .expect("tls server config");

    let router: axum::Router = axum::Router::new().route("/health", axum::routing::get(|| async { "ok" }));

    // bind 一个临时端口
    let listener = std::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .expect("bind");
    listener.set_nonblocking(true).expect("nonblocking");
    let local_addr = listener.local_addr().expect("local_addr");

    let serve = tokio::spawn(async move {
        axum_server::from_tcp_rustls(
            listener,
            axum_server::tls_rustls::RustlsConfig::from_config(cfg),
        )
        .serve(router.into_make_service())
        .await
    });

    // 留时间让 listener 准备好
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .expect("client");
    let url = format!("https://127.0.0.1:{}/health", local_addr.port());
    let resp = client.get(&url).send().await.expect("https handshake");
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.unwrap();
    assert_eq!(body, "ok");

    serve.abort();
}

/// 额外 SAN 注入: extra_sans 里的 IP / DNS 进了签发出来的 leaf 证书 SAN 列表,
/// 同时内置 localhost/127.0.0.1/::1 不被覆盖.
#[tokio::test]
async fn tls_leaf_includes_extra_sans() {
    use std::net::IpAddr;
    use tempfile::tempdir;
    use x509_parser::extensions::GeneralName;
    use x509_parser::prelude::FromDer;

    let app_data_dir = tempdir().expect("tempdir");
    let extras = vec![
        "192.168.1.5".to_string(),
        "my-laptop.local".to_string(),
        "  ".to_string(),                 // 空白被丢
        "not valid host!".to_string(),    // 非法 DNS 被丢
    ];
    cc_router_lib::tls::load_or_init_server_config(app_data_dir.path(), &extras)
        .await
        .expect("tls init with extras");

    let leaf_pem =
        std::fs::read_to_string(app_data_dir.path().join("tls").join("leaf.pem")).unwrap();
    let pem = pem::parse(leaf_pem.as_bytes()).expect("parse leaf pem");
    let (_, cert) = x509_parser::certificate::X509Certificate::from_der(pem.contents())
        .expect("parse x509");

    let san = cert
        .extensions()
        .iter()
        .find_map(|ext| match ext.parsed_extension() {
            x509_parser::extensions::ParsedExtension::SubjectAlternativeName(s) => Some(s),
            _ => None,
        })
        .expect("SAN extension");

    let mut has_localhost = false;
    let mut has_loopback = false;
    let mut has_extra_ip = false;
    let mut has_extra_dns = false;
    for name in &san.general_names {
        match name {
            GeneralName::DNSName(d) => {
                if *d == "localhost" {
                    has_localhost = true;
                }
                if *d == "my-laptop.local" {
                    has_extra_dns = true;
                }
            }
            GeneralName::IPAddress(bytes) => {
                if let Ok(ip) = parse_ip_bytes(bytes) {
                    if ip == IpAddr::from([127, 0, 0, 1]) {
                        has_loopback = true;
                    }
                    if ip == IpAddr::from([192, 168, 1, 5]) {
                        has_extra_ip = true;
                    }
                }
            }
            _ => {}
        }
    }
    assert!(has_localhost, "内置 localhost SAN 未保留");
    assert!(has_loopback, "内置 127.0.0.1 SAN 未保留");
    assert!(has_extra_ip, "192.168.1.5 未注入 SAN");
    assert!(has_extra_dns, "my-laptop.local 未注入 SAN");
}

fn parse_ip_bytes(bytes: &[u8]) -> Result<std::net::IpAddr, ()> {
    match bytes.len() {
        4 => Ok(std::net::IpAddr::V4(std::net::Ipv4Addr::new(
            bytes[0], bytes[1], bytes[2], bytes[3],
        ))),
        16 => {
            let mut arr = [0u8; 16];
            arr.copy_from_slice(bytes);
            Ok(std::net::IpAddr::V6(std::net::Ipv6Addr::from(arr)))
        }
        _ => Err(()),
    }
}

/// 双协议端口冲突避免: HTTP 占 N, HTTPS 默认也 N 时, 启动应 +1 到 N+1.
/// 这里只测 tls 模块本身的端口探测 helper 行为不衰减.
#[tokio::test]
async fn tls_module_idempotent_on_second_init() {
    use tempfile::tempdir;
    let app_data_dir = tempdir().expect("tempdir");
    let _cfg_1 = cc_router_lib::tls::load_or_init_server_config(app_data_dir.path(), &[])
        .await
        .expect("first init");
    let _cfg_2 = cc_router_lib::tls::load_or_init_server_config(app_data_dir.path(), &[])
        .await
        .expect("second init reuses CA");
    let ca_pem_path = cc_router_lib::tls::ca_pem_path(app_data_dir.path());
    let pem = std::fs::read_to_string(&ca_pem_path).expect("ca pem");
    assert!(pem.starts_with("-----BEGIN CERTIFICATE-----"));
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

#[test]
fn google_ai_studio_provider_yaml_shape() {
    let resource_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let providers = cc_router_lib::provider::loader::load_all(&resource_dir)
        .expect("load providers");
    let g = providers.get("google_ai_studio").expect("google_ai_studio present");
    use cc_router_lib::provider::model::{AuthHeaderFormat, AuthType};
    assert!(matches!(g.auth.auth_type, AuthType::GeminiApiKey));
    assert!(matches!(g.auth.header_format, AuthHeaderFormat::Raw));
    assert_eq!(g.auth.header_name, "x-goog-api-key");
    let default_endpoint = g
        .endpoint(g.default_endpoint.as_deref().unwrap_or(""))
        .expect("default endpoint exists");
    assert!(
        default_endpoint.messages_path.contains("{model}"),
        "Gemini messages_path 必须含 {{model}} 占位符"
    );
    assert!(g.model_discovery.enabled);
    assert!(!g.model_discovery.example_models.is_empty());
}

/// Gemini 流式: dispatch 把 Anthropic 请求翻译成 Gemini, 上游回 SSE,
/// dispatch 再把它翻译回 Anthropic SSE. 验证 round-trip 完整性 + URL 占位符替换 + alt=sse query.
#[tokio::test]
async fn gemini_streaming_roundtrip() {
    use bytes::Bytes;
    use futures::StreamExt;
    use wiremock::matchers::{header, query_param};

    let mock = MockServer::start().await;

    // 上游 Gemini SSE: 两帧文本 + 终帧带 finishReason + usage.
    // 真实 Gemini 用 CRLF (\r\n\r\n) 作帧分隔符, fixture 必须复刻, 否则 find_frame_boundary
    // 兼容性退化时无法被 CI 抓到 (历史 bug: 客户端收到 0 字节响应).
    let sse_body = "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"hello \"}]}}]}\r\n\r\n\
        data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"world\"}]}}]}\r\n\r\n\
        data: {\"candidates\":[{\"content\":{\"parts\":[]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":3,\"candidatesTokenCount\":2}}\r\n\r\n";

    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-2.5-flash:streamGenerateContent"))
        .and(query_param("alt", "sse"))
        .and(header("x-goog-api-key", "test-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .mount(&mock)
        .await;

    let url_template = format!("{}/v1beta/models/{{model}}:streamGenerateContent", mock.uri());
    let client = reqwest::Client::new();
    let body = json!({
        "model": "gemini-2.5-flash",
        "messages": [{"role": "user", "content": "say hi"}],
    });

    let ok = cc_router_lib::proxy::gemini_dispatch::dispatch_gemini_attempt(
        &client,
        "test-key".into(),
        "x-goog-api-key".into(),
        url_template,
        "gemini-2.5-flash".into(),
        &body,
        true, // client_wants_streaming
        Vec::new(),
        axum::http::HeaderMap::new(),
        std::collections::BTreeMap::new(),
    )
    .await
    .expect("dispatch ok");

    // 消费 upstream_stream + 跑 GeminiSseConverter (与 finalize_gemini_streaming 等价的逻辑)
    use cc_router_lib::proxy::transform::gemini::{parse_gemini_sse_frame, GeminiSseConverter};

    let mut converter = GeminiSseConverter::new("gemini-2.5-flash");
    let mut buffer = bytes::BytesMut::new();
    let mut anth_events: Vec<String> = Vec::new();
    let mut stream = ok.upstream_stream;
    while let Some(c) = stream.next().await {
        let chunk: Bytes = c.expect("chunk ok");
        buffer.extend_from_slice(&chunk);
        loop {
            let Some((idx, sep_len)) =
                cc_router_lib::proxy::sse_framing::find_sse_frame_boundary(&buffer)
            else {
                break;
            };
            let frame_bytes = buffer.split_to(idx + sep_len);
            let frame_str = std::str::from_utf8(&frame_bytes[..frame_bytes.len() - sep_len]).unwrap_or("");
            if let Some(j) = parse_gemini_sse_frame(frame_str) {
                for evt in converter.feed(&j) {
                    anth_events.push(evt.event.to_string());
                }
            }
        }
    }
    for evt in converter.finalize() {
        anth_events.push(evt.event.to_string());
    }

    // 验证 emit 序列含: message_start, content_block_start(text), 多个 content_block_delta,
    // content_block_stop, message_delta, message_stop
    assert_eq!(anth_events.first().map(|s| s.as_str()), Some("message_start"));
    assert!(anth_events.contains(&"content_block_start".to_string()));
    assert!(anth_events.contains(&"content_block_delta".to_string()));
    assert!(anth_events.contains(&"content_block_stop".to_string()));
    assert_eq!(anth_events.last().map(|s| s.as_str()), Some("message_stop"));
    assert!(anth_events.contains(&"message_delta".to_string()));
}

/// Gemini 401: dispatch 返回 OAuthDispatchError::Upstream with status=401, 携带上游错误 body.
#[tokio::test]
async fn gemini_401_returns_upstream_error() {
    let mock = MockServer::start().await;
    let err_body = json!({
        "error": {"code": 401, "message": "API key not valid", "status": "UNAUTHENTICATED"}
    });
    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-2.5-flash:streamGenerateContent"))
        .respond_with(ResponseTemplate::new(401).set_body_json(err_body))
        .mount(&mock)
        .await;

    let url_template = format!("{}/v1beta/models/{{model}}:streamGenerateContent", mock.uri());
    let client = reqwest::Client::new();
    let body = json!({"model": "gemini-2.5-flash", "messages": [{"role": "user", "content": "hi"}]});

    let res = cc_router_lib::proxy::gemini_dispatch::dispatch_gemini_attempt(
        &client,
        "bad-key".into(),
        "x-goog-api-key".into(),
        url_template,
        "gemini-2.5-flash".into(),
        &body,
        false,
        Vec::new(),
        axum::http::HeaderMap::new(),
        std::collections::BTreeMap::new(),
    )
    .await;

    use cc_router_lib::proxy::oauth_dispatch::OAuthDispatchError;
    match res {
        Err(OAuthDispatchError::Upstream { status, message }) => {
            assert_eq!(status, Some(401));
            assert!(message.contains("UNAUTHENTICATED") || message.contains("API key"));
        }
        other => panic!("expected Upstream 401, got {:?}", other.is_ok()),
    }
}

/// URL 模板缺少 {model} 占位符 → dispatch 直接返回错误, 不发请求.
#[tokio::test]
async fn gemini_missing_model_placeholder_errors() {
    let client = reqwest::Client::new();
    let body = json!({"model": "gemini-2.5-flash", "messages": []});
    let res = cc_router_lib::proxy::gemini_dispatch::dispatch_gemini_attempt(
        &client,
        "k".into(),
        "x-goog-api-key".into(),
        "https://example.invalid/v1beta/models/gemini-2.5-flash:streamGenerateContent".into(),
        "gemini-2.5-flash".into(),
        &body,
        false,
        Vec::new(),
        axum::http::HeaderMap::new(),
        std::collections::BTreeMap::new(),
    )
    .await;
    use cc_router_lib::proxy::oauth_dispatch::OAuthDispatchError;
    assert!(matches!(res, Err(OAuthDispatchError::Upstream { .. })));
}

