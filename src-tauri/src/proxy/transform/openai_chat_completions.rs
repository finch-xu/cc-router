//! Anthropic Messages ↔ OpenAI Chat Completions (`/v1/chat/completions`) 翻译层.
//!
//! 用于 `auth_type=OpenaiChatCompletionsApiKey` 订阅: 覆盖 DeepSeek 官方 / Together / Groq /
//! Ollama / 阿里 qwen / 各类 one-api/new-api 中转, 以及 OpenAI 官方早期模型 (GPT-4 / 4-turbo).
//!
//! 与 [`super::openai`] / [`super::openai_responses`] (Responses API) 完全独立:
//! - 请求 body schema 不同 (messages/tools.function vs input/tools.shape)
//! - SSE 状态机不同 (Chat Completions 是 `delta.{content,tool_calls[i]}` 增量,
//!   Responses 是 `output_item.added` / `content_part.added` 显式事件)
//! - reasoning 仅单向暴露 (DeepSeek `reasoning_content` → Anthropic thinking block);
//!   多轮回灌 (`roundtrip_reasoning`) Phase 2 再做
//!
//! 入口:
//! - 请求: [`anthropic_to_openai_chat`]
//! - 非流响应: [`chat_json_to_anthropic`]
//! - 流响应: [`ChatCompletionsSseConverter`]
//!
//! quirks 通过 [`ChatCompletionsTransformConfig`] 参数化, 避免硬编码到翻译层 (历史教训:
//! ResponsesTransformConfig 第一版硬编码 codex 4 大约束, 接 OpenAI 官方时被迫重构)。

use std::collections::HashMap;

use serde_json::{json, Map, Value};

use crate::error::{AppError, AppResult};

use super::openai::ReasoningEffort;
use super::responses_common::AnthropicEvent;

// ============================================================
// Config
// ============================================================

#[derive(Debug, Clone)]
pub struct ChatCompletionsTransformConfig {
    /// 上游 reasoning 字段名. 默认 `"reasoning_content"` (DeepSeek R1/V3 风格,
    /// 覆盖大多数中转). Qwen 用 `"thinking"`, Phase 2 加订阅级覆盖 (UI 输入框 + DB migration).
    pub reasoning_field_name: String,
    /// 上游 reasoning_content → Anthropic thinking content_block (signature 留空).
    /// Phase 1 默认 true (yaml expose_reasoning 兜底).
    pub expose_reasoning: bool,
    /// 客户端 thinking block → 上游 assistant.message.reasoning_content (多轮回灌).
    /// Phase 1 默认 false (大多数 chat completions 上游不接受此字段, 写了会被忽略或报错).
    pub roundtrip_reasoning: bool,
    /// 部分老中转拒收 `max_tokens` 时置 true; 默认 false.
    pub drop_max_tokens: bool,
    /// Anthropic image content_block 处理策略.
    pub vision_mode: VisionMode,
    /// system 消息 role 名. 默认 `"system"`, 预留 `"developer"` 给 OpenAI o1+ 系列.
    pub system_role_name: &'static str,
    /// 是否把 Anthropic system array 多段合并成单条 string 内容. 默认 true.
    pub merge_consecutive_system: bool,
    /// tool_calls 增量模式. 默认 Incremental (OpenAI 官方协议: arguments 分片).
    /// Phase 2 给 Tencent/Aliyun 等每帧重发完整 arguments 的中转加 Cumulative.
    pub tool_call_arguments_mode: ToolCallArgumentsMode,
    /// stream=true 时是否自动注入 `stream_options: {include_usage: true}`.
    /// 默认 true (OpenAI 官方不主动发 usage, 必须 opt-in). 老版中转不识别时可关.
    pub inject_stream_options_include_usage: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisionMode {
    /// Anthropic image block → OpenAI `{type: "image_url", image_url: {url}}`.
    ImageUrl,
    /// 翻译层报错 (用于明确不支持 vision 的 yaml provider).
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallArgumentsMode {
    Incremental,
    #[allow(dead_code)]
    Cumulative,
}

impl ChatCompletionsTransformConfig {
    /// 默认配置: 覆盖 OpenAI 官方 + DeepSeek + 大多数 OpenAI 兼容中转.
    pub fn permissive() -> Self {
        Self {
            reasoning_field_name: "reasoning_content".to_string(),
            expose_reasoning: true,
            roundtrip_reasoning: false,
            drop_max_tokens: false,
            vision_mode: VisionMode::ImageUrl,
            system_role_name: "system",
            merge_consecutive_system: true,
            tool_call_arguments_mode: ToolCallArgumentsMode::Incremental,
            inject_stream_options_include_usage: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ChatCompletionsExtras {
    /// reasoning_effort 优先级链推导后的值 (None 表示客户端/订阅/yaml 都没给).
    pub reasoning_effort: Option<ReasoningEffort>,
    /// yaml 兜底 expose_reasoning. 若客户端 / 订阅级有覆盖, dispatch 层在传入前已合并.
    pub expose_reasoning: bool,
}

// ============================================================
// 请求方向: Anthropic Messages → OpenAI Chat Completions
// ============================================================

/// 翻译 Anthropic Messages 请求 body 到 OpenAI Chat Completions 请求 body.
///
/// 关键映射:
/// - `system` (string / array) → 拼接成单条 `{role: "system", content: <text>}` 消息插入首位
/// - `messages[].content` 的 text / image / tool_use / tool_result 按 Chat Completions
///   形态展开 (tool_use 提升为 assistant.tool_calls; tool_result 拆为独立 `role:"tool"` 消息)
/// - `tools[].input_schema` → `tools[].function.parameters`
/// - `tool_choice`: `auto` / `any` / `{type:"tool",name}` → OpenAI `"auto"` / `"required"` /
///   `{type:"function",function:{name}}`
/// - `max_tokens` 同名 (config.drop_max_tokens=true 时 strip)
/// - `stop_sequences` → `stop`
/// - `thinking` 字段 strip; `extra_body` strip
/// - stream=true 时自动注入 `stream_options: {include_usage: true}` (config 控制)
pub fn anthropic_to_openai_chat(
    body: &Value,
    config: &ChatCompletionsTransformConfig,
    extras: &ChatCompletionsExtras,
) -> AppResult<Value> {
    let mut out = Map::new();

    let model = body
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::BadRequest("缺少 model 字段".into()))?;
    out.insert("model".into(), json!(model));

    let mut messages: Vec<Value> = Vec::new();

    // system → 首条 system message
    if let Some(system) = body.get("system") {
        if let Some(text) = stringify_system(system, config.merge_consecutive_system) {
            if !text.is_empty() {
                messages.push(json!({
                    "role": config.system_role_name,
                    "content": text,
                }));
            }
        }
    }

    // messages 转换
    if let Some(arr) = body.get("messages").and_then(Value::as_array) {
        for msg in arr {
            let role = msg.get("role").and_then(Value::as_str).unwrap_or("user");
            let content = msg.get("content");
            translate_message(role, content, config, &mut messages)?;
        }
    }

    out.insert("messages".into(), Value::Array(messages));

    // tools
    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        let mut translated = Vec::with_capacity(tools.len());
        for t in tools {
            translated.push(translate_tool_definition(t)?);
        }
        if !translated.is_empty() {
            out.insert("tools".into(), Value::Array(translated));
        }
    }

    // tool_choice
    if let Some(tc) = body.get("tool_choice") {
        out.insert("tool_choice".into(), translate_tool_choice(tc));
    }

    // max_tokens
    if !config.drop_max_tokens {
        if let Some(mt) = body.get("max_tokens").and_then(Value::as_u64) {
            out.insert("max_tokens".into(), json!(mt));
        }
    }

    // stop_sequences → stop
    if let Some(stops) = body.get("stop_sequences") {
        out.insert("stop".into(), stops.clone());
    }

    // 直接透传的标量. 修 #15: 移除 "n" — Anthropic Messages 不支持多 sample,
    // 即便客户端通过 extra_body 注入也不应上传, 否则 chat_json_to_anthropic 只取
    // choices[0] 时其余 sample 静默丢失.
    for key in &["temperature", "top_p", "top_k", "stream", "response_format", "seed"] {
        if let Some(v) = body.get(*key) {
            out.insert((*key).to_string(), v.clone());
        }
    }

    // reasoning_effort (来自 extras): OpenAI o1+ 接收, 其他 provider 普遍忽略 (无害)
    if let Some(effort) = extras.reasoning_effort {
        out.insert("reasoning_effort".into(), json!(effort.as_str()));
    }

    // stream_options 注入 (stream=true 且 config 启用时)
    let is_streaming = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    if is_streaming && config.inject_stream_options_include_usage {
        // 已有 stream_options 时尊重客户端意图, 仅在缺失时注入
        if !out.contains_key("stream_options") {
            out.insert(
                "stream_options".into(),
                json!({"include_usage": true}),
            );
        }
    }

    Ok(Value::Object(out))
}

/// 把 Anthropic system 字段 (string 或 [{type:text,text}, ...]) 拼成单条 string.
/// merge=false 时只取首段 (Phase 1 未启用此分支, 预留).
fn stringify_system(value: &Value, merge: bool) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Array(arr) => {
            let mut parts = Vec::new();
            for item in arr {
                if let Some(t) = item.get("text").and_then(Value::as_str) {
                    parts.push(t.to_string());
                    if !merge {
                        break;
                    }
                }
            }
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("\n\n"))
            }
        }
        _ => None,
    }
}

