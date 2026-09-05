//! OpenAI Chat Completions → Anthropic Messages 反向翻译层 (入站方向).
//!
//! 用途: cc-router 对外的 `POST /v1/chat/completions` 兼容入口 ([`handler::chat_completions`]) 用本模块
//! 把 Chat Completions 请求翻译成 Anthropic Messages 走现有 pipeline, 再把 Anthropic 响应 / SSE
//! 翻译回 Chat Completions 给客户端。
//!
//! 配对方 (方向相反, 代码不共享): [`super::openai_chat_completions`] 是出站方向
//! (Anthropic → 上游 Chat Completions, 用于 `auth_type=openai_chat_completions_api_key`)。
//!
//! ## 边界
//!
//! 本模块的函数都是纯函数 + 同步状态机, 不发起任何网络请求, 不读 DB / settings。
//! pipeline 不变, 上游全部 provider 与翻译 dispatch 零改动。
//!
//! ## 刻意不支持 (spec §1)
//!
//! - 旧版 `functions` / `function_call` → 400
//! - `n>1` / `logprobs` / `logit_bias` / `seed` / `presence_penalty` / `frequency_penalty` /
//!   `response_format` / `stream_options` → 忽略
//! - 历史消息里的 `reasoning_content` → 丢弃 (客户端拿不到合法 signature, Anthropic 透传会拒)
//! - 音频 / 文件类 content part → 400

#[allow(unused_imports)]
use std::collections::HashMap;

use serde_json::{json, Value};
#[allow(unused_imports)]
use uuid::Uuid;

use crate::error::{AppError, AppResult};

use super::responses_common::effort_to_budget_tokens;

/// Anthropic 要求 max_tokens 必填; 客户端没给时的缺省, 与 responses_inbound 一致.
const DEFAULT_MAX_TOKENS: i64 = 4096;

// ============================================================
// 请求侧: Chat Completions request → Anthropic Messages request
// ============================================================

/// OpenAI Chat Completions 请求体 → Anthropic Messages 请求体 (spec §3).
pub fn request_to_anthropic(body: &Value) -> AppResult<Value> {
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::BadRequest("请求 body 缺少 model".into()))?;
    if body.get("functions").is_some() || body.get("function_call").is_some() {
        return Err(AppError::BadRequest(
            "不支持旧版 functions / function_call 字段, 请使用 tools / tool_choice".into(),
        ));
    }
    let messages = body
        .get("messages")
        .and_then(|v| v.as_array())
        .filter(|a| !a.is_empty())
        .ok_or_else(|| AppError::BadRequest("messages 缺失或为空".into()))?;

    let mut out = json!({ "model": model });
    if let Some(s) = body.get("stream") {
        out["stream"] = s.clone();
    }

    let (anthropic_messages, system_text) = convert_messages(messages)?;
    if anthropic_messages.is_empty() {
        return Err(AppError::BadRequest("messages 中没有 user / assistant / tool 消息".into()));
    }
    out["messages"] = Value::Array(anthropic_messages);
    if !system_text.is_empty() {
        out["system"] = json!([{"type": "text", "text": system_text}]);
    }

    // max_completion_tokens 优先 (OpenAI 新字段), 其次 max_tokens; 都缺省 → 4096
    let max_tokens = body
        .get("max_completion_tokens")
        .and_then(|v| v.as_i64())
        .or_else(|| body.get("max_tokens").and_then(|v| v.as_i64()))
        .unwrap_or(DEFAULT_MAX_TOKENS);
    out["max_tokens"] = json!(max_tokens);

    for key in ["temperature", "top_p"] {
        if let Some(v) = body.get(key) {
            out[key] = v.clone();
        }
    }

    match body.get("stop") {
        Some(Value::String(s)) => out["stop_sequences"] = json!([s]),
        Some(Value::Array(arr)) => out["stop_sequences"] = Value::Array(arr.clone()),
        _ => {}
    }

    if let Some(user) = body.get("user").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        out["metadata"] = json!({ "user_id": user });
    }

    if let Some(effort) = body.get("reasoning_effort").and_then(|v| v.as_str()) {
        if effort != "none" {
            out["thinking"] = json!({
                "type": "enabled",
                "budget_tokens": effort_to_budget_tokens(effort),
            });
        }
    }

    let mut has_tools = false;
    if let Some(tools) = body.get("tools").and_then(|v| v.as_array()) {
        let mut converted = Vec::with_capacity(tools.len());
        for t in tools {
            converted.push(convert_tool(t)?);
        }
        if !converted.is_empty() {
            out["tools"] = Value::Array(converted);
            has_tools = true;
        }
    }

    // parallel_tool_calls:false → Anthropic 的 disable_parallel_tool_use 挂在 tool_choice 上,
    // 客户端没给 tool_choice 但有 tools 时以 auto 承载.
    let disable_parallel = body.get("parallel_tool_calls").and_then(|v| v.as_bool()) == Some(false);
    let mut tool_choice = match body.get("tool_choice") {
        Some(tc) => Some(convert_tool_choice(tc)?),
        None if has_tools && disable_parallel => Some(json!({"type": "auto"})),
        None => None,
    };
    if let Some(tc) = tool_choice.as_mut() {
        if disable_parallel && tc.get("type").and_then(|v| v.as_str()) != Some("none") {
            tc["disable_parallel_tool_use"] = json!(true);
        }
    }
    if let Some(tc) = tool_choice {
        out["tool_choice"] = tc;
    }

    Ok(out)
}

