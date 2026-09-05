//! `/v1/chat/completions` 入口的真机 live 验证 — 打**运行中的 cc-router**, 默认全 ignored.
//!
//! 环境变量:
//!   LIVE_CC_ROUTER_BASE   默认 http://127.0.0.1:23456
//!   LIVE_CC_ROUTER_TOKEN  cc-router 设置页的 token (关闭鉴权时可不设)
//!   LIVE_CHAT_MODEL       默认 model-sonnet
//!
//! 运行: cargo test --test live_chat_completions_probe -- --ignored --nocapture

use serde_json::{json, Value};

fn base() -> String {
    std::env::var("LIVE_CC_ROUTER_BASE").unwrap_or_else(|_| "http://127.0.0.1:23456".into())
}

fn model() -> String {
    std::env::var("LIVE_CHAT_MODEL").unwrap_or_else(|_| "model-sonnet".into())
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

fn post(body: &Value) -> reqwest::RequestBuilder {
    let mut req = client()
        .post(format!("{}/v1/chat/completions", base()))
        .header("content-type", "application/json")
        .json(body);
    if let Ok(t) = std::env::var("LIVE_CC_ROUTER_TOKEN") {
        req = req.bearer_auth(t);
    }
    req
}

#[tokio::test]
#[ignore]
async fn live_nonstreaming_text() {
    let resp = post(&json!({
        "model": model(),
        "messages": [{"role":"user","content":"用一句话介绍你自己"}],
        "max_tokens": 200,
    }))
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 200, "{}", resp.text().await.unwrap());
    let v: Value = resp.json().await.unwrap();
    println!("{v:#}");
    assert_eq!(v["object"], "chat.completion");
    assert_eq!(v["model"], model(), "回显请求名");
    assert!(v["choices"][0]["message"]["content"].as_str().unwrap().len() > 0);
    assert_eq!(v["choices"][0]["finish_reason"], "stop");
    assert!(v["usage"]["total_tokens"].as_i64().unwrap() > 0);
}

#[tokio::test]
#[ignore]
async fn live_streaming_collects_content_usage_done() {
    let resp = post(&json!({
        "model": model(),
        "stream": true,
        "messages": [{"role":"user","content":"数到 5, 用逗号分隔"}],
        "max_tokens": 100,
    }))
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("text/event-stream"));
    let text = resp.text().await.unwrap();
    println!("{text}");
    let frames: Vec<&str> = text.split("\n\n").filter(|f| f.starts_with("data: ")).collect();
    assert_eq!(*frames.last().unwrap(), "data: [DONE]");
    let jsons: Vec<Value> = frames[..frames.len() - 1]
        .iter()
        .map(|f| serde_json::from_str(&f["data: ".len()..]).unwrap())
        .collect();
    assert_eq!(jsons[0]["choices"][0]["delta"]["role"], "assistant");
    let content: String = jsons
        .iter()
        .filter_map(|j| j["choices"].get(0).and_then(|c| c["delta"]["content"].as_str()))
        .collect();
    assert!(content.contains('5'), "拼接内容: {content}");
    assert!(jsons.iter().any(|j| j["choices"].get(0).map(|c| c["finish_reason"] == "stop").unwrap_or(false)));
    let usage = jsons.last().unwrap();
    assert_eq!(usage["choices"], json!([]));
    assert!(usage["usage"]["total_tokens"].as_i64().unwrap() > 0);
}

#[tokio::test]
#[ignore]
async fn live_tool_roundtrip() {
    let tools = json!([{"type":"function","function":{
        "name":"get_weather","description":"查询城市天气",
        "parameters":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}
    }}]);
    // 第一轮: 强制调用工具
    let first: Value = post(&json!({
        "model": model(),
        "messages": [{"role":"user","content":"上海现在天气怎么样?"}],
        "tools": tools,
        "tool_choice": "required",
        "max_tokens": 300,
    }))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    println!("{first:#}");
    assert_eq!(first["choices"][0]["finish_reason"], "tool_calls");
    let call = &first["choices"][0]["message"]["tool_calls"][0];
    assert_eq!(call["function"]["name"], "get_weather");
    let args: Value = serde_json::from_str(call["function"]["arguments"].as_str().unwrap()).unwrap();
    assert!(args["city"].is_string());

    // 第二轮: 回传 tool 结果 (assistant 消息原样回传, 含 tool_calls)
    let second: Value = post(&json!({
        "model": model(),
        "messages": [
            {"role":"user","content":"上海现在天气怎么样?"},
            first["choices"][0]["message"].clone(),
            {"role":"tool","tool_call_id": call["id"], "content":"晴, 26°C"}
        ],
        "tools": tools,
        "max_tokens": 300,
    }))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    println!("{second:#}");
    assert_eq!(second["choices"][0]["finish_reason"], "stop");
    assert!(second["choices"][0]["message"]["content"].as_str().unwrap().contains("26"));
}