/// 把单条 Anthropic 消息翻译成 0..N 条 OpenAI 消息 (tool_result 会被拆出独立 role:tool 消息).
fn translate_message(
    role: &str,
    content: Option<&Value>,
    config: &ChatCompletionsTransformConfig,
    out: &mut Vec<Value>,
) -> AppResult<()> {
    let Some(content) = content else {
        return Ok(());
    };

    // content 是 string: 直接传 (user/assistant 通用)
    if let Some(s) = content.as_str() {
        out.push(json!({"role": role, "content": s}));
        return Ok(());
    }

    let Some(blocks) = content.as_array() else {
        return Ok(());
    };

    // 收集各类 block
    let mut text_parts: Vec<Value> = Vec::new(); // 可能含 image_url multipart
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut tool_results: Vec<Value> = Vec::new(); // 每个 result 拆为独立 role:tool 消息
    let mut reasoning_text: Option<String> = None;

    for block in blocks {
        let btype = block.get("type").and_then(Value::as_str).unwrap_or("");
        match btype {
            "text" => {
                if let Some(t) = block.get("text").and_then(Value::as_str) {
                    text_parts.push(json!({"type": "text", "text": t}));
                }
            }
            "image" => {
                let translated = translate_image_block(block, config)?;
                text_parts.push(translated);
            }
            "tool_use" => {
                let id = block.get("id").and_then(Value::as_str).unwrap_or("");
                let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                let input = block.get("input").cloned().unwrap_or(json!({}));
                let args = serde_json::to_string(&input)
                    .unwrap_or_else(|_| "{}".to_string());
                tool_calls.push(json!({
                    "id": id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": args,
                    }
                }));
            }
            "tool_result" => {
                let tool_use_id = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let result_content = block.get("content");
                tool_results.push(json!({
                    "role": "tool",
                    "tool_call_id": tool_use_id,
                    "content": stringify_tool_result(result_content),
                }));
            }
            "thinking" => {
                if config.roundtrip_reasoning {
                    if let Some(t) = block.get("thinking").and_then(Value::as_str) {
                        reasoning_text = Some(t.to_string());
                    }
                }
                // 否则 drop (Phase 1 默认行为)
            }
            _ => {
                // 未知 block type silent skip (Phase 2 可选 strict mode)
            }
        }
    }

    // assistant 消息: 合并 text + tool_calls 成单条; tool_result 不应出现在 assistant
    // user 消息: 合并 text + 拆 tool_results 成多条
    if role == "assistant" {
        let mut msg = Map::new();
        msg.insert("role".into(), json!("assistant"));
        // assistant content 规则:
        // - 多 text/image 块 → array
        // - 单 text 块 → string
        // - 空 + 有 tool_calls → null (修 #5: OpenAI 规范允许 null 但禁空字符串 ""
        //   与 tool_calls 共存; DeepSeek/Groq 等严格中转会直接 400)
        // - 空 + 无 tool_calls → "" (保持 string 兜底, 防止上游 schema 拒收 null)
        if !text_parts.is_empty() {
            msg.insert("content".into(), collapse_text_parts(&text_parts));
        } else if tool_calls.is_empty() {
            msg.insert("content".into(), Value::String(String::new()));
        } else {
            msg.insert("content".into(), Value::Null);
        }
        if !tool_calls.is_empty() {
            msg.insert("tool_calls".into(), Value::Array(tool_calls));
        }
        if let Some(rc) = reasoning_text {
            msg.insert(config.reasoning_field_name.clone(), json!(rc));
        }
        out.push(Value::Object(msg));
        // tool_result 理论不该出现在 assistant 消息里, 但若出现还是要拆出来 (容错)
        for tr in tool_results {
            out.push(tr);
        }
    } else {
        // user / 其他 role
        if !text_parts.is_empty() {
            out.push(json!({
                "role": role,
                "content": collapse_text_parts(&text_parts),
            }));
        }
        for tr in tool_results {
            out.push(tr);
        }
    }

    Ok(())
}

/// 单 text block → string; 多个或含 image → array.
fn collapse_text_parts(parts: &[Value]) -> Value {
    if parts.is_empty() {
        return Value::String(String::new());
    }
    let only_one_text = parts.len() == 1
        && parts[0]
            .get("type")
            .and_then(Value::as_str)
            .map_or(false, |t| t == "text");
    if only_one_text {
        return parts[0]
            .get("text")
            .cloned()
            .unwrap_or_else(|| Value::String(String::new()));
    }
    Value::Array(parts.to_vec())
}

fn translate_image_block(
    block: &Value,
    config: &ChatCompletionsTransformConfig,
) -> AppResult<Value> {
    if config.vision_mode == VisionMode::Error {
        return Err(AppError::BadRequest(
            "当前 provider 配置不支持 image content_block (vision_mode=Error)".into(),
        ));
    }
    let source = block.get("source");
    let url = match source {
        Some(s) => {
            let stype = s.get("type").and_then(Value::as_str).unwrap_or("");
            match stype {
                "base64" => {
                    let media = s
                        .get("media_type")
                        .and_then(Value::as_str)
                        .unwrap_or("image/png");
                    let data = s.get("data").and_then(Value::as_str).unwrap_or("");
                    format!("data:{};base64,{}", media, data)
                }
                "url" => s
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                _ => {
                    return Err(AppError::BadRequest(format!(
                        "未知 image.source.type: {stype}"
                    )))
                }
            }
        }
        None => {
            return Err(AppError::BadRequest("image block 缺少 source 字段".into()));
        }
    };
    Ok(json!({
        "type": "image_url",
        "image_url": {"url": url},
    }))
}

/// tool_result.content 可以是 string 或 [{type:"text",text}, ...], 拼成单 string 给 OpenAI tool message.
fn stringify_tool_result(content: Option<&Value>) -> String {
    let Some(c) = content else {
        return String::new();
    };
    if let Some(s) = c.as_str() {
        return s.to_string();
    }
    if let Some(arr) = c.as_array() {
        let mut parts = Vec::new();
        for item in arr {
            if let Some(t) = item.get("text").and_then(Value::as_str) {
                parts.push(t.to_string());
            }
        }
        return parts.join("\n");
    }
    serde_json::to_string(c).unwrap_or_default()
}

fn translate_tool_definition(t: &Value) -> AppResult<Value> {
    let name = t
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::BadRequest("tool 缺少 name".into()))?;
    let mut function = Map::new();
    function.insert("name".into(), json!(name));
    if let Some(d) = t.get("description") {
        function.insert("description".into(), d.clone());
    }
    let parameters = t
        .get("input_schema")
        .cloned()
        .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
    function.insert("parameters".into(), parameters);
    Ok(json!({
        "type": "function",
        "function": Value::Object(function),
    }))
}

fn translate_tool_choice(tc: &Value) -> Value {
    if let Some(s) = tc.as_str() {
        return match s {
            "auto" => json!("auto"),
            "any" => json!("required"),
            "none" => json!("none"),
            other => json!(other),
        };
    }
    if let Some(obj) = tc.as_object() {
        let ttype = obj.get("type").and_then(Value::as_str).unwrap_or("");
        match ttype {
            "auto" => return json!("auto"),
            "any" => return json!("required"),
            "tool" => {
                let name = obj.get("name").and_then(Value::as_str).unwrap_or("");
                return json!({
                    "type": "function",
                    "function": {"name": name},
                });
            }
            _ => {}
        }
    }
    tc.clone()
}

// ============================================================
// 非流响应方向: OpenAI Chat Completions JSON → Anthropic Messages JSON
// ============================================================