/// Chat `messages[]` → (Anthropic messages, 合并后的 system 文本).
///
/// 规则 (spec §3.2): system/developer 抽到顶层; assistant 的 tool_calls → tool_use 块,
/// reasoning_content 丢弃; **连续多条 tool 消息合并进同一条 user 消息** (Anthropic 要求
/// 同一轮的 tool_result 全在一条 user 消息里)。**转换完成后再做一趟相邻同 role 合并**
/// (`merge_adjacent_same_role`): tool_result 消息与紧随其后的普通 user 文本消息都是
/// `role:"user"`, 不合并会产生两条相邻 user 消息——Anthropic 官方能容忍, 但 cc-router
/// 还会分发给严格程度不一的第三方 Anthropic 兼容端点/翻译层, 合并后的形状在所有下游都合法。
fn convert_messages(messages: &[Value]) -> AppResult<(Vec<Value>, String)> {
    let mut out: Vec<Value> = Vec::new();
    let mut system = String::new();
    let mut pending_tool_results: Vec<Value> = Vec::new();

    fn flush_tool_results(out: &mut Vec<Value>, pending: &mut Vec<Value>) {
        if !pending.is_empty() {
            out.push(json!({ "role": "user", "content": std::mem::take(pending) }));
        }
    }

    for (i, msg) in messages.iter().enumerate() {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role != "tool" {
            flush_tool_results(&mut out, &mut pending_tool_results);
        }
        match role {
            "system" | "developer" => {
                let text = content_text(msg.get("content"));
                if !text.is_empty() {
                    if !system.is_empty() {
                        system.push_str("\n\n");
                    }
                    system.push_str(&text);
                }
            }
            "user" => {
                let blocks = user_content_blocks(msg.get("content"), i)?;
                out.push(json!({ "role": "user", "content": blocks }));
            }
            "assistant" => {
                let mut blocks: Vec<Value> = Vec::new();
                let text = content_text(msg.get("content"));
                if !text.is_empty() {
                    blocks.push(json!({ "type": "text", "text": text }));
                }
                if let Some(calls) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                    for call in calls {
                        blocks.push(convert_tool_call(call)?);
                    }
                }
                // reasoning_content 刻意丢弃 (spec §3.4)
                if !blocks.is_empty() {
                    out.push(json!({ "role": "assistant", "content": blocks }));
                }
            }
            "tool" => {
                let id = msg
                    .get("tool_call_id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| AppError::BadRequest(format!("messages[{i}] (tool) 缺少 tool_call_id")))?;
                pending_tool_results.push(json!({
                    "type": "tool_result",
                    "tool_use_id": id,
                    "content": content_text(msg.get("content")),
                }));
            }
            other => {
                return Err(AppError::BadRequest(format!("messages[{i}] 不支持的 role: {other:?}")));
            }
        }
    }
    flush_tool_results(&mut out, &mut pending_tool_results);
    Ok((merge_adjacent_same_role(out), system))
}

