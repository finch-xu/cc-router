//! Local socket tests: compression → production client → JSON/SSE consumers.
use super::{peek_first_event, process_event, PeekResult};
use crate::proxy::sse_framing::find_sse_frame_boundary;
use crate::proxy::transform::openai_chat_completions::{
    ChatCompletionsSseConverter, ChatCompletionsTransformConfig,
};
use crate::proxy::{header_log, header_source::HeaderSource, upstream};
use bytes::BytesMut;
use futures::StreamExt;
use reqwest::header::HeaderMap;
use std::{future::Future, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

const CODECS: [&str; 4] = ["gzip", "deflate", "br", "zstd"];
const DEADLINE: Duration = Duration::from_secs(5);

// Flush each input part inside ONE compression stream. In particular, this is
// not a concatenation of independently finished gzip/zstd frames.
async fn encode(codec: &str, parts: &[&[u8]]) -> Vec<Vec<u8>> {
    use async_compression::tokio::write::*;
    macro_rules! encoded {
        ($encoder:ident) => {{
            let mut encoder = $encoder::new(Vec::new());
            let mut chunks = Vec::new();
            let mut offset = 0;
            for part in parts {
                encoder.write_all(part).await.unwrap();
                encoder.flush().await.unwrap();
                chunks.push(encoder.get_ref()[offset..].to_vec());
                offset = encoder.get_ref().len();
            }
            encoder.shutdown().await.unwrap();
            chunks.push(encoder.get_ref()[offset..].to_vec());
            chunks
        }};
    }
    match codec {
        "gzip" => encoded!(GzipEncoder),
        "deflate" => encoded!(ZlibEncoder), // HTTP deflate is zlib-wrapped.
        "br" => encoded!(BrotliEncoder),
        "zstd" => encoded!(ZstdEncoder),
        _ => panic!("unknown codec"),
    }
}

async fn serve<F, Fut>(respond: F) -> (String, tokio::task::JoinHandle<HeaderMap>)
where
    F: FnOnce(TcpStream) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = Vec::new();
        let head_end = loop {
            let mut chunk = [0; 4096];
            let n = socket.read(&mut chunk).await.unwrap();
            assert_ne!(n, 0);
            buf.extend_from_slice(&chunk[..n]);
            if let Some(i) = buf.windows(4).position(|b| b == b"\r\n\r\n") {
                break i + 4;
            }
            assert!(buf.len() < 32 * 1024);
        };
        let mut headers = HeaderMap::new();
        for line in std::str::from_utf8(&buf[..head_end])
            .unwrap()
            .lines()
            .skip(1)
        {
            if let Some((name, value)) = line.split_once(':') {
                headers.append(
                    reqwest::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                    value.trim().parse().unwrap(),
                );
            }
        }
        let len: usize = headers["content-length"].to_str().unwrap().parse().unwrap();
        while buf.len() < head_end + len {
            let mut chunk = [0; 4096];
            let n = socket.read(&mut chunk).await.unwrap();
            assert_ne!(n, 0);
            buf.extend_from_slice(&chunk[..n]);
        }
        assert_eq!(&buf[head_end..head_end + len], b"{}");
        assert!(
            !headers.contains_key("content-encoding"),
            "request body stays uncompressed"
        );
        respond(socket).await;
        headers
    });
    (url, task)
}

async fn fixed(
    codec: Option<&'static str>,
    status: u16,
    body: Vec<u8>,
) -> (String, tokio::task::JoinHandle<HeaderMap>) {
    serve(move |mut socket| async move {
        let encoding = codec.map(|c| format!("Content-Encoding: {c}\r\n")).unwrap_or_default();
        let head = format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\n{encoding}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        socket.write_all(head.as_bytes()).await.unwrap();
        socket.write_all(&body).await.unwrap();
    }).await
}

async fn chunk(socket: &mut TcpStream, bytes: &[u8]) {
    // A zero-length HTTP chunk ends the response; a compressor flush can be empty.
    if bytes.is_empty() {
        return;
    }
    socket
        .write_all(format!("{:x}\r\n", bytes.len()).as_bytes())
        .await
        .unwrap();
    socket.write_all(bytes).await.unwrap();
    socket.write_all(b"\r\n").await.unwrap();
}