/// 把 OpenAI Chat Completions 非流响应 JSON 翻译成 Anthropic Messages 响应 JSON.
///
/// 输入 schema:
/// ```json
/// {
///   "id": "chatcmpl-xxx", "model": "...",
///   "choices": [{"index":0, "message":{"role":"assistant","content":"...","tool_calls":[...],"reasoning_content":"..."}, "finish_reason":"stop"}],
///   "usage": {"prompt_tokens":N,"completion_tokens":N,"total_tokens":N}
/// }
/// ```
pub fn chat_json_to_anthropic(
    upstream: &Value,
    config: &ChatCompletionsTransformConfig,
    response_id_hint: &str,
) -> AppResult<Value> {
    // 修 #12: 与 SSE 路径 (emit_message_start) 对齐, id 必须以 `msg_` 开头.
    // Anthropic SDK 严格模式按 `msg_*` pattern 校验, 不加前缀会让客户端 schema fail.
    let raw_id = upstream
        .get("id")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .unwrap_or_else(|| response_id_hint.to_string());
    let id = if raw_id.starts_with("msg_") {
        raw_id
    } else {
        format!("msg_{raw_id}")
    };
    let model = upstream
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let choices_arr = upstream
        .get("choices")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::BadRequest("响应缺少 choices 数组".into()))?;
    // 修 #15: 多 choice 仅取首条但 warn (Anthropic Messages 不支持 multi-sample).
    if choices_arr.len() > 1 {
        tracing::warn!(
            choices_count = choices_arr.len(),
            "OpenAI Chat 响应含多个 choice, 仅取 choices[0] 丢弃其余"
        );
    }
    let choice = choices_arr
        .first()
        .ok_or_else(|| AppError::BadRequest("响应缺少 choices[0]".into()))?;

    let message = choice
        .get("message")
        .ok_or_else(|| AppError::BadRequest("choices[0].message 缺失".into()))?;

    let finish_reason = choice.get("finish_reason").and_then(Value::as_str);

    let mut content_blocks: Vec<Value> = Vec::new();

    // reasoning_content → thinking block (放最前)
    if config.expose_reasoning {
        if let Some(rc) = message
            .get(&config.reasoning_field_name)
            .and_then(Value::as_str)
        {
            if !rc.is_empty() {
                content_blocks.push(json!({
                    "type": "thinking",
                    "thinking": rc,
                    "signature": "",
                }));
            }
        }
    }

    // 主 content (string)
    if let Some(s) = message.get("content").and_then(Value::as_str) {
        if !s.is_empty() {
            content_blocks.push(json!({"type": "text", "text": s}));
        }
    } else if let Some(arr) = message.get("content").and_then(Value::as_array) {
        // 少数实现 content 是 multipart; 取文本部分
        for part in arr {
            if let Some(t) = part.get("text").and_then(Value::as_str) {
                content_blocks.push(json!({"type": "text", "text": t}));
            }
        }
    }

    // tool_calls
    if let Some(tcs) = message.get("tool_calls").and_then(Value::as_array) {
        for tc in tcs {
            let tc_id = tc.get("id").and_then(Value::as_str).unwrap_or("");
            let func = tc.get("function");
            let name = func
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let args_str = func
                .and_then(|f| f.get("arguments"))
                .and_then(Value::as_str)
                .unwrap_or("{}");
            // 修 #7: args 解析失败时上抛 502 而非 silent unwrap_or({}). 模型截断输出 /
            // 中转吐出 malformed JSON 时让客户端看到错误而不是被静默给个空参数 tool_use.
            let input: Value = serde_json::from_str(args_str).map_err(|e| {
                AppError::BadGateway(format!(
                    "tool_call.function.arguments 不是合法 JSON ({e}): {}",
                    truncate_for_error(args_str, 256)
                ))
            })?;
            content_blocks.push(json!({
                "type": "tool_use",
                "id": tc_id,
                "name": name,
                "input": input,
            }));
        }
    }

    let stop_reason = map_finish_reason(finish_reason);

    // 修 #14: usage 容错 (float / string) + None vs 0 区分. Anthropic message.usage 必须有
    // 字段, 缺失时给 0 (协议约束); 但调用方 (dispatch 日志) 若拿原始 Option 应另走 helper.
    let usage = upstream.get("usage");
    let input_tokens = usage
        .and_then(|u| u.get("prompt_tokens"))
        .and_then(parse_token_count)
        .unwrap_or(0);
    let output_tokens = usage
        .and_then(|u| u.get("completion_tokens"))
        .and_then(parse_token_count)
        .unwrap_or(0);

    Ok(json!({
        "id": id,
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content_blocks,
        "stop_reason": stop_reason,
        "stop_sequence": Value::Null,
        "usage": {
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
        },
    }))
}

/// 容错解析 OpenAI usage 字段中的 token 数. 上游可能用 u64 (规范) / f64 (Ollama 0.3.x) /
/// string ("5", 某些 one-api fork). 全失败返 None (调用方决定是当 0 还是上报缺失).
pub(crate) fn parse_token_count(v: &Value) -> Option<u64> {
    if let Some(n) = v.as_u64() {
        return Some(n);
    }
    if let Some(f) = v.as_f64() {
        if f >= 0.0 && f.is_finite() {
            return Some(f as u64);
        }
    }
    if let Some(s) = v.as_str() {
        if let Ok(n) = s.parse::<u64>() {
            return Some(n);
        }
    }
    None
}

/// 把超长字符串截短到指定字节数 (UTF-8 安全) 用于 error message 防爆.
fn truncate_for_error(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut cutoff = max_bytes;
    while cutoff > 0 && !s.is_char_boundary(cutoff) {
        cutoff -= 1;
    }
    format!("{}...", &s[..cutoff])
}

/// OpenAI finish_reason → Anthropic stop_reason.
///
/// | OpenAI | Anthropic |
/// |---|---|
/// | stop / null | end_turn |
/// | length | max_tokens |
/// | tool_calls / function_call | tool_use |
/// | content_filter | refusal (Anthropic 标准 stop_reason, 客户端可识别上游审核拦截) |
fn map_finish_reason(fr: Option<&str>) -> &'static str {
    match fr {
        Some("length") => "max_tokens",
        Some("tool_calls") | Some("function_call") => "tool_use",
        // 修 #13: 用 refusal 而非 end_turn, 客户端可区分模型主动停止 vs 风控触发,
        // 避免把空回答塞 history 后下一轮被严格 provider 400.
        Some("content_filter") => "refusal",
        _ => "end_turn",
    }
}

// ============================================================
// SSE 状态机: OpenAI Chat Completions SSE → Anthropic SSE
// ============================================================

#[derive(Debug, Clone, Default)]
struct UsageSnapshot {
    input_tokens: u64,
    output_tokens: u64,
}

#[derive(Debug, Clone, Default)]
struct ToolCallAccumulator {
    /// Anthropic 侧分配的 content_block 索引 (与上游 tool_call index 不同)
    anthropic_index: u32,
    id: String,
    name: String,
    /// 累积已 emit 的 args (调试用, finish 不依赖)
    #[allow(dead_code)]
    args_buf: String,
    /// started=false 期间收到的 arguments 分片暂存于此, content_block_start emit
    /// 后一次性 flush 成首个 input_json_delta. 防止 #1: 首帧仅含 arguments 时 args 丢失.
    pending_args: String,
    started: bool,
}

pub struct ChatCompletionsSseConverter {
    config: ChatCompletionsTransformConfig,
    response_id: String,
    model: String,
    started: bool,
    stopped: bool,
    // 当前已开的 Anthropic 块状态
    text_block_open: bool,
    text_block_index: Option<u32>,
    reasoning_block_open: bool,
    reasoning_block_index: Option<u32>,
    tool_calls: HashMap<u32, ToolCallAccumulator>,
    /// 上游缺 `delta.tool_calls[].index` 时分配的合成 key. 从高位起 (避开正常 0..N),
    /// 配合 by-id 匹配避免 #2: 多个 parallel tool_calls 全部撞到 key 0.
    next_synthetic_tool_idx: u32,
    next_block_index: u32,
    finish_reason: Option<String>,
    usage: Option<UsageSnapshot>,
}

impl ChatCompletionsSseConverter {
    pub fn new(config: ChatCompletionsTransformConfig, response_id: String, model: String) -> Self {
        Self {
            config,
            response_id,
            model,
            started: false,
            stopped: false,
            text_block_open: false,
            text_block_index: None,
            reasoning_block_open: false,
            reasoning_block_index: None,
            tool_calls: HashMap::new(),
            next_synthetic_tool_idx: u32::MAX / 2,
            next_block_index: 0,
            finish_reason: None,
            usage: None,
        }
    }

