use axum::http::StatusCode;
use bytes::Bytes;
use futures::stream::BoxStream;
use reqwest::header::HeaderMap as ReqHeaderMap;
use serde_json::Value;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum UpstreamError {
    #[error("{}", describe_error(.0))]
    Reqwest(#[from] reqwest::Error),
}

pub enum UpstreamResponse {
    NonStreaming {
        status: StatusCode,
        headers: ReqHeaderMap,
        body: Value,
        /// 原始响应文本。仅在非 2xx 时填充, 用于错误路径诊断展示;
        /// 成功路径不保留, 避免每个响应多 hold 一份 KB 级 String。
        body_text: Option<String>,
    },
    Streaming {
        status: StatusCode,
        headers: ReqHeaderMap,
        stream: BoxStream<'static, Result<Bytes, reqwest::Error>>,
    },
}

/// Idle limit of the production upstream client (see [`build_client`]).
pub const UPSTREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(600);

/// The production upstream client.
///
/// `idle` is a read timeout, not a total one: it covers waiting for the response head and
/// every body read, and resets whenever bytes arrive. A total timeout cut long but healthy
/// streams at a fixed wall-clock cliff (issue #59); every request that fit in a total of
/// `idle` still fits here, so this only ever loosens. Streams the client abandons are not
/// leaked: every stream loop stops reading upstream once `client_tx.send` fails.
pub fn build_client(idle: Duration) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(idle)
        .user_agent(concat!("cc-router/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// User-facing text for an upstream network error. reqwest's own Display for a body read
/// timeout is the opaque "error decoding response body". No number in the text: the probe
/// client (30s total) reaches the same call sites.
pub fn describe_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        return if e.is_connect() { "连接上游超时".into() } else { "等待上游数据超时, 连接已断开".into() };
    }
    e.to_string()
}

pub async fn send(
    client: &reqwest::Client,
    url: &str,
    body: Vec<u8>,
    headers: ReqHeaderMap,
    is_streaming: bool,
) -> Result<UpstreamResponse, UpstreamError> {
    let resp = client
        .post(url)
        .headers(headers)
        .body(body)
        .send()
        .await?;

    let status = StatusCode::from_u16(resp.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let resp_headers = resp.headers().clone();

    if is_streaming && status.is_success() {
        Ok(UpstreamResponse::Streaming {
            status,
            headers: resp_headers,
            stream: Box::pin(resp.bytes_stream()),
        })
    } else {
        // 非流式：完整读取
        let text = resp.text().await?;
        let body = serde_json::from_str::<Value>(&text)
            .unwrap_or_else(|_| serde_json::json!({ "raw": text.clone() }));
        let body_text = if status.is_success() { None } else { Some(text) };
        Ok(UpstreamResponse::NonStreaming {
            status,
            headers: resp_headers,
            body,
            body_text,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn minimax_catalog_models_reach_native_messages_over_http() {
        use crate::provider::{loader::load_all, model::{AuthType, EndpointProtocol}};
        use crate::subscription::{model::SubscriptionRow, snapshot::snapshot_connection};
        use crate::proxy::{forward::build_anthropic_passthrough_headers, sanitize::strip_foreign_thinking_blocks};
        use serde_json::json;
        use std::collections::BTreeMap;
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::{method, path, header}};

        let providers = load_all().unwrap();
        let provider = &providers["minimax"];
        assert_eq!(provider.auth.auth_type, AuthType::ApiKey);
        assert_eq!(provider.default_endpoint.as_deref(), Some("china_paygo"));
        assert_eq!(provider.model_discovery.example_models, ["MiniMax-M3.1-Flash", "MiniMax-M3"]);
        for (endpoint_id, origin) in [
            ("china_paygo", "https://api.minimaxi.com"),
            ("global_paygo", "https://api.minimax.io"),
        ] {
            let endpoint = provider.endpoint(endpoint_id).unwrap();
            let snapshot = snapshot_connection(provider, endpoint, &BTreeMap::new()).unwrap();
            assert_eq!(snapshot.endpoint_protocol, EndpointProtocol::Messages);
            let mut row = SubscriptionRow::test_fixture("minimax", endpoint_id);
            row.base_url = snapshot.base_url;
            row.messages_path = snapshot.messages_path;
            row.required_headers = snapshot.required_headers;
            row.auth_header_name = provider.auth.header_name.clone();
            row.auth_header_format = provider.auth.header_format.clone();
            row.model_slots.sonnet.clear();
            row.model_slots.haiku.clear();
            row.model_slots.opus.clear();
            row.model_discovery.example_models = vec!["MiniMax-M2.7".into()];
            assert_eq!(crate::subscription::ping::pick_test_model(&row, &providers).unwrap().0, "MiniMax-M3.1-Flash");
            assert_eq!(row.messages_url(), format!("{origin}/anthropic/v1/messages"));

            // Only the origin is redirected to loopback. Path, auth, model and JSON
            // use the same production helpers as the native Messages branch.
            let server = MockServer::start().await;
            row.base_url = format!("{}/anthropic", server.uri());
            Mock::given(method("POST"))
                .and(path("/anthropic/v1/messages"))
                .and(header("authorization", "Bearer test-key"))
                .and(header("anthropic-version", "2023-06-01"))
                .and(header("content-type", "application/json"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"type": "message", "content": []})))
                .expect(6)
                .mount(&server).await;
            let client = build_client(Duration::from_secs(5)).unwrap();
            for model in snapshot.model_discovery.example_models.iter().map(String::as_str)
                .chain(["MiniMax-M2.7"]) {
                row.model_slots.sonnet = if model == "MiniMax-M3.1-Flash" { String::new() } else { model.into() };
                row.is_user_defined = model == "MiniMax-M2.7";
                let (selected, _) = crate::subscription::ping::pick_test_model(&row, &providers).unwrap();
                assert_eq!(selected, model);
                for streaming in [false, true] {
                    let mut body = json!({
                        "model": selected, "max_tokens": 1024, "stream": streaming,
                        "thinking": {"type": "adaptive"},
                        "output_config": {"effort": "max"},
                        "tools": [{"name": "lookup", "input_schema": {"type": "object"}}],
                        "messages": [
                            {"role": "user", "content": "Look up the answer"},
                            {"role": "assistant", "content": [
                                {"type": "thinking", "thinking": "Need a lookup", "signature": "native-signature"},
                                {"type": "tool_use", "id": "call_1", "name": "lookup", "input": {}}
                            ]},
                            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "42"}]}
                        ]
                    });
                    let expected = body.clone();
                    assert_eq!(strip_foreign_thinking_blocks(&mut body, false), (0, 0));
                    let headers = build_anthropic_passthrough_headers(
                        &axum::http::HeaderMap::new(), false, &provider.forward_headers,
                        &row.resolved_required_headers(), &row.auth_header_name, &row.auth_header_value(),
                    );
                    let response = send(&client, &row.messages_url(), serde_json::to_vec(&body).unwrap(), headers, streaming).await.unwrap();
                    match response {
                        UpstreamResponse::NonStreaming { status, .. } => assert!(status.is_success()),
                        UpstreamResponse::Streaming { status, mut stream, .. } => {
                            assert!(status.is_success());
                            while let Some(chunk) = stream.next().await { chunk.unwrap(); }
                        }
                    }
                    let requests = server.received_requests().await.unwrap();
                    let sent: Value = requests.last().unwrap().body_json().unwrap();
                    assert_eq!(sent, expected);
                    assert!(sent.get("input").is_none());
                    assert!(sent.get("reasoning_split").is_none());
                }
            }
        }
    }

    /// One-shot HTTP/1.1 server: reads the request head, then runs `respond` on the socket.
    async fn serve_once<F, Fut>(respond: F) -> String
    where
        F: FnOnce(tokio::net::TcpStream) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await;
            respond(sock).await;
        });
        format!("http://{addr}/v1/messages")
    }

    const SSE_HEAD: &[u8] =
        b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n";

    async fn write_chunk(sock: &mut tokio::net::TcpStream, data: &[u8]) {
        sock.write_all(format!("{:x}\r\n", data.len()).as_bytes()).await.unwrap();
        sock.write_all(data).await.unwrap();
        sock.write_all(b"\r\n").await.unwrap();
    }

    /// issue #59: a stream that keeps producing data must outlive the idle limit —
    /// only silence ends it, not total duration.
    #[tokio::test]
    async fn steady_stream_outlives_the_idle_limit() {
        let url = serve_once(|mut sock| async move {
            sock.write_all(SSE_HEAD).await.unwrap();
            for _ in 0..8 {
                write_chunk(&mut sock, b"data: {}\n\n").await;
                tokio::time::sleep(Duration::from_millis(60)).await;
            }
            sock.write_all(b"0\r\n\r\n").await.unwrap();
        })
        .await;
        let client = build_client(Duration::from_millis(250)).unwrap();
        let resp = send(&client, &url, b"{}".to_vec(), ReqHeaderMap::new(), true).await.unwrap();
        let UpstreamResponse::Streaming { mut stream, .. } = resp else { panic!("expected streaming") };
        let mut chunks = 0;
        while let Some(c) = stream.next().await {
            c.expect("a stream with steady data must not time out");
            chunks += 1;
        }
        assert!(chunks > 0);
    }

    /// Silence mid-stream ends the stream with a timeout whose message says so
    /// (reqwest's own Display is the opaque "error decoding response body").
    #[tokio::test]
    async fn silent_stream_times_out_with_a_readable_message() {
        let url = serve_once(|mut sock| async move {
            sock.write_all(SSE_HEAD).await.unwrap();
            write_chunk(&mut sock, b"data: {}\n\n").await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        })
        .await;
        let client = build_client(Duration::from_millis(200)).unwrap();
        let resp = send(&client, &url, b"{}".to_vec(), ReqHeaderMap::new(), true).await.unwrap();
        let UpstreamResponse::Streaming { mut stream, .. } = resp else { panic!("expected streaming") };
        stream.next().await.unwrap().unwrap();
        let err = stream.next().await.unwrap().expect_err("silence must time out");
        assert!(err.is_timeout());
        assert_eq!(describe_error(&err), "等待上游数据超时, 连接已断开");
    }

    /// The idle limit also covers waiting for the response head (non-streaming upstreams
    /// send nothing until they are done).
    #[tokio::test]
    async fn silent_response_head_times_out() {
        let url = serve_once(|sock| async move {
            // hold the socket open without answering
            let _keep = sock;
            tokio::time::sleep(Duration::from_secs(5)).await;
        })
        .await;
        let client = build_client(Duration::from_millis(200)).unwrap();
        let Err(UpstreamError::Reqwest(err)) =
            send(&client, &url, b"{}".to_vec(), ReqHeaderMap::new(), false).await
        else {
            panic!("expected a timeout error")
        };
        assert!(err.is_timeout());
        assert_eq!(describe_error(&err), "等待上游数据超时, 连接已断开");
    }
}