async fn gated_sse(
    codec: &'static str,
    parts: Vec<Vec<u8>>,
    gate_after: usize,
    finish: bool,
) -> (
    String,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<HeaderMap>,
) {
    let (release, wait) = oneshot::channel();
    let (url, task) = serve(move |mut socket| async move {
        let head = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Encoding: {codec}\r\nTransfer-Encoding: chunked\r\n\r\n");
        socket.write_all(head.as_bytes()).await.unwrap();
        for part in &parts[..gate_after] { chunk(&mut socket, part).await; }
        if wait.await.is_err() { return; }
        for part in &parts[gate_after..] { chunk(&mut socket, part).await; }
        if finish { socket.write_all(b"0\r\n\r\n").await.unwrap(); }
    }).await;
    (url, release, task)
}

fn client() -> reqwest::Client {
    upstream::build_client(DEADLINE).unwrap()
}

async fn send(url: &str, headers: HeaderMap, streaming: bool) -> upstream::UpstreamResponse {
    upstream::send(&client(), url, b"{}".to_vec(), headers, streaming)
        .await
        .unwrap()
}

#[tokio::test]
async fn compression_json_and_error_responses() {
    let expected = serde_json::json!({"text":"你好 🌍", "nested":{"items":[1,true,null]}, "error":{"message":"限额已到"}});
    for codec in CODECS {
        for status in [200, 400, 429, 500] {
            let raw = serde_json::to_vec(&expected).unwrap();
            let body = encode(codec, &[&raw]).await.concat();
            let (url, server) = fixed(Some(codec), status, body).await;
            let upstream::UpstreamResponse::NonStreaming {
                status: got_status,
                headers,
                body,
                body_text,
            } = send(&url, HeaderMap::new(), false).await
            else {
                panic!("JSON expected")
            };
            assert_eq!(got_status.as_u16(), status, "{codec}");
            assert_eq!(body, expected, "{codec}");
            assert!(!headers.contains_key("content-encoding"), "{codec}");
            assert!(!headers.contains_key("content-length"), "{codec}");
            assert_eq!(
                body_text,
                (status != 200).then(|| String::from_utf8(raw).unwrap())
            );
            server.await.unwrap();
        }
    }
}

#[tokio::test]
async fn compression_negotiation_matches_logs_and_preview() {
    // Range is intentionally covered: reqwest 0.13's tower-http layer adds
    // Accept-Encoding even with Range, unlike the old reqwest implementation.
    for explicit in [
        None,
        Some("identity"),
        Some("gzip"),
        Some("zstd,gzip,deflate,br"),
    ] {
        for range in [false, true] {
            let mut headers = HeaderMap::new();
            if let Some(v) = explicit {
                headers.insert("accept-encoding", v.parse().unwrap());
            }
            if range {
                headers.insert("range", "bytes=0-10".parse().unwrap());
            }
            let (url, server) = fixed(None, 200, b"{}".to_vec()).await;
            send(&url, headers.clone(), false).await;
            let wire = server.await.unwrap();
            let actual = wire
                .get("accept-encoding")
                .expect("automatic negotiation")
                .to_str()
                .unwrap();
            if let Some(v) = explicit {
                assert_eq!(actual, v);
            } else {
                let mut encodings: Vec<_> = actual.split(',').map(str::trim).collect();
                encodings.sort_unstable();
                assert_eq!(encodings, ["br", "deflate", "gzip", "zstd"]);
            }
            let tagged: Vec<_> = headers
                .iter()
                .map(|(k, v)| {
                    (
                        k.to_string(),
                        v.to_str().unwrap().to_string(),
                        HeaderSource::Custom,
                    )
                })
                .collect();
            let hints = header_log::SourceHints {
                custom_names: tagged.iter().map(|h| h.0.clone()).collect(),
                ..Default::default()
            };
            let logs = [
                header_log::serialize_tagged(&tagged, true, &[]).unwrap(),
                header_log::tag_capture(
                    header_log::capture_untagged(&headers, true, &[]).unwrap(),
                    &hints,
                    &HeaderMap::new(),
                )
                .unwrap(),
            ];
            for log in logs {
                let rows: Vec<[String; 3]> = serde_json::from_str(&log).unwrap();
                let encoding: Vec<_> = rows.iter().filter(|r| r[0] == "accept-encoding").collect();
                assert_eq!(encoding.len(), 1, "{log}");
                assert_eq!(encoding[0][1], actual);
                assert_eq!(
                    encoding[0][2],
                    if explicit.is_some() {
                        HeaderSource::Custom
                    } else {
                        HeaderSource::CcRouter
                    }
                    .as_str()
                );
            }
            let mut row =
                crate::subscription::model::SubscriptionRow::test_fixture("anthropic", "api_paygo");
            row.extra_headers = headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_str().unwrap().to_string()))
                .collect();
            let preview = crate::commands::header_preview::preview(
                &row,
                &crate::subscription::header_policy::HeaderPolicy::none(),
            );
            let shown: Vec<_> = preview
                .sent
                .iter()
                .filter(|h| h.name == "accept-encoding")
                .collect();
            assert_eq!(shown.len(), 1);
            assert_eq!(shown[0].value, actual);
            assert_eq!(
                shown[0].source,
                if explicit.is_some() {
                    HeaderSource::Custom
                } else {
                    HeaderSource::CcRouter
                }
            );
        }
    }
}