    /// 喂一行上游 SSE (含或不含 `data: ` 前缀). 返回 0..N 个 Anthropic SSE 帧 (含 `event:` 头).
    /// 调用方应按 `\n\n` 切帧, 每帧单独传入 (或传入整段 raw, 内部按行处理).
    pub fn ingest(&mut self, frame: &str) -> Vec<String> {
        let mut out: Vec<AnthropicEvent> = Vec::new();
        for line in frame.split('\n') {
            let line = line.trim_end_matches('\r');
            let data = if let Some(rest) = line.strip_prefix("data: ") {
                rest
            } else if let Some(rest) = line.strip_prefix("data:") {
                rest.trim_start()
            } else {
                continue;
            };
            if data.trim() == "[DONE]" {
                let finish_events = self.flush_finish();
                out.extend(finish_events);
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            self.process_chunk(&value, &mut out);
        }
        out.into_iter().map(|e| e.to_sse_frame()).collect()
    }

    /// 处理流末: 若上游断流没发 [DONE] / 没发 message_stop, 由调用方触发兜底.
    pub fn finish(&mut self) -> Vec<String> {
        let evs = self.flush_finish();
        evs.into_iter().map(|e| e.to_sse_frame()).collect()
    }

    /// 上游中途出错时调用: 关闭所有已开的 content_block 让客户端的解析器不留悬挂状态.
    /// 不发 message_delta / message_stop —— Anthropic 协议下 `event: error` 是终结事件,
    /// 调用方应在本方法返回的 frames 之后立即发 error 帧并关闭 stream.
    /// 若 converter 尚未 started, 返回空 Vec (调用方应直接发 error, 不需要伪造 message_start).
    pub fn close_open_blocks(&mut self) -> Vec<String> {
        if self.stopped || !self.started {
            return Vec::new();
        }
        let mut out: Vec<AnthropicEvent> = Vec::new();
        self.close_text_block(&mut out);
        self.close_reasoning_block(&mut out);
        let mut tool_indices: Vec<u32> = self
            .tool_calls
            .values()
            .filter(|e| e.started)
            .map(|e| e.anthropic_index)
            .collect();
        tool_indices.sort();
        for idx in tool_indices {
            out.push(AnthropicEvent::ContentBlockStop { index: idx });
        }
        // 注意: 不发 message_delta/message_stop, error 帧本身就是终结.
        self.stopped = true;
        out.into_iter().map(|e| e.to_sse_frame()).collect()
    }

    /// converter 是否已 emit message_start (用于 dispatch 层判断是否需要发 error 前的兜底事件).
    pub fn has_started(&self) -> bool {
        self.started
    }

    /// 末帧 usage.prompt_tokens (用于 dispatch 层 request log).
    pub fn final_input_tokens(&self) -> Option<u64> {
        self.usage.as_ref().map(|u| u.input_tokens)
    }

    /// 末帧 usage.completion_tokens (用于 dispatch 层 request log).
    pub fn final_output_tokens(&self) -> Option<u64> {
        self.usage.as_ref().map(|u| u.output_tokens)
    }

    /// 当前观察到的上游 response id (chatcmpl-xxx). 流末调用获取最终值.
    pub fn response_id(&self) -> &str {
        &self.response_id
    }

    /// 上游响应 model (流构造时初始化, chat completions chunk 通常不改写).
    pub fn response_model(&self) -> &str {
        &self.model
    }

    fn process_chunk(&mut self, chunk: &Value, out: &mut Vec<AnthropicEvent>) {
        // 修 #8: [DONE] 触发 flush_finish 后 stopped=true. 此时若上游再发任何 chunk
        // (反代乱发 / 多个 SSE event coalesce / vLLM 把 usage 拆到 [DONE] 之后),
        // 必须直接丢弃, 否则会在 message_stop 之后又 emit content_block_start, 违反
        // Anthropic 协议, 客户端 SDK 会 panic.
        if self.stopped {
            return;
        }

        // 顶层 usage (含 include_usage 末帧). 修 #14:
        // - 用 parse_token_count 容错 float / string (Ollama / 某些 one-api fork)
        // - 不让中间帧的 all-zero usage 覆盖已有的真实值 (Together/Groq 中转有此 quirk:
        //   每帧都附 usage:{0,0}, 仅末帧填实际值; 若末帧 usage 缺失或被中转吞掉,
        //   留住此前看到的最大值好过全归零)
        if let Some(u) = chunk.get("usage") {
            let new_in = u
                .get("prompt_tokens")
                .and_then(parse_token_count)
                .unwrap_or(0);
            let new_out = u
                .get("completion_tokens")
                .and_then(parse_token_count)
                .unwrap_or(0);
            let is_all_zero = new_in == 0 && new_out == 0;
            let already_have_nonzero = self
                .usage
                .as_ref()
                .map(|s| s.input_tokens > 0 || s.output_tokens > 0)
                .unwrap_or(false);
            if !(is_all_zero && already_have_nonzero) {
                self.usage = Some(UsageSnapshot {
                    input_tokens: new_in,
                    output_tokens: new_out,
                });
            }
        }

        // 优先用 chunk.id 修正 response_id (上游可能在末帧才发完整 id)
        if let Some(id) = chunk.get("id").and_then(Value::as_str) {
            if !id.is_empty() {
                self.response_id = id.to_string();
            }
        }

        let Some(choices) = chunk.get("choices").and_then(Value::as_array) else {
            return;
        };
        let Some(choice) = choices.first() else {
            return;
        };

        // finish_reason 出现 → 记录, 不立即发 message_stop (要等 [DONE] 或上游 EOF)
        if let Some(fr) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(fr.to_string());
        }

        let Some(delta) = choice.get("delta") else {
            return;
        };

        // message_start (第一次见到 delta.role 或任何 content)
        if !self.started {
            self.emit_message_start(out);
        }

        // reasoning_content (在 content 之前, 通常 DeepSeek R1 先发思考再发回答).
        // 修 #6: 反方向 (text 已开后再来 reasoning) 必须先关 text, 否则两 block 同时打开,
        // Anthropic 协议禁止. 部分 R1 中转会乱序交错下发, 实测会触发.
        if self.config.expose_reasoning {
            if let Some(rc) = delta
                .get(&self.config.reasoning_field_name)
                .and_then(Value::as_str)
            {
                if !rc.is_empty() {
                    self.close_text_block(out);
                    self.ensure_reasoning_block(out);
                    if let Some(idx) = self.reasoning_block_index {
                        out.push(AnthropicEvent::ContentBlockDelta {
                            index: idx,
                            delta: json!({"type": "thinking_delta", "thinking": rc}),
                        });
                    }
                }
            }
        }

        // 主 content 文本增量
        if let Some(text) = delta.get("content").and_then(Value::as_str) {
            if !text.is_empty() {
                // 切到 text block 前先关 reasoning (DeepSeek R1: reasoning_content 结束后才发 content)
                self.close_reasoning_block(out);
                self.ensure_text_block(out);
                if let Some(idx) = self.text_block_index {
                    out.push(AnthropicEvent::ContentBlockDelta {
                        index: idx,
                        delta: json!({"type": "text_delta", "text": text}),
                    });
                }
            }
        }

        // tool_calls 增量
        if let Some(tcs) = delta.get("tool_calls").and_then(Value::as_array) {
            for tc in tcs {
                self.process_tool_call_chunk(tc, out);
            }
        }
    }

    fn emit_message_start(&mut self, out: &mut Vec<AnthropicEvent>) {
        self.started = true;
        let msg_id = if self.response_id.starts_with("msg_") {
            self.response_id.clone()
        } else {
            format!("msg_{}", self.response_id)
        };
        let message = json!({
            "id": msg_id,
            "type": "message",
            "role": "assistant",
            "model": self.model,
            "content": [],
            "stop_reason": Value::Null,
            "stop_sequence": Value::Null,
            "usage": {"input_tokens": 0, "output_tokens": 0},
        });
        out.push(AnthropicEvent::MessageStart { message });
    }

    fn ensure_text_block(&mut self, out: &mut Vec<AnthropicEvent>) {
        if self.text_block_open {
            return;
        }
        let idx = self.next_block_index;
        self.next_block_index += 1;
        self.text_block_index = Some(idx);
        self.text_block_open = true;
        out.push(AnthropicEvent::ContentBlockStart {
            index: idx,
            content_block: json!({"type": "text", "text": ""}),
        });
    }

