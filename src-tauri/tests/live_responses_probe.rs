//! 真端点 live 验证 — **不入库** (untracked), 需要环境变量:
//!   LIVE_RESPONSES_BASE  e.g. https://example-relay.invalid/v1
//!   LIVE_RESPONSES_KEY   sk-...
//!   LIVE_RESPONSES_MODEL e.g. gpt-5.6-luna
//!
//! 运行: cargo test --test live_responses_probe -- --ignored --nocapture

use bytes::BytesMut;
use futures::StreamExt;
use serde_json::{json, Value};

use cc_router_lib::proxy::openai_responses_dispatch::{
    dispatch_openai_responses_attempt, OpenaiResponsesPayload,
};
use cc_router_lib::proxy::sse_framing::find_sse_frame_boundary;
use cc_router_lib::proxy::transform::openai::OpenAiResponsesExtras;
use cc_router_lib::proxy::transform::openai_responses::parse_sse_frame;
use cc_router_lib::proxy::transform::responses_common::{
    AnthropicEvent, ResponsesSseConverter, ResponsesTransformConfig,
};
use cc_router_lib::provider::model::AuthHeaderFormat;

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("需要环境变量 {name}"))
}

async fn dispatch(body: Value, streaming: bool) -> Result<OpenaiResponsesPayload, String> {
    let result = dispatch_openai_responses_attempt(
        &reqwest::Client::new(),
        env("LIVE_RESPONSES_KEY"),
        "Authorization".into(),
        AuthHeaderFormat::Bearer,
        format!("{}/responses", env("LIVE_RESPONSES_BASE")),
        &body,
        streaming,
        Vec::new(),
        axum::http::HeaderMap::new(),
        std::collections::BTreeMap::new(),
        OpenAiResponsesExtras {
            reasoning_effort: None,
            expose_reasoning: true,
        },
    )
    .await;
    match result {
        Ok(ok) => Ok(ok.payload),
        Err(e) => Err(format!("{e:?}")),
    }
}

/// 把上游流喂给 converter, 返回全部 Anthropic 事件 (复刻 finalize_streaming 的分帧逻辑)
async fn drain(payload: OpenaiResponsesPayload) -> (Vec<AnthropicEvent>, ResponsesSseConverter) {
    let OpenaiResponsesPayload::Streaming(mut stream) = payload else {
        panic!("expected streaming payload");
    };
    let mut converter =
        ResponsesSseConverter::new_with_config(ResponsesTransformConfig::openai_official(true));
    let mut buffer = BytesMut::new();
    let mut events = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.expect("stream chunk");
        buffer.extend_from_slice(&chunk);
        loop {
            let Some((idx, sep_len)) = find_sse_frame_boundary(&buffer) else {
                break;
            };
            let frame = buffer.split_to(idx + sep_len);
            let frame_str = std::str::from_utf8(&frame[..frame.len() - sep_len]).unwrap_or("");
            if let Some((name, data)) = parse_sse_frame(frame_str) {
                events.extend(converter.feed(&name, &data));
            }
        }
    }
    events.extend(converter.finalize_if_needed());
    (events, converter)
}

fn model() -> String {
    env("LIVE_RESPONSES_MODEL")
}

#[tokio::test]
#[ignore]
async fn live_streaming_text_and_usage() {
    let body = json!({
        "model": model(),
        "stream": true,
        "max_tokens": 512,
        "messages": [{"role": "user", "content": "Reply with exactly: OK"}],
    });
    let (events, _conv) = drain(dispatch(body, true).await.expect("dispatch ok")).await;

    let text: String = events
        .iter()
        .filter_map(|e| match e {
            AnthropicEvent::ContentBlockDelta { delta, .. }
                if delta["type"] == "text_delta" =>
            {
                delta["text"].as_str().map(String::from)
            }
            _ => None,
        })
        .collect();
    assert!(!text.is_empty(), "应收到文本");
    let usage = events
        .iter()
        .find_map(|e| match e {
            AnthropicEvent::MessageDelta { usage, .. } => Some(usage.clone()),
            _ => None,
        })
        .expect("应收到 message_delta 携带 usage");
    assert!(usage.get("input_tokens").is_some());
    println!("text={text:?}\nusage={usage}");
}