/// 合并相邻的同 role 消息, 把后一条的 content 数组接到前一条后面.
///
/// 覆盖 user+user (含 "tool_result 块后接一条 user 文本" 这种常见 agentic 形状) 与
/// assistant+assistant。两条消息的 `content` 都保证是数组 (本模块构造的消息全部如此)。
fn merge_adjacent_same_role(messages: Vec<Value>) -> Vec<Value> {
    let mut merged: Vec<Value> = Vec::with_capacity(messages.len());
    for mut msg in messages {
        if let Some(last) = merged.last_mut() {
            if last["role"] == msg["role"] {
                if let (Some(last_content), Some(cur_content)) =
                    (last["content"].as_array_mut(), msg["content"].as_array_mut())
                {
                    last_content.append(cur_content);
                    continue;
                }
            }
        }
        merged.push(msg);
    }
    merged
}

/// content 为 string 直接返回; 为数组时拼接其中 `type:text` 的 text (以 "\n" 连接); 其他 → 空串.
fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// user 消息 content → Anthropic content blocks. 空文本 → 400 (Anthropic 拒绝空 text 块).
fn user_content_blocks(content: Option<&Value>, idx: usize) -> AppResult<Vec<Value>> {
    let mut blocks: Vec<Value> = Vec::new();
    match content {
        Some(Value::String(s)) => {
            if !s.is_empty() {
                blocks.push(json!({ "type": "text", "text": s }));
            }
        }
        Some(Value::Array(parts)) => {
            for part in parts {
                match part.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                    "text" => {
                        if let Some(t) = part.get("text").and_then(|t| t.as_str()).filter(|t| !t.is_empty()) {
                            blocks.push(json!({ "type": "text", "text": t }));
                        }
                    }
                    "image_url" => blocks.push(convert_image_url(part.get("image_url"), idx)?),
                    other => {
                        return Err(AppError::BadRequest(format!(
                            "messages[{idx}] 不支持的 content part 类型: {other:?}"
                        )));
                    }
                }
            }
        }
        _ => {}
    }
    if blocks.is_empty() {
        return Err(AppError::BadRequest(format!("messages[{idx}] (user) 内容为空")));
    }
    Ok(blocks)
}

/// `image_url` part → Anthropic image 块. data URI → base64 source; http(s) → url source; 其他 → 400.
fn convert_image_url(image_url: Option<&Value>, idx: usize) -> AppResult<Value> {
    let url = match image_url {
        Some(Value::String(s)) => s.as_str(),
        Some(obj) => obj.get("url").and_then(|u| u.as_str()).unwrap_or(""),
        None => "",
    };
    if let Some(rest) = url.strip_prefix("data:") {
        // data:<media_type>;base64,<payload>
        let (meta, data) = rest
            .split_once(',')
            .ok_or_else(|| AppError::BadRequest(format!("messages[{idx}] image data URI 缺少逗号分隔")))?;
        let media_type = meta.strip_suffix(";base64").ok_or_else(|| {
            AppError::BadRequest(format!("messages[{idx}] image data URI 必须是 base64 编码"))
        })?;
        return Ok(json!({
            "type": "image",
            "source": { "type": "base64", "media_type": media_type, "data": data },
        }));
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        return Ok(json!({ "type": "image", "source": { "type": "url", "url": url } }));
    }
    Err(AppError::BadRequest(format!(
        "messages[{idx}] image_url 只支持 data: 与 http(s): 两种形式"
    )))
}

/// `{type:"function", function:{name, description?, parameters?}}` → Anthropic tool spec.
fn convert_tool(t: &Value) -> AppResult<Value> {
    if t.get("type").and_then(|v| v.as_str()) != Some("function") {
        return Err(AppError::BadRequest(format!(
            "tools 只支持 type=function, 收到: {}",
            t.get("type").cloned().unwrap_or(Value::Null)
        )));
    }
    let f = t
        .get("function")
        .ok_or_else(|| AppError::BadRequest("tools[] 缺少 function 字段".into()))?;
    let name = f
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("tools[].function 缺少 name".into()))?;
    let mut out = json!({
        "name": name,
        "input_schema": f.get("parameters").cloned().unwrap_or_else(|| json!({"type": "object", "properties": {}})),
    });
    if let Some(d) = f.get("description") {
        out["description"] = d.clone();
    }
    Ok(out)
}