    fn close_text_block(&mut self, out: &mut Vec<AnthropicEvent>) {
        if !self.text_block_open {
            return;
        }
        if let Some(idx) = self.text_block_index {
            out.push(AnthropicEvent::ContentBlockStop { index: idx });
        }
        self.text_block_open = false;
    }

    fn ensure_reasoning_block(&mut self, out: &mut Vec<AnthropicEvent>) {
        if self.reasoning_block_open {
            return;
        }
        let idx = self.next_block_index;
        self.next_block_index += 1;
        self.reasoning_block_index = Some(idx);
        self.reasoning_block_open = true;
        out.push(AnthropicEvent::ContentBlockStart {
            index: idx,
            content_block: json!({
                "type": "thinking",
                "thinking": "",
                "signature": "",
            }),
        });
    }

    fn close_reasoning_block(&mut self, out: &mut Vec<AnthropicEvent>) {
        if !self.reasoning_block_open {
            return;
        }
        if let Some(idx) = self.reasoning_block_index {
            out.push(AnthropicEvent::ContentBlockStop { index: idx });
        }
        self.reasoning_block_open = false;
    }

    fn process_tool_call_chunk(&mut self, tc: &Value, out: &mut Vec<AnthropicEvent>) {
        // 确定 accumulator key. 三层优先:
        // 1. 上游显式 `index` 字段 (OpenAI 规范要求, 大多数实现都有)
        // 2. 缺 index 时按 `id` 查已有 entry (修 #2: 单 chunk 内多 tool_calls 无 index 时
        //    回退到 unwrap_or(0) 会让全部 entry 撞 key 0, 后到的 id 覆盖前者并继续往错误
        //    block 推送 args)
        // 3. id 也缺则分配合成 key (高位起避免与正常 0..N 冲突)
        let upstream_idx = if let Some(i) = tc.get("index").and_then(Value::as_u64) {
            i as u32
        } else if let Some(id) = tc
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            if let Some(k) = self
                .tool_calls
                .iter()
                .find(|(_, e)| e.id == id)
                .map(|(k, _)| *k)
            {
                k
            } else {
                let k = self.next_synthetic_tool_idx;
                self.next_synthetic_tool_idx = self.next_synthetic_tool_idx.saturating_add(1);
                k
            }
        } else {
            let k = self.next_synthetic_tool_idx;
            self.next_synthetic_tool_idx = self.next_synthetic_tool_idx.saturating_add(1);
            k
        };

        // 切到 tool_use 前关掉 text / reasoning 块
        self.close_text_block(out);
        self.close_reasoning_block(out);

        let entry = self.tool_calls.entry(upstream_idx).or_default();

        // 用本帧带来的字段更新 entry
        if let Some(id) = tc.get("id").and_then(Value::as_str) {
            if !id.is_empty() {
                entry.id = id.to_string();
            }
        }
        let mut args_fragment: Option<String> = None;
        if let Some(func) = tc.get("function") {
            if let Some(n) = func.get("name").and_then(Value::as_str) {
                if !n.is_empty() {
                    entry.name = n.to_string();
                }
            }
            if let Some(a) = func.get("arguments").and_then(Value::as_str) {
                if !a.is_empty() {
                    args_fragment = Some(a.to_string());
                }
            }
        }

        // 发 content_block_start 的前提: 已有 id (Anthropic tool_use.id 必填, 没 id 没法回灌 tool_result).
        // name 可能此时仍空 (后续帧才补), 用空 name 暂占位; 但更典型场景是 OpenAI 协议下首帧就把
        // id + name 一起发, 这里仅是兜底.
        if !entry.started && !entry.id.is_empty() {
            let block_idx = self.next_block_index;
            self.next_block_index += 1;
            entry.anthropic_index = block_idx;
            entry.started = true;
            out.push(AnthropicEvent::ContentBlockStart {
                index: block_idx,
                content_block: json!({
                    "type": "tool_use",
                    "id": entry.id,
                    "name": entry.name,
                    "input": {},
                }),
            });
            // 修 #1: started 之前缓冲的 args 一次性 flush 出去
            if !entry.pending_args.is_empty() {
                let pending = std::mem::take(&mut entry.pending_args);
                entry.args_buf.push_str(&pending);
                out.push(AnthropicEvent::ContentBlockDelta {
                    index: block_idx,
                    delta: json!({"type": "input_json_delta", "partial_json": pending}),
                });
            }
        }

        // 本帧的 args 分片: started 后直接 emit; 否则缓冲到 pending_args 等 id 到来
        if let Some(args) = args_fragment {
            if entry.started {
                entry.args_buf.push_str(&args);
                out.push(AnthropicEvent::ContentBlockDelta {
                    index: entry.anthropic_index,
                    delta: json!({"type": "input_json_delta", "partial_json": args}),
                });
            } else {
                entry.pending_args.push_str(&args);
            }
        }
    }

    fn flush_finish(&mut self) -> Vec<AnthropicEvent> {
        if self.stopped {
            return Vec::new();
        }
        let mut out = Vec::new();
        // 关所有未关的内容块
        self.close_text_block(&mut out);
        self.close_reasoning_block(&mut out);
        // 关所有 tool_use 块 (按 anthropic_index 顺序)
        let mut tool_entries: Vec<(u32, u32)> = self
            .tool_calls
            .values()
            .filter(|e| e.started)
            .map(|e| (e.anthropic_index, e.anthropic_index))
            .collect();
        tool_entries.sort_by_key(|(idx, _)| *idx);
        for (idx, _) in tool_entries {
            out.push(AnthropicEvent::ContentBlockStop { index: idx });
        }

        // message_delta + message_stop (前提是已 started; 否则跳过, 调用方不该发空流)
        if self.started {
            let stop_reason = map_finish_reason(self.finish_reason.as_deref());
            let usage_snap = self.usage.clone().unwrap_or_default();
            out.push(AnthropicEvent::MessageDelta {
                delta: json!({
                    "stop_reason": stop_reason,
                    "stop_sequence": Value::Null,
                }),
                usage: json!({
                    "input_tokens": usage_snap.input_tokens,
                    "output_tokens": usage_snap.output_tokens,
                }),
            });
            out.push(AnthropicEvent::MessageStop);
        }
        self.stopped = true;
        out
    }
}