#[tokio::test]
#[ignore]
async fn live_tool_use_with_forced_choice() {
    let body = json!({
        "model": model(),
        "stream": true,
        "max_tokens": 512,
        "tools": [{
            "name": "get_weather",
            "description": "Get current weather for a city",
            "input_schema": {"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}
        }],
        "tool_choice": {"type": "any"},
        "messages": [{"role": "user", "content": "What's the weather in Tokyo?"}],
    });
    let (events, _) = drain(dispatch(body, true).await.expect("dispatch ok")).await;

    let has_tool_start = events.iter().any(|e| matches!(
        e,
        AnthropicEvent::ContentBlockStart { content_block, .. } if content_block["type"] == "tool_use"
    ));
    let args: String = events
        .iter()
        .filter_map(|e| match e {
            AnthropicEvent::ContentBlockDelta { delta, .. }
                if delta["type"] == "input_json_delta" =>
            {
                delta["partial_json"].as_str().map(String::from)
            }
            _ => None,
        })
        .collect();
    assert!(has_tool_start, "tool_choice any 应产生 tool_use, events: {events:?}");
    serde_json::from_str::<Value>(&args).expect("tool args 应是合法 JSON");
    println!("tool args={args}");
}

#[tokio::test]
#[ignore]
async fn live_image_input() {
    // 1x1 红色 PNG
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC";
    let body = json!({
        "model": model(),
        "stream": true,
        "max_tokens": 512,
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": "What color is this image? Answer with one word."},
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": png}}
            ]
        }],
    });
    let (events, _) = drain(dispatch(body, true).await.expect("dispatch ok")).await;
    let text: String = events
        .iter()
        .filter_map(|e| match e {
            AnthropicEvent::ContentBlockDelta { delta, .. }
                if delta["type"] == "text_delta" =>
            {
                delta["text"].as_str().map(String::from)
            }
            _ => None,
        })
        .collect();
    assert!(!text.is_empty(), "图片输入应得到回答");
    println!("image answer={text:?}");
}

#[tokio::test]
#[ignore]
async fn live_multiturn_thinking_roundtrip() {
    // 第一轮: 拿 thinking block (expose_reasoning=true 时 signature 编码 encrypted_content)
    let body1 = json!({
        "model": model(),
        "stream": true,
        "max_tokens": 1024,
        "messages": [{"role": "user", "content": "What is 17 * 23? Just the number."}],
    });
    let (events, _) = drain(dispatch(body1, true).await.expect("turn1 ok")).await;

    let mut thinking = String::new();
    let mut signature = String::new();
    let mut answer = String::new();
    for e in &events {
        if let AnthropicEvent::ContentBlockDelta { delta, .. } = e {
            match delta["type"].as_str() {
                Some("thinking_delta") => {
                    thinking.push_str(delta["thinking"].as_str().unwrap_or(""))
                }
                Some("signature_delta") => {
                    signature = delta["signature"].as_str().unwrap_or("").to_string()
                }
                Some("text_delta") => answer.push_str(delta["text"].as_str().unwrap_or("")),
                _ => {}
            }
        }
    }
    println!("turn1 answer={answer:?} thinking_len={} sig_len={}", thinking.len(), signature.len());

    // 第二轮: 回灌 assistant thinking block (中转站惯例检查: 不能 400)
    let mut assistant_content = vec![];
    if !signature.is_empty() {
        assistant_content.push(json!({
            "type": "thinking", "thinking": thinking, "signature": signature,
        }));
    }
    assistant_content.push(json!({"type": "text", "text": answer}));
    let body2 = json!({
        "model": model(),
        "stream": true,
        "max_tokens": 1024,
        "messages": [
            {"role": "user", "content": "What is 17 * 23? Just the number."},
            {"role": "assistant", "content": assistant_content},
            {"role": "user", "content": "Now add 9 to that. Just the number."},
        ],
    });
    let (events2, _) = drain(dispatch(body2, true).await.expect("turn2 不应 400")).await;
    let text2: String = events2
        .iter()
        .filter_map(|e| match e {
            AnthropicEvent::ContentBlockDelta { delta, .. }
                if delta["type"] == "text_delta" =>
            {
                delta["text"].as_str().map(String::from)
            }
            _ => None,
        })
        .collect();
    assert!(!text2.is_empty(), "多轮回灌后应正常回答");
    println!("turn2 answer={text2:?}");
}

#[tokio::test]
#[ignore]
async fn live_nonstreaming_json_with_cache_usage() {
    use cc_router_lib::proxy::transform::openai::responses_json_to_anthropic;
    let body = json!({
        "model": model(),
        "stream": false,
        "max_tokens": 512,
        "messages": [{"role": "user", "content": "Reply with exactly: OK"}],
    });
    let payload = dispatch(body, false).await.expect("dispatch ok");
    let OpenaiResponsesPayload::NonStreaming(upstream) = payload else {
        panic!("expected non-streaming payload");
    };
    let out = responses_json_to_anthropic(
        &upstream,
        &ResponsesTransformConfig::openai_official(true),
    )
    .expect("翻译成功");
    assert_eq!(out["stop_reason"], "end_turn");
    assert!(out["usage"].get("input_tokens").is_some());
    println!("nonstreaming usage={}", out["usage"]);
}