/// assistant.tool_calls[i] → Anthropic tool_use 块. arguments 是 JSON 字符串 (空串视为 {}),
/// 部分客户端会直接给对象, 也接受; 非法 JSON → 400 并带 tool_call id.
fn convert_tool_call(call: &Value) -> AppResult<Value> {
    let id = call.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let f = call
        .get("function")
        .ok_or_else(|| AppError::BadRequest(format!("tool_calls[{id}] 缺少 function 字段")))?;
    let name = f
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::BadRequest(format!("tool_calls[{id}] 缺少 function.name")))?;
    let input = match f.get("arguments") {
        None => json!({}),
        Some(Value::String(s)) if s.trim().is_empty() => json!({}),
        Some(Value::String(s)) => serde_json::from_str::<Value>(s).map_err(|e| {
            AppError::BadRequest(format!("tool_calls[{id}] 的 arguments 不是合法 JSON: {e}"))
        })?,
        Some(other) => other.clone(),
    };
    Ok(json!({ "type": "tool_use", "id": id, "name": name, "input": input }))
}

/// Chat tool_choice → Anthropic tool_choice (spec §3.3).
fn convert_tool_choice(tc: &Value) -> AppResult<Value> {
    if let Some(s) = tc.as_str() {
        return match s {
            "auto" => Ok(json!({ "type": "auto" })),
            "none" => Ok(json!({ "type": "none" })),
            "required" => Ok(json!({ "type": "any" })),
            other => Err(AppError::BadRequest(format!("不支持的 tool_choice: {other:?}"))),
        };
    }
    if tc.get("type").and_then(|v| v.as_str()) == Some("function") {
        if let Some(name) = tc.get("function").and_then(|f| f.get("name")).and_then(|n| n.as_str()) {
            return Ok(json!({ "type": "tool", "name": name }));
        }
    }
    Err(AppError::BadRequest("tool_choice 形状不合法".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(messages: Value) -> Value {
        json!({"model": "gpt-5.5", "messages": messages})
    }

    // ---- 请求侧 ----

    #[test]
    fn request_text_only_defaults() {
        let out = request_to_anthropic(&base(json!([{"role":"user","content":"Hello"}]))).unwrap();
        assert_eq!(out["model"], "gpt-5.5");
        assert_eq!(out["max_tokens"], 4096);
        assert_eq!(out["messages"][0]["role"], "user");
        assert_eq!(out["messages"][0]["content"][0], json!({"type":"text","text":"Hello"}));
        assert!(out.get("system").is_none());
        assert!(out.get("stream").is_none());
        assert!(out.get("thinking").is_none());
    }

    #[test]
    fn request_system_and_developer_merge_into_system_in_order() {
        let out = request_to_anthropic(&base(json!([
            {"role":"system","content":"A"},
            {"role":"user","content":"hi"},
            {"role":"developer","content":[{"type":"text","text":"B"}]},
        ])))
        .unwrap();
        assert_eq!(out["system"], json!([{"type":"text","text":"A\n\nB"}]));
        assert_eq!(out["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn request_max_completion_tokens_wins_over_max_tokens() {
        let mut b = base(json!([{"role":"user","content":"hi"}]));
        b["max_tokens"] = json!(100);
        b["max_completion_tokens"] = json!(200);
        assert_eq!(request_to_anthropic(&b).unwrap()["max_tokens"], 200);
        let mut b2 = base(json!([{"role":"user","content":"hi"}]));
        b2["max_tokens"] = json!(100);
        assert_eq!(request_to_anthropic(&b2).unwrap()["max_tokens"], 100);
    }

    #[test]
    fn request_stream_temperature_top_p_stop_user_passthrough() {
        let mut b = base(json!([{"role":"user","content":"hi"}]));
        b["stream"] = json!(true);
        b["temperature"] = json!(0.3);
        b["top_p"] = json!(0.9);
        b["stop"] = json!("END");
        b["user"] = json!("u-1");
        let out = request_to_anthropic(&b).unwrap();
        assert_eq!(out["stream"], true);
        assert_eq!(out["temperature"], 0.3);
        assert_eq!(out["top_p"], 0.9);
        assert_eq!(out["stop_sequences"], json!(["END"]));
        assert_eq!(out["metadata"]["user_id"], "u-1");
        b["stop"] = json!(["a", "b"]);
        assert_eq!(request_to_anthropic(&b).unwrap()["stop_sequences"], json!(["a", "b"]));
    }

    #[test]
    fn request_reasoning_effort_to_thinking_and_none_disables() {
        let mut b = base(json!([{"role":"user","content":"hi"}]));
        b["reasoning_effort"] = json!("low");
        let out = request_to_anthropic(&b).unwrap();
        assert_eq!(out["thinking"], json!({"type":"enabled","budget_tokens":2048}));
        b["reasoning_effort"] = json!("none");
        assert!(request_to_anthropic(&b).unwrap().get("thinking").is_none());
    }

    #[test]
    fn request_tools_and_tool_choice_variants() {
        let mut b = base(json!([{"role":"user","content":"hi"}]));
        b["tools"] = json!([
            {"type":"function","function":{"name":"get_weather","description":"d","parameters":{"type":"object","properties":{"city":{"type":"string"}}}}},
            {"type":"function","function":{"name":"noop"}},
        ]);
        let out = request_to_anthropic(&b).unwrap();
        assert_eq!(out["tools"][0]["name"], "get_weather");
        assert_eq!(out["tools"][0]["description"], "d");
        assert_eq!(out["tools"][0]["input_schema"]["properties"]["city"]["type"], "string");
        assert_eq!(out["tools"][1]["input_schema"], json!({"type":"object","properties":{}}));
        assert!(out.get("tool_choice").is_none());

        for (input, expected) in [
            (json!("auto"), json!({"type":"auto"})),
            (json!("none"), json!({"type":"none"})),
            (json!("required"), json!({"type":"any"})),
            (json!({"type":"function","function":{"name":"get_weather"}}), json!({"type":"tool","name":"get_weather"})),
        ] {
            b["tool_choice"] = input;
            assert_eq!(request_to_anthropic(&b).unwrap()["tool_choice"], expected);
        }

        b["tool_choice"] = json!("bogus");
        assert!(request_to_anthropic(&b).is_err());
        b["tool_choice"] = json!("auto");
        b["tools"] = json!([{"type":"file_search"}]);
        assert!(request_to_anthropic(&b).is_err());
    }

    #[test]
    fn request_parallel_tool_calls_false_sets_disable_parallel() {
        let mut b = base(json!([{"role":"user","content":"hi"}]));
        b["tools"] = json!([{"type":"function","function":{"name":"f"}}]);
        b["parallel_tool_calls"] = json!(false);
        let out = request_to_anthropic(&b).unwrap();
        assert_eq!(out["tool_choice"], json!({"type":"auto","disable_parallel_tool_use":true}));
        b["tool_choice"] = json!("none");
        assert_eq!(request_to_anthropic(&b).unwrap()["tool_choice"], json!({"type":"none"}));
    }

    #[test]
    fn request_assistant_tool_calls_and_consecutive_tool_results() {
        let out = request_to_anthropic(&base(json!([
            {"role":"user","content":"weather?"},
            {"role":"assistant","content":null,"reasoning_content":"thinking...","tool_calls":[
                {"id":"call_1","type":"function","function":{"name":"get_weather","arguments":"{\"city\":\"SH\"}"}},
                {"id":"call_2","type":"function","function":{"name":"get_time","arguments":""}}
            ]},
            {"role":"tool","tool_call_id":"call_1","content":"sunny"},
            {"role":"tool","tool_call_id":"call_2","content":[{"type":"text","text":"12:00"}]},
            {"role":"user","content":"thanks"}
        ])))
        .unwrap();
        let msgs = out["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3, "tool_result 消息与后续 user 文本消息合并为一条");
        for pair in msgs.windows(2) {
            assert_ne!(pair[0]["role"], pair[1]["role"], "相邻消息不应同 role");
        }
        let asst = &msgs[1]["content"];
        assert_eq!(asst.as_array().unwrap().len(), 2, "空 content 不产生 text 块, reasoning_content 丢弃");
        assert_eq!(asst[0], json!({"type":"tool_use","id":"call_1","name":"get_weather","input":{"city":"SH"}}));
        assert_eq!(asst[1]["input"], json!({}), "空 arguments 视为 {{}}");
        // 两条 tool 结果 + 之后的 user 文本合并进同一条 user 消息
        assert_eq!(msgs[2]["role"], "user");
        assert_eq!(msgs[2]["content"], json!([
            {"type":"tool_result","tool_use_id":"call_1","content":"sunny"},
            {"type":"tool_result","tool_use_id":"call_2","content":"12:00"},
            {"type":"text","text":"thanks"}
        ]));
    }

    #[test]
    fn request_merges_adjacent_same_role_messages() {
        let out = request_to_anthropic(&base(json!([
            {"role":"user","content":"a"},
            {"role":"user","content":"b"},
            {"role":"assistant","content":"x"},
            {"role":"assistant","content":"y"},
            {"role":"user","content":"c"}
        ])))
        .unwrap();
        let msgs = out["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"], json!([
            {"type":"text","text":"a"},
            {"type":"text","text":"b"}
        ]));
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[1]["content"], json!([
            {"type":"text","text":"x"},
            {"type":"text","text":"y"}
        ]));
        assert_eq!(msgs[2]["role"], "user");
        assert_eq!(msgs[2]["content"], json!([{"type":"text","text":"c"}]));
    }

    #[test]
    fn request_assistant_text_with_tool_calls_keeps_both() {
        let out = request_to_anthropic(&base(json!([
            {"role":"user","content":"x"},
            {"role":"assistant","content":"Let me check","tool_calls":[
                {"id":"c","type":"function","function":{"name":"f","arguments":{"a":1}}}
            ]}
        ])))
        .unwrap();
        let asst = &out["messages"][1]["content"];
        assert_eq!(asst[0], json!({"type":"text","text":"Let me check"}));
        assert_eq!(asst[1]["input"], json!({"a":1}), "arguments 已是对象时直接采用");
    }

    #[test]
    fn request_invalid_tool_call_arguments_is_400_with_id() {
        let err = request_to_anthropic(&base(json!([
            {"role":"user","content":"x"},
            {"role":"assistant","tool_calls":[{"id":"call_bad","type":"function","function":{"name":"f","arguments":"{not json"}}]}
        ])))
        .unwrap_err();
        assert!(matches!(err, AppError::BadRequest(ref m) if m.contains("call_bad")), "{err:?}");
    }

    #[test]
    fn request_image_data_uri_and_http_url() {
        let out = request_to_anthropic(&base(json!([{"role":"user","content":[
            {"type":"text","text":"看图"},
            {"type":"image_url","image_url":{"url":"data:image/png;base64,AAAA","detail":"low"}},
            {"type":"image_url","image_url":"https://example.com/a.jpg"}
        ]}])))
        .unwrap();
        let c = &out["messages"][0]["content"];
        assert_eq!(c[0], json!({"type":"text","text":"看图"}));
        assert_eq!(c[1], json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAA"}}));
        assert_eq!(c[2], json!({"type":"image","source":{"type":"url","url":"https://example.com/a.jpg"}}));
        let bad = base(json!([{"role":"user","content":[{"type":"image_url","image_url":{"url":"ftp://x"}}]}]));
        assert!(request_to_anthropic(&bad).is_err());
        let audio = base(json!([{"role":"user","content":[{"type":"input_audio","input_audio":{}}]}]));
        assert!(request_to_anthropic(&audio).is_err());
    }

    #[test]
    fn request_rejects_legacy_functions_empty_messages_and_unknown_role() {
        let mut b = base(json!([{"role":"user","content":"x"}]));
        b["functions"] = json!([]);
        assert!(matches!(request_to_anthropic(&b).unwrap_err(), AppError::BadRequest(_)));
        assert!(request_to_anthropic(&base(json!([]))).is_err());
        assert!(request_to_anthropic(&json!({"model":"m"})).is_err());
        assert!(request_to_anthropic(&base(json!([{"role":"robot","content":"x"}]))).is_err());
        assert!(request_to_anthropic(&base(json!([{"role":"tool","content":"x"}]))).is_err(), "tool 缺 tool_call_id");
        assert!(request_to_anthropic(&base(json!([{"role":"system","content":"only system"}]))).is_err(), "没有任何对话消息");
        assert!(request_to_anthropic(&json!({"messages":[{"role":"user","content":"x"}]})).is_err(), "缺 model");
    }

    #[test]
    fn request_empty_user_text_is_400() {
        assert!(request_to_anthropic(&base(json!([{"role":"user","content":""}]))).is_err());
    }
}