// ============================================================
// 单测
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ChatCompletionsTransformConfig {
        ChatCompletionsTransformConfig::permissive()
    }

    fn extras() -> ChatCompletionsExtras {
        ChatCompletionsExtras {
            reasoning_effort: None,
            expose_reasoning: true,
        }
    }

    // -------- 请求方向 --------

    #[test]
    fn anthropic_to_chat_basic_user_message_only() {
        let body = json!({
            "model": "deepseek-chat",
            "messages": [{"role": "user", "content": "hello"}],
        });
        let out = anthropic_to_openai_chat(&body, &cfg(), &extras()).unwrap();
        assert_eq!(out["model"], "deepseek-chat");
        let messages = out["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "hello");
    }

    #[test]
    fn anthropic_to_chat_system_string_merged_to_first_message() {
        let body = json!({
            "model": "m",
            "system": "You are helpful.",
            "messages": [{"role": "user", "content": "hi"}],
        });
        let out = anthropic_to_openai_chat(&body, &cfg(), &extras()).unwrap();
        let messages = out["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "You are helpful.");
    }

    #[test]
    fn anthropic_to_chat_system_array_joined() {
        let body = json!({
            "model": "m",
            "system": [
                {"type": "text", "text": "Part 1"},
                {"type": "text", "text": "Part 2"},
            ],
            "messages": [{"role": "user", "content": "hi"}],
        });
        let out = anthropic_to_openai_chat(&body, &cfg(), &extras()).unwrap();
        let sys = &out["messages"][0];
        assert_eq!(sys["role"], "system");
        assert_eq!(sys["content"], "Part 1\n\nPart 2");
    }

    #[test]
    fn anthropic_to_chat_tool_definition_mapping() {
        let body = json!({
            "model": "m",
            "messages": [],
            "tools": [{
                "name": "search",
                "description": "Search docs",
                "input_schema": {"type": "object", "properties": {"q": {"type": "string"}}},
            }],
        });
        let out = anthropic_to_openai_chat(&body, &cfg(), &extras()).unwrap();
        let tool = &out["tools"][0];
        assert_eq!(tool["type"], "function");
        assert_eq!(tool["function"]["name"], "search");
        assert_eq!(tool["function"]["description"], "Search docs");
        assert_eq!(tool["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn anthropic_to_chat_tool_use_assistant_to_tool_calls() {
        let body = json!({
            "model": "m",
            "messages": [
                {"role": "user", "content": "search rust"},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "Sure, searching."},
                    {"type": "tool_use", "id": "call_1", "name": "search", "input": {"q": "rust"}},
                ]},
            ],
        });
        let out = anthropic_to_openai_chat(&body, &cfg(), &extras()).unwrap();
        let messages = out["messages"].as_array().unwrap();
        let assistant = &messages[1];
        assert_eq!(assistant["role"], "assistant");
        assert_eq!(assistant["content"], "Sure, searching.");
        let tcs = assistant["tool_calls"].as_array().unwrap();
        assert_eq!(tcs[0]["id"], "call_1");
        assert_eq!(tcs[0]["type"], "function");
        assert_eq!(tcs[0]["function"]["name"], "search");
        let args: Value = serde_json::from_str(tcs[0]["function"]["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(args, json!({"q": "rust"}));
    }

    #[test]
    fn anthropic_to_chat_tool_result_to_separate_role_tool_message() {
        let body = json!({
            "model": "m",
            "messages": [
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "call_1", "content": "Result text"},
                ]},
            ],
        });
        let out = anthropic_to_openai_chat(&body, &cfg(), &extras()).unwrap();
        let messages = out["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "tool");
        assert_eq!(messages[0]["tool_call_id"], "call_1");
        assert_eq!(messages[0]["content"], "Result text");
    }

    #[test]
    fn anthropic_to_chat_image_base64_to_data_url() {
        let body = json!({
            "model": "m",
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "what's this"},
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}},
            ]}],
        });
        let out = anthropic_to_openai_chat(&body, &cfg(), &extras()).unwrap();
        let content = out["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(content[1]["image_url"]["url"], "data:image/png;base64,AAAA");
    }

    #[test]
    fn anthropic_to_chat_image_url_passthrough() {
        let body = json!({
            "model": "m",
            "messages": [{"role": "user", "content": [
                {"type": "image", "source": {"type": "url", "url": "https://example.com/x.jpg"}},
            ]}],
        });
        let out = anthropic_to_openai_chat(&body, &cfg(), &extras()).unwrap();
        assert_eq!(
            out["messages"][0]["content"][0]["image_url"]["url"],
            "https://example.com/x.jpg"
        );
    }

    #[test]
    fn anthropic_to_chat_vision_mode_error_rejects_image() {
        let mut c = cfg();
        c.vision_mode = VisionMode::Error;
        let body = json!({
            "model": "m",
            "messages": [{"role": "user", "content": [
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "x"}},
            ]}],
        });
        assert!(anthropic_to_openai_chat(&body, &c, &extras()).is_err());
    }

    #[test]
    fn anthropic_to_chat_thinking_block_dropped_by_default() {
        let body = json!({
            "model": "m",
            "messages": [
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "Let me think...", "signature": ""},
                    {"type": "text", "text": "ok"},
                ]},
            ],
        });
        let out = anthropic_to_openai_chat(&body, &cfg(), &extras()).unwrap();
        let assistant = &out["messages"][0];
        assert!(assistant.get("reasoning_content").is_none());
        assert_eq!(assistant["content"], "ok");
    }

    #[test]
    fn anthropic_to_chat_thinking_block_roundtrip_when_enabled() {
        let mut c = cfg();
        c.roundtrip_reasoning = true;
        let body = json!({
            "model": "m",
            "messages": [
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "Let me think...", "signature": ""},
                    {"type": "text", "text": "ok"},
                ]},
            ],
        });
        let out = anthropic_to_openai_chat(&body, &c, &extras()).unwrap();
        let assistant = &out["messages"][0];
        assert_eq!(assistant["reasoning_content"], "Let me think...");
        assert_eq!(assistant["content"], "ok");
    }

    #[test]
    fn anthropic_to_chat_stream_true_injects_include_usage() {
        let body = json!({
            "model": "m",
            "stream": true,
            "messages": [{"role": "user", "content": "hi"}],
        });
        let out = anthropic_to_openai_chat(&body, &cfg(), &extras()).unwrap();
        assert_eq!(out["stream_options"]["include_usage"], true);
    }

    #[test]
    fn anthropic_to_chat_drop_max_tokens_when_configured() {
        let mut c = cfg();
        c.drop_max_tokens = true;
        let body = json!({
            "model": "m",
            "max_tokens": 1024,
            "messages": [{"role": "user", "content": "hi"}],
        });
        let out = anthropic_to_openai_chat(&body, &c, &extras()).unwrap();
        assert!(out.get("max_tokens").is_none());
    }

    #[test]
    fn anthropic_to_chat_tool_choice_variants() {
        let mk = |tc: Value| {
            json!({
                "model": "m",
                "messages": [],
                "tool_choice": tc,
            })
        };
        let out1 = anthropic_to_openai_chat(&mk(json!("auto")), &cfg(), &extras()).unwrap();
        assert_eq!(out1["tool_choice"], "auto");
        let out2 = anthropic_to_openai_chat(&mk(json!("any")), &cfg(), &extras()).unwrap();
        assert_eq!(out2["tool_choice"], "required");
        let out3 = anthropic_to_openai_chat(
            &mk(json!({"type": "tool", "name": "search"})),
            &cfg(),
            &extras(),
        )
        .unwrap();
        assert_eq!(out3["tool_choice"]["type"], "function");
        assert_eq!(out3["tool_choice"]["function"]["name"], "search");
    }

    #[test]
    fn anthropic_to_chat_stop_sequences_to_stop() {
        let body = json!({
            "model": "m",
            "stop_sequences": ["\n\n", "END"],
            "messages": [{"role": "user", "content": "hi"}],
        });
        let out = anthropic_to_openai_chat(&body, &cfg(), &extras()).unwrap();
        assert_eq!(out["stop"], json!(["\n\n", "END"]));
    }

    #[test]
    fn anthropic_to_chat_reasoning_effort_propagated() {
        let body = json!({
            "model": "m",
            "messages": [{"role": "user", "content": "hi"}],
        });
        let mut e = extras();
        e.reasoning_effort = Some(ReasoningEffort::High);
        let out = anthropic_to_openai_chat(&body, &cfg(), &e).unwrap();
        assert_eq!(out["reasoning_effort"], "high");
    }

    // -------- 非流响应方向 --------

    #[test]
    fn chat_json_to_anthropic_simple_text_response() {
        let up = json!({
            "id": "chatcmpl-abc",
            "model": "deepseek-chat",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "Hello!"},
                "finish_reason": "stop",
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 10, "total_tokens": 15},
        });
        let out = chat_json_to_anthropic(&up, &cfg(), "fallback").unwrap();
        // 修 #12 后: 非流式 id 也必须加 msg_ 前缀对齐 SSE 路径
        assert_eq!(out["id"], "msg_chatcmpl-abc");
        assert_eq!(out["stop_reason"], "end_turn");
        assert_eq!(out["content"][0]["type"], "text");
        assert_eq!(out["content"][0]["text"], "Hello!");
        assert_eq!(out["usage"]["input_tokens"], 5);
        assert_eq!(out["usage"]["output_tokens"], 10);
    }

    #[test]
    fn chat_json_to_anthropic_tool_calls_response() {
        let up = json!({
            "id": "chatcmpl-x",
            "model": "deepseek-chat",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "search", "arguments": "{\"q\":\"rust\"}"},
                    }],
                },
                "finish_reason": "tool_calls",
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 10},
        });
        let out = chat_json_to_anthropic(&up, &cfg(), "fallback").unwrap();
        assert_eq!(out["stop_reason"], "tool_use");
        assert_eq!(out["content"][0]["type"], "tool_use");
        assert_eq!(out["content"][0]["id"], "call_1");
        assert_eq!(out["content"][0]["name"], "search");
        assert_eq!(out["content"][0]["input"], json!({"q": "rust"}));
    }

    #[test]
    fn chat_json_to_anthropic_reasoning_content_becomes_thinking_block() {
        let up = json!({
            "id": "chatcmpl-x",
            "model": "deepseek-reasoner",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Final answer.",
                    "reasoning_content": "Let me think step by step.",
                },
                "finish_reason": "stop",
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 10},
        });
        let out = chat_json_to_anthropic(&up, &cfg(), "fallback").unwrap();
        // thinking 在 text 之前
        assert_eq!(out["content"][0]["type"], "thinking");
        assert_eq!(out["content"][0]["thinking"], "Let me think step by step.");
        assert_eq!(out["content"][1]["type"], "text");
        assert_eq!(out["content"][1]["text"], "Final answer.");
    }

    /// 修 #12: 非流式 id 必须加 msg_ 前缀对齐 SSE 路径
    #[test]
    fn chat_json_to_anthropic_id_gets_msg_prefix() {
        let up = json!({
            "id": "chatcmpl-abc",
            "model": "m",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"}, "finish_reason": "stop"}],
        });
        let out = chat_json_to_anthropic(&up, &cfg(), "fb").unwrap();
        assert_eq!(out["id"], "msg_chatcmpl-abc");
    }

    /// 修 #12: 已带 msg_ 前缀的 id 不重复拼接
    #[test]
    fn chat_json_to_anthropic_id_with_msg_prefix_unchanged() {
        let up = json!({
            "id": "msg_already_prefixed",
            "model": "m",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"}, "finish_reason": "stop"}],
        });
        let out = chat_json_to_anthropic(&up, &cfg(), "fb").unwrap();
        assert_eq!(out["id"], "msg_already_prefixed");
    }

    /// 修 #7: malformed tool_call.arguments 必须上抛 502 而非 silent {} 兜底
    #[test]
    fn chat_json_to_anthropic_malformed_tool_args_returns_bad_gateway() {
        let up = json!({
            "id": "x",
            "model": "m",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "c1",
                        "type": "function",
                        "function": {"name": "s", "arguments": "{\"q\":"},
                    }],
                },
                "finish_reason": "tool_calls",
            }],
        });
        let err = chat_json_to_anthropic(&up, &cfg(), "fb").unwrap_err();
        assert!(
            matches!(err, AppError::BadGateway(_)),
            "应返回 BadGateway 而非 silent {{}} 替换"
        );
    }

    /// 修 #13: content_filter → refusal (而非 end_turn)
    #[test]
    fn chat_json_to_anthropic_content_filter_to_refusal() {
        let up = json!({
            "id": "x",
            "model": "m",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": ""}, "finish_reason": "content_filter"}],
        });
        let out = chat_json_to_anthropic(&up, &cfg(), "fb").unwrap();
        assert_eq!(out["stop_reason"], "refusal");
    }

    /// 修 #14: usage 字段 float / string 类型容错
    #[test]
    fn parse_token_count_accepts_int_float_string() {
        assert_eq!(parse_token_count(&json!(5)), Some(5));
        assert_eq!(parse_token_count(&json!(5.0)), Some(5));
        assert_eq!(parse_token_count(&json!("5")), Some(5));
        assert_eq!(parse_token_count(&json!(null)), None);
        assert_eq!(parse_token_count(&json!(-1.0)), None); // 负数拒绝
        assert_eq!(parse_token_count(&json!("not a number")), None);
    }

    /// 修 #15: anthropic_to_openai_chat 不透传 n 字段
    #[test]
    fn anthropic_to_chat_strips_n_field() {
        let body = json!({
            "model": "m",
            "n": 3,
            "messages": [{"role": "user", "content": "hi"}],
        });
        let out = anthropic_to_openai_chat(&body, &cfg(), &extras()).unwrap();
        assert!(out.get("n").is_none(), "n 字段必须被 strip");
    }

    /// 修 #14 (中间帧 zero-usage 覆盖): 中间帧 usage:{0,0} 不应覆盖已有的真实值
    #[test]
    fn sse_converter_mid_chunk_zero_usage_does_not_overwrite_real_value() {
        let mut c = ChatCompletionsSseConverter::new(cfg(), "x".into(), "m".into());
        let frames = [
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"},\"finish_reason\":null}],\"usage\":{\"prompt_tokens\":50,\"completion_tokens\":2}}\n\n",
            // Together/Groq quirk: 中间帧又来 usage:{0,0}
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":0,\"completion_tokens\":0}}\n\n",
            "data: [DONE]\n\n",
        ];
        let _ = collect_sse(&mut c, &frames);
        assert_eq!(c.final_input_tokens(), Some(50), "真实 usage 不应被零覆盖");
        assert_eq!(c.final_output_tokens(), Some(2));
    }

    #[test]
    fn chat_json_to_anthropic_finish_reason_length_to_max_tokens() {
        let up = json!({
            "id": "x",
            "model": "m",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "..."}, "finish_reason": "length"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 100},
        });
        let out = chat_json_to_anthropic(&up, &cfg(), "fb").unwrap();
        assert_eq!(out["stop_reason"], "max_tokens");
    }

    // -------- SSE 状态机 --------

    fn collect_sse(c: &mut ChatCompletionsSseConverter, frames: &[&str]) -> Vec<String> {
        let mut out = Vec::new();
        for f in frames {
            out.extend(c.ingest(f));
        }
        out
    }

    #[test]
    fn sse_converter_incremental_text_delta_multi_chunk() {
        let mut c = ChatCompletionsSseConverter::new(
            cfg(),
            "chatcmpl-1".into(),
            "deepseek-chat".into(),
        );
        let frames = [
            "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\" world\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2}}\n\n",
            "data: [DONE]\n\n",
        ];
        let out = collect_sse(&mut c, &frames);
        let joined = out.join("");
        assert!(joined.contains("event: message_start"));
        assert!(joined.contains("event: content_block_start"));
        assert!(joined.contains("\"text\":\"Hello\""));
        assert!(joined.contains("\"text\":\" world\""));
        assert!(joined.contains("event: content_block_stop"));
        assert!(joined.contains("event: message_delta"));
        assert!(joined.contains("\"stop_reason\":\"end_turn\""));
        assert!(joined.contains("\"input_tokens\":5"));
        assert!(joined.contains("\"output_tokens\":2"));
        assert!(joined.contains("event: message_stop"));
    }

    #[test]
    fn sse_converter_tool_call_incremental_arguments() {
        let mut c = ChatCompletionsSseConverter::new(cfg(), "x".into(), "m".into());
        let frames = [
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"search\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"q\\\":\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"rust\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":7}}\n\n",
            "data: [DONE]\n\n",
        ];
        let out = collect_sse(&mut c, &frames);
        let joined = out.join("");
        assert!(joined.contains("\"type\":\"tool_use\""));
        assert!(joined.contains("\"id\":\"call_1\""));
        assert!(joined.contains("\"name\":\"search\""));
        assert!(joined.contains("\"type\":\"input_json_delta\""));
        assert!(joined.contains("\"stop_reason\":\"tool_use\""));
    }

    #[test]
    fn sse_converter_reasoning_content_when_expose_true() {
        let mut c = ChatCompletionsSseConverter::new(cfg(), "x".into(), "deepseek-reasoner".into());
        let frames = [
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"Thinking\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\" more\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Answer\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":4}}\n\n",
            "data: [DONE]\n\n",
        ];
        let out = collect_sse(&mut c, &frames);
        let joined = out.join("");
        // thinking block 应在 text block 之前发出
        let thinking_pos = joined.find("\"type\":\"thinking\"").unwrap();
        let text_pos = joined.find("\"type\":\"text\"").unwrap();
        assert!(thinking_pos < text_pos);
        assert!(joined.contains("\"thinking_delta\""));
        assert!(joined.contains("\"thinking\":\"Thinking\""));
        assert!(joined.contains("\"text\":\"Answer\""));
    }

    #[test]
    fn sse_converter_reasoning_dropped_when_expose_false() {
        let mut c_cfg = cfg();
        c_cfg.expose_reasoning = false;
        let mut c = ChatCompletionsSseConverter::new(c_cfg, "x".into(), "m".into());
        let frames = [
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"hidden\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"visible\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n",
            "data: [DONE]\n\n",
        ];
        let out = collect_sse(&mut c, &frames);
        let joined = out.join("");
        assert!(!joined.contains("\"thinking\""));
        assert!(joined.contains("\"text\":\"visible\""));
    }

    #[test]
    fn sse_converter_finish_without_done_sentinel_via_explicit_finish() {
        let mut c = ChatCompletionsSseConverter::new(cfg(), "x".into(), "m".into());
        let frames = [
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n",
        ];
        let mut out = collect_sse(&mut c, &frames);
        // 没收到 [DONE], 调用方触发 finish
        out.extend(c.finish());
        let joined = out.join("");
        assert!(joined.contains("event: message_stop"));
    }

    #[test]
    fn sse_converter_msg_id_prefix_added_when_missing() {
        let mut c = ChatCompletionsSseConverter::new(cfg(), "raw-id".into(), "m".into());
        let frames = [
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
            "data: [DONE]\n\n",
        ];
        let out = collect_sse(&mut c, &frames);
        let joined = out.join("");
        assert!(joined.contains("\"id\":\"msg_raw-id\""));
    }

    /// 修 #1: 首帧仅含 arguments, 第二帧才发 id+name. 旧实现会丢首帧 args
    /// 且后续 content_block_start 用空 name.
    #[test]
    fn sse_converter_tool_call_args_before_id_buffered_and_flushed() {
        let mut c = ChatCompletionsSseConverter::new(cfg(), "x".into(), "m".into());
        let frames = [
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
            // 首帧仅 args, 无 id/name
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"q\\\":\"}}]},\"finish_reason\":null}]}\n\n",
            // 第二帧补 id + name + 继续 args
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"search\",\"arguments\":\"\\\"rust\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n",
            "data: [DONE]\n\n",
        ];
        let out = collect_sse(&mut c, &frames);
        let joined = out.join("");
        // content_block_start 必须有正确的 id 和 name (不能空)
        assert!(joined.contains("\"id\":\"call_1\""), "tool_use 必须含 id");
        assert!(joined.contains("\"name\":\"search\""), "tool_use 必须含 name");
        // 首帧的 args 片段必须 flush 出去 (不能丢)
        assert!(
            joined.contains("\"partial_json\":\"{\\\"q\\\":\""),
            "首帧 args 片段必须保留并 emit"
        );
        // 第二帧的 args 片段也必须 emit
        assert!(joined.contains("\"partial_json\":\"\\\"rust\\\"}\""));
    }

    /// 修 #2: 单 chunk 内多个 tool_calls 全部缺 `index` 字段, 必须按 id 分到不同 block
    /// 而不是 unwrap_or(0) 撞 key.
    #[test]
    fn sse_converter_parallel_tool_calls_without_index_field() {
        let mut c = ChatCompletionsSseConverter::new(cfg(), "x".into(), "m".into());
        let frames = [
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
            // 同一 chunk 两个 tool_calls, 均无 index
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"id\":\"call_a\",\"type\":\"function\",\"function\":{\"name\":\"foo\",\"arguments\":\"{\\\"x\\\":1}\"}},{\"id\":\"call_b\",\"type\":\"function\",\"function\":{\"name\":\"bar\",\"arguments\":\"{\\\"y\\\":2}\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n",
            "data: [DONE]\n\n",
        ];
        let out = collect_sse(&mut c, &frames);
        let joined = out.join("");
        // 必须出现两个独立的 tool_use block, 各自的 id/name 不能错位
        assert!(joined.contains("\"id\":\"call_a\""), "call_a 必须保留");
        assert!(joined.contains("\"id\":\"call_b\""), "call_b 必须保留");
        assert!(joined.contains("\"name\":\"foo\""));
        assert!(joined.contains("\"name\":\"bar\""));
        // 两个 args 都要 emit
        assert!(joined.contains("\"partial_json\":\"{\\\"x\\\":1}\""));
        assert!(joined.contains("\"partial_json\":\"{\\\"y\\\":2}\""));
        // 两个 content_block_start (除 message_start 自身的开始事件外, 还应有 2 个 tool_use 块开始)
        let tool_use_starts = joined.matches("\"type\":\"tool_use\"").count();
        assert_eq!(tool_use_starts, 2, "应有两个独立 tool_use content_block");
    }

    /// 修 #5: assistant 消息只有 tool_use 时, content 必须是 null 而非 ""; 否则
    /// DeepSeek/Groq 等严格中转 400.
    #[test]
    fn anthropic_to_chat_assistant_with_only_tool_use_emits_null_content() {
        let body = json!({
            "model": "m",
            "messages": [
                {"role": "user", "content": "search"},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "c1", "name": "s", "input": {"q": "rust"}},
                ]},
            ],
        });
        let out = anthropic_to_openai_chat(&body, &cfg(), &extras()).unwrap();
        let assistant = &out["messages"][1];
        assert_eq!(assistant["role"], "assistant");
        assert!(assistant["content"].is_null(), "无 text 时 content 必须 null");
        assert_eq!(assistant["tool_calls"].as_array().unwrap().len(), 1);
    }

    /// #5 反例: 没 tool_calls 时 content 应 string "" (不破现有行为)
    #[test]
    fn anthropic_to_chat_assistant_empty_no_tool_calls_still_empty_string() {
        let body = json!({
            "model": "m",
            "messages": [{"role": "assistant", "content": []}],
        });
        let out = anthropic_to_openai_chat(&body, &cfg(), &extras()).unwrap();
        let assistant = &out["messages"][0];
        assert_eq!(assistant["content"], "");
    }

    /// 修 #6: text 已开后再来 reasoning_content 必须先 close_text_block.
    /// 修 #11: started 后调用 close_open_blocks 应关掉 text/reasoning/tool block
    /// 但不发 message_delta/message_stop (Anthropic 协议: error 帧是终结事件).
    #[test]
    fn sse_converter_close_open_blocks_after_started_emits_block_stops_only() {
        let mut c = ChatCompletionsSseConverter::new(cfg(), "x".into(), "m".into());
        // 推进到 started + text block open + reasoning block open
        let frames = [
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"r\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"t\"},\"finish_reason\":null}]}\n\n",
        ];
        let _ = collect_sse(&mut c, &frames);
        assert!(c.has_started());

        let closing = c.close_open_blocks();
        let joined = closing.join("");
        // 必须含 content_block_stop, 不能有 message_stop (后者由 error 帧自身终结流)
        assert!(joined.contains("event: content_block_stop"));
        assert!(!joined.contains("event: message_stop"));
        assert!(!joined.contains("event: message_delta"));
    }

    /// 修 #11: 未 started 调用 close_open_blocks 返空 (dispatch 层直接发 error 帧, 无需关空块)
    #[test]
    fn sse_converter_close_open_blocks_when_not_started_returns_empty() {
        let mut c = ChatCompletionsSseConverter::new(cfg(), "x".into(), "m".into());
        assert!(!c.has_started());
        let out = c.close_open_blocks();
        assert!(out.is_empty(), "未 started 时不应 emit 任何帧");
    }

    #[test]
    fn sse_converter_text_then_reasoning_closes_text_first() {
        let mut c = ChatCompletionsSseConverter::new(cfg(), "x".into(), "m".into());
        let frames = [
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n",
            // 异常交错: text 之后又来 reasoning_content
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"second-thought\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n",
            "data: [DONE]\n\n",
        ];
        let out = collect_sse(&mut c, &frames);
        let joined = out.join("");
        // 必须有 content_block_stop 在 thinking block 开始之前出现
        let first_stop = joined
            .match_indices("\"content_block_stop\"")
            .next()
            .map(|(i, _)| i)
            .expect("应有至少一个 content_block_stop");
        let thinking_start = joined
            .find("\"type\":\"thinking\"")
            .expect("thinking block 必须出现");
        assert!(
            first_stop < thinking_start,
            "text block 必须在 reasoning 开始前关闭"
        );
    }

    /// 修 #8: [DONE] 之后的 chunk 必须被忽略 (stopped guard).
    #[test]
    fn sse_converter_chunks_after_done_are_ignored() {
        let mut c = ChatCompletionsSseConverter::new(cfg(), "x".into(), "m".into());
        let frames = [
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n",
            "data: [DONE]\n\n",
            // 反代乱发: [DONE] 之后又来一个 chunk
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"GHOST\"},\"finish_reason\":null}]}\n\n",
        ];
        let out = collect_sse(&mut c, &frames);
        let joined = out.join("");
        assert!(
            !joined.contains("GHOST"),
            "stopped 后的 chunk 必须被丢弃, 不应再 emit"
        );
        // message_stop 应只发一次
        assert_eq!(
            joined.matches("event: message_stop").count(),
            1,
            "message_stop 应只发一次"
        );
    }

    #[test]
    fn sse_converter_text_then_tool_call_closes_text_first() {
        let mut c = ChatCompletionsSseConverter::new(cfg(), "x".into(), "m".into());
        let frames = [
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Let me search.\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"type\":\"function\",\"function\":{\"name\":\"s\",\"arguments\":\"{}\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n",
            "data: [DONE]\n\n",
        ];
        let out = collect_sse(&mut c, &frames);
        let joined = out.join("");
        // 0 号 block (text) 必须先 stop, 然后才出现 tool_use 的 1 号 block_start
        let text_stop_pos = joined
            .match_indices("\"content_block_stop\"")
            .next()
            .unwrap()
            .0;
        let tool_start_pos = joined.find("\"type\":\"tool_use\"").unwrap();
        assert!(text_stop_pos < tool_start_pos);
    }
}