#[tokio::test]
async fn compression_sse_is_incremental_and_preserves_parsing() {
    let first = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"消息\",\"model\":\"real-model\",\"usage\":{\"input_tokens\":7}}}\n\n";
    let rest = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"你好 🌍\"}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":9}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
    let split = first.find('消').unwrap() + 1; // inside a UTF-8 character
    for codec in CODECS {
        let parts = encode(
            codec,
            &[
                &first.as_bytes()[..split],
                &first.as_bytes()[split..],
                rest.as_bytes(),
            ],
        )
        .await;
        let (url, release, server) = gated_sse(codec, parts, 2, true).await;
        let upstream::UpstreamResponse::Streaming {
            stream, headers, ..
        } = send(&url, HeaderMap::new(), true).await
        else {
            panic!("SSE expected")
        };
        assert!(!headers.contains_key("content-encoding"));
        let peek = tokio::time::timeout(DEADLINE, peek_first_event(stream))
            .await
            .expect("first event must arrive before upstream finishes");
        let PeekResult::Ok { mut stream, .. } = peek else {
            panic!("{codec}: {peek:?}")
        };
        let first_bytes = stream.next().await.unwrap().unwrap();
        assert_eq!(first_bytes.as_ref(), first.as_bytes(), "{codec}");
        let (rewritten, meta) = process_event(&first_bytes, Some("model-sonnet"));
        let text = std::str::from_utf8(&rewritten).unwrap();
        assert!(text.contains("model-sonnet") && text.contains("消息"));
        assert_eq!(meta.unwrap().input_tokens, Some(7));
        assert!(
            !server.is_finished(),
            "server is still waiting to send the rest"
        );
        release.send(()).unwrap();
        let mut buffer = BytesMut::new();
        let mut output = Vec::new();
        let mut output_tokens = None;
        while let Some(part) = stream.next().await {
            buffer.extend_from_slice(&part.unwrap());
            while let Some((i, len)) = find_sse_frame_boundary(&buffer) {
                let (bytes, meta) = process_event(&buffer.split_to(i + len), Some("model-sonnet"));
                if let Some(meta) = meta {
                    output_tokens = meta.output_tokens.or(output_tokens);
                }
                output.extend_from_slice(&bytes);
            }
        }
        assert!(buffer.is_empty());
        assert_eq!(output, rest.as_bytes(), "{codec}");
        assert_eq!(output_tokens, Some(9));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn compression_openai_sse_translation_is_incremental() {
    let first = "data: {\"id\":\"chat-1\",\"model\":\"real-model\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"你好\"},\"finish_reason\":null}]}\r\n\r\n";
    let rest = "data: {\"id\":\"chat-1\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":9}}\r\n\r\ndata: [DONE]\r\n\r\n";
    for codec in CODECS {
        let parts = encode(codec, &[first.as_bytes(), rest.as_bytes()]).await;
        let (url, release, server) = gated_sse(codec, parts, 1, true).await;
        let upstream::UpstreamResponse::Streaming { mut stream, .. } =
            send(&url, HeaderMap::new(), true).await
        else {
            panic!("SSE expected")
        };
        let mut converter = ChatCompletionsSseConverter::new(
            ChatCompletionsTransformConfig::permissive(),
            "fallback-id".into(),
            "model-sonnet".into(),
        );
        let mut buffer = BytesMut::new();
        let mut output = String::new();
        tokio::time::timeout(DEADLINE, async {
            while !output.contains("你好") {
                buffer.extend_from_slice(&stream.next().await.unwrap().unwrap());
                while let Some((i, len)) = find_sse_frame_boundary(&buffer) {
                    let frame = buffer.split_to(i + len);
                    output.push_str(
                        &converter
                            .ingest(std::str::from_utf8(&frame).unwrap())
                            .concat(),
                    );
                }
            }
        })
        .await
        .expect("translated text must arrive before upstream finishes");
        assert!(
            output.contains("message_start") && output.contains("text_delta"),
            "{codec}: {output}"
        );
        release.send(()).unwrap();
        while let Some(part) = stream.next().await {
            buffer.extend_from_slice(&part.unwrap());
            while let Some((i, len)) = find_sse_frame_boundary(&buffer) {
                let frame = buffer.split_to(i + len);
                output.push_str(
                    &converter
                        .ingest(std::str::from_utf8(&frame).unwrap())
                        .concat(),
                );
            }
        }
        output.push_str(&converter.finish().concat());
        assert!(buffer.is_empty());
        assert!(output.contains("message_stop"));
        assert_eq!(converter.final_output_tokens(), Some(9));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn compression_corrupt_json_is_an_error() {
    for codec in CODECS {
        let (url, server) = fixed(Some(codec), 200, b"invalid compressed payload".to_vec()).await;
        assert!(
            upstream::send(&client(), &url, b"{}".to_vec(), HeaderMap::new(), false)
                .await
                .is_err(),
            "{codec}"
        );
        server.await.unwrap();
    }
}

#[tokio::test]
async fn compression_interrupted_sse_returns_an_error() {
    for codec in CODECS {
        let parts = encode(codec, &[b"data: {}\n\n"]).await;
        let (url, release, server) = gated_sse(codec, vec![parts[0].clone()], 1, false).await;
        let upstream::UpstreamResponse::Streaming { mut stream, .. } =
            send(&url, HeaderMap::new(), true).await
        else {
            panic!("SSE expected")
        };
        assert_eq!(
            stream.next().await.unwrap().unwrap().as_ref(),
            b"data: {}\n\n"
        );
        release.send(()).unwrap();
        let mut error = false;
        while let Some(part) = stream.next().await {
            error |= part.is_err();
        }
        assert!(
            error,
            "{codec}: truncated stream must not be successful EOF"
        );
        server.await.unwrap();
    }
}

#[tokio::test]
async fn compression_truncated_sse_with_complete_http_framing_is_an_error() {
    for codec in CODECS {
        let mut body = encode(codec, &[b"data: {}\n\n"]).await.concat();
        body.pop(); // Missing compression trailer, but HTTP Content-Length is satisfied.
        let (url, server) = serve(move |mut socket| async move {
            let head = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Encoding: {codec}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            socket.write_all(head.as_bytes()).await.unwrap();
            socket.write_all(&body).await.unwrap();
        }).await;
        let upstream::UpstreamResponse::Streaming { mut stream, .. } =
            send(&url, HeaderMap::new(), true).await
        else {
            panic!("SSE expected")
        };
        let mut error = false;
        while let Some(part) = stream.next().await {
            error |= part.is_err();
        }
        assert!(
            error,
            "{codec}: incomplete compression must not be successful EOF"
        );
        server.await.unwrap();
    }
}

#[tokio::test]
async fn compression_silent_sse_retains_idle_timeout() {
    for codec in CODECS {
        let parts = encode(codec, &[b"data: {}\n\n"]).await;
        let (url, release, server) = gated_sse(codec, parts, 1, true).await;
        let client = upstream::build_client(Duration::from_millis(300)).unwrap();
        let upstream::UpstreamResponse::Streaming { mut stream, .. } =
            upstream::send(&client, &url, b"{}".to_vec(), HeaderMap::new(), true)
                .await
                .unwrap()
        else {
            panic!("SSE expected")
        };
        assert_eq!(
            stream.next().await.unwrap().unwrap().as_ref(),
            b"data: {}\n\n"
        );
        let err = stream
            .next()
            .await
            .unwrap()
            .expect_err("silent upstream must time out");
        assert!(err.is_timeout(), "{codec}: {err:?}");
        drop(release);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn compression_preserves_archive_bytes_and_plain_json() {
    // A .gz artifact is NOT HTTP Content-Encoding. Its signed bytes must survive.
    let archive = encode("gzip", &[b"signed archive payload"]).await.concat();
    for codec in [
        None,
        Some("gzip"),
        Some("deflate"),
        Some("br"),
        Some("zstd"),
    ] {
        let body = match codec {
            Some(c) => encode(c, &[&archive]).await.concat(),
            None => archive.clone(),
        };
        let (url, server) = fixed(codec, 200, body).await;
        let bytes = client()
            .post(&url)
            .body("{}")
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(bytes.as_ref(), archive.as_slice(), "{codec:?}");
        server.await.unwrap();
    }
    let (url, server) = fixed(None, 200, "{\"text\":\"未压缩\"}".as_bytes().to_vec()).await;
    let upstream::UpstreamResponse::NonStreaming { body, .. } =
        send(&url, HeaderMap::new(), false).await
    else {
        panic!("JSON expected")
    };
    assert_eq!(body["text"], "未压缩");
    server.await.unwrap();
}
