//! Phase 0 冒烟测试：验证 ChatGPT Plus/Pro 反代后端的协议假设。
//!
//! 跑法：
//!   cd src-tauri && cargo run --example chatgpt_smoke
//!
//! 前置：用户已经在本机用 `codex login` 登录过 ChatGPT 账号。
//!
//! 这个 binary 不属于产品代码，只为验证：
//! 1. 从 ~/.codex/auth.json 读出来的 access_token 是否能直接打 chatgpt.com 后端
//! 2. 必需 headers (User-Agent/ChatGPT-Account-Id/OpenAI-Beta) 的真实组合
//! 3. 必需 body 字段 (store=false, include=reasoning.encrypted_content) 的反应
//! 4. 流式 SSE 的真实事件名列表（决定翻译层要 cover 的 case）
//!
//! 跑通后把结论手写进 plan 文件的 "协议事实" 段，然后 Phase 1 才动手写主体。

use std::env;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};

const RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";

/// 与 src/oauth/chatgpt.rs::build_codex_ua 保持一致 (避免依赖 lib 模块).
/// 形如: codex_cli_rs/<ver> (<os>; <arch>) cc-router-smoke
fn build_codex_ua() -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    format!("codex_cli_rs/smoke ({os}; {arch}) cc-router-smoke")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let auth_path = home_dir().join(".codex").join("auth.json");
    let raw = std::fs::read_to_string(&auth_path)
        .map_err(|e| format!("无法读 {}: {e}", auth_path.display()))?;
    let auth: Value = serde_json::from_str(&raw)?;

    let access_token = auth
        .pointer("/tokens/access_token")
        .and_then(|v| v.as_str())
        .ok_or("auth.json 缺少 tokens.access_token")?
        .to_string();
    let account_id = auth
        .pointer("/tokens/account_id")
        .and_then(|v| v.as_str())
        .ok_or("auth.json 缺少 tokens.account_id")?
        .to_string();

    println!(
        "[smoke] account_id 预览={}…{} (长度 {})",
        &account_id[..6.min(account_id.len())],
        &account_id[account_id.len().saturating_sub(6)..],
        account_id.len()
    );
    println!("[smoke] access_token 长度={} (头/尾各保留 6 char)", access_token.len());
    println!(
        "[smoke] access_token 预览={}…{}",
        &access_token[..6.min(access_token.len())],
        &access_token[access_token.len().saturating_sub(6)..]
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?;

    let model = env::var("SMOKE_MODEL").unwrap_or_else(|_| "gpt-5.5".to_string());
    println!("[smoke] 用模型: {} (覆盖: SMOKE_MODEL=...)", model);
    println!("[smoke] 注意: ChatGPT 订阅后端强制 stream=true, 跳过非流式测试");

    // ============ 测试 1: 流式 SSE 抓事件名（短文本） ============
    println!("\n=== Test 1: 流式 SSE 抓事件名 ===");
    let body_stream = json!({
        "model": model,
        "instructions": "你是一个简洁的助手。",
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "数从 1 到 5"}]
        }],
        "stream": true,
        "store": false,
        "include": ["reasoning.encrypted_content"],
    });

    let resp = client
        .post(RESPONSES_URL)
        .bearer_auth(&access_token)
        .header("ChatGPT-Account-Id", &account_id)
        .header("User-Agent", build_codex_ua())
        .header("OpenAI-Beta", "responses=experimental")
        .header("originator", "codex_cli_rs")
        .header("Accept", "text/event-stream")
        .json(&body_stream)
        .send()
        .await?;

    let status = resp.status();
    println!("[smoke] HTTP {}", status);

    if !status.is_success() {
        let text = resp.text().await?;
        println!("[smoke] body: {}", &text.chars().take(1500).collect::<String>());
        return Ok(());
    }

    // 解析 SSE: 每帧形如 `event: <name>\ndata: <json>\n\n`
    let mut event_counts: std::collections::BTreeMap<String, u32> = Default::default();
    let mut sample_payloads: std::collections::BTreeMap<String, String> = Default::default();
    let mut buf = String::new();

    let mut stream = resp.bytes_stream();
    use futures::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buf.push_str(std::str::from_utf8(&chunk).unwrap_or(""));
        while let Some(idx) = buf.find("\n\n") {
            let frame = buf[..idx].to_string();
            buf.drain(..idx + 2);
            let mut event_name = String::new();
            let mut data = String::new();
            for line in frame.lines() {
                if let Some(rest) = line.strip_prefix("event: ") {
                    event_name = rest.to_string();
                } else if let Some(rest) = line.strip_prefix("data: ") {
                    if !data.is_empty() {
                        data.push('\n');
                    }
                    data.push_str(rest);
                }
            }
            if event_name.is_empty() {
                continue;
            }
            *event_counts.entry(event_name.clone()).or_insert(0) += 1;
            sample_payloads
                .entry(event_name.clone())
                .or_insert_with(|| data.chars().take(400).collect::<String>());
        }
    }

    println!("\n[smoke] === SSE 事件统计 ===");
    for (name, count) in &event_counts {
        println!("  {} ×{}", name, count);
    }
    println!("\n[smoke] === 每种事件首次 payload (前 400 char) ===");
    for (name, payload) in &sample_payloads {
        println!("--- {} ---", name);
        println!("{}", payload);
        println!();
    }

    // ============ 测试 2: 带 tool definition 的流式（验证 function_call 事件） ============
    println!("\n=== Test 2: 带 tool 定义的流式 (验证 tool_use SSE 事件) ===");
    let body_with_tools = json!({
        "model": model,
        "instructions": "如果需要查询天气, 调用 get_weather 工具。",
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "北京今天天气怎么样?"}]
        }],
        "tools": [{
            "type": "function",
            "name": "get_weather",
            "description": "获取指定城市的实时天气",
            "parameters": {
                "type": "object",
                "properties": {
                    "city": {"type": "string", "description": "城市名"}
                },
                "required": ["city"]
            }
        }],
        "stream": true,
        "store": false,
        "include": ["reasoning.encrypted_content"],
    });

    let resp = client
        .post(RESPONSES_URL)
        .bearer_auth(&access_token)
        .header("ChatGPT-Account-Id", &account_id)
        .header("User-Agent", build_codex_ua())
        .header("OpenAI-Beta", "responses=experimental")
        .header("originator", "codex_cli_rs")
        .header("Accept", "text/event-stream")
        .json(&body_with_tools)
        .send()
        .await?;

    let status = resp.status();
    println!("[smoke] HTTP {}", status);
    if !status.is_success() {
        let text = resp.text().await?;
        println!("[smoke] body: {}", &text.chars().take(2000).collect::<String>());
        return Ok(());
    }

    let mut event_counts2: std::collections::BTreeMap<String, u32> = Default::default();
    let mut sample_payloads2: std::collections::BTreeMap<String, String> = Default::default();
    let mut buf2 = String::new();
    let mut stream2 = resp.bytes_stream();
    while let Some(chunk) = stream2.next().await {
        let chunk = chunk?;
        buf2.push_str(std::str::from_utf8(&chunk).unwrap_or(""));
        while let Some(idx) = buf2.find("\n\n") {
            let frame = buf2[..idx].to_string();
            buf2.drain(..idx + 2);
            let mut event_name = String::new();
            let mut data = String::new();
            for line in frame.lines() {
                if let Some(rest) = line.strip_prefix("event: ") {
                    event_name = rest.to_string();
                } else if let Some(rest) = line.strip_prefix("data: ") {
                    if !data.is_empty() {
                        data.push('\n');
                    }
                    data.push_str(rest);
                }
            }
            if event_name.is_empty() {
                continue;
            }
            *event_counts2.entry(event_name.clone()).or_insert(0) += 1;
            sample_payloads2
                .entry(event_name.clone())
                .or_insert_with(|| data.chars().take(600).collect::<String>());
        }
    }

    println!("\n[smoke] === Test 2 SSE 事件统计 (含 tool) ===");
    for (name, count) in &event_counts2 {
        println!("  {} ×{}", name, count);
    }
    println!("\n[smoke] === Test 2 每种事件首次 payload ===");
    for (name, payload) in &sample_payloads2 {
        println!("--- {} ---", name);
        println!("{}", payload);
        println!();
    }

    // ============ 测试 3: GET /codex/models 拿当前账号可见的模型列表 ============
    println!("\n=== Test 3: 列模型 (GET /backend-api/codex/models) ===");
    let client_version = env!("CARGO_PKG_VERSION");
    let models_url = format!(
        "https://chatgpt.com/backend-api/codex/models?client_version={}",
        client_version
    );
    let resp = client
        .get(&models_url)
        .bearer_auth(&access_token)
        .header("ChatGPT-Account-Id", &account_id)
        .header("User-Agent", build_codex_ua())
        .header("OpenAI-Beta", "responses=experimental")
        .header("originator", "codex_cli_rs")
        .send()
        .await?;
    let status = resp.status();
    println!("[smoke] HTTP {} {}", status, models_url);
    let text = resp.text().await?;
    if !status.is_success() {
        println!(
            "[smoke] body: {}",
            &text.chars().take(2000).collect::<String>()
        );
    } else {
        // 完整 dump 一份, 让我们能看到 visibility / supported_in_api / priority 等字段实际取值.
        match serde_json::from_str::<Value>(&text) {
            Ok(v) => {
                if let Some(models) = v.get("models").and_then(|m| m.as_array()) {
                    println!("[smoke] 共 {} 个模型, visibility 分布:", models.len());
                    let mut vis_counts: std::collections::BTreeMap<String, u32> = Default::default();
                    for m in models {
                        let v = m
                            .get("visibility")
                            .and_then(|s| s.as_str())
                            .unwrap_or("<missing>")
                            .to_string();
                        *vis_counts.entry(v).or_insert(0) += 1;
                    }
                    for (v, c) in &vis_counts {
                        println!("  {} ×{}", v, c);
                    }
                    println!("\n[smoke] 完整模型清单 (slug | visibility | display_name):");
                    for m in models {
                        let slug = m.get("slug").and_then(|s| s.as_str()).unwrap_or("?");
                        let vis = m.get("visibility").and_then(|s| s.as_str()).unwrap_or("?");
                        let dn = m.get("display_name").and_then(|s| s.as_str()).unwrap_or("");
                        println!("  {} | {} | {}", slug, vis, dn);
                    }
                    println!("\n[smoke] 第一条完整 payload (用于核对其他字段):");
                    if let Some(first) = models.first() {
                        println!("{}", serde_json::to_string_pretty(first)?);
                    }
                } else {
                    println!(
                        "[smoke] 顶层缺 models 数组, 完整响应 (前 2000 char):\n{}",
                        text.chars().take(2000).collect::<String>()
                    );
                }
            }
            Err(e) => {
                println!(
                    "[smoke] 解析 JSON 失败: {e}, 原文 (前 2000 char):\n{}",
                    text.chars().take(2000).collect::<String>()
                );
            }
        }
    }

    println!("\n[smoke] ✅ Phase 0 完毕。请把 SSE 事件列表 + Test 3 的 visibility 取值贴给 Claude。");
    Ok(())
}

fn home_dir() -> PathBuf {
    if let Ok(h) = env::var("HOME") {
        return PathBuf::from(h);
    }
    if let Ok(h) = env::var("USERPROFILE") {
        return PathBuf::from(h);
    }
    panic!("无 HOME / USERPROFILE 环境变量");
}
