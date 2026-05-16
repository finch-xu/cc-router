//! Google Gemini generateContent ↔ Anthropic Messages 协议翻译.
//!
//! 协议事实 (依据 https://ai.google.dev/gemini-api/docs):
//! - 上游 endpoint: `{base_url}/v1beta/models/{model}:streamGenerateContent?alt=sse`
//!   或 `:generateContent` (非流式). cc-router 始终走 SSE 流式版本, 客户端要非流式时
//!   由 `NonStreamingCollector` 收齐 SSE 帧再吐 Anthropic JSON.
//! - 认证: `x-goog-api-key: <API_KEY>` header.
//! - SSE 每帧是完整 JSON, 形如:
//!   `data: {"candidates":[{"content":{"role":"model","parts":[...]}, "finishReason":"STOP"?}], "usageMetadata":{...}?}\n\n`
//!   注意: 没有 `event: foo` 头, 解析时只取 `data:` 行.
//!
//! ## 请求侧 (anthropic_to_gemini)
//!
//! | Anthropic | Gemini |
//! |---|---|
//! | `model` | 不进 body, 由 dispatch 层填到 URL path 里 |
//! | `system` (str/array) | `systemInstruction: {parts: [{text}]}` (array 合并为单 text) |
//! | `messages[].role: "user"` | `contents[].role: "user"` |
//! | `messages[].role: "assistant"` | `contents[].role: "model"` |
//! | `content: str` | `parts: [{text}]` |
//! | `content[].type: "text"` | `parts[].text` |
//! | `content[].type: "image", source.{data,media_type}` | `parts[].inlineData: {mimeType, data}` |
//! | `content[].type: "tool_use"` | `parts[].functionCall: {name, args}` (id 丢, 翻译层无状态依赖) |
//! | `content[].type: "tool_result"` | `parts[].functionResponse: {name, response: {result}}` |
//! | `temperature/top_p/max_tokens/stop_sequences` | `generationConfig.{temperature,topP,maxOutputTokens,stopSequences}` |
//! | `tools[]` (input_schema) | `tools[0].functionDeclarations: [{name, description, parameters}]` |
//! | `tool_choice: auto/any/none/tool` | `toolConfig.functionCallingConfig.mode: AUTO/ANY/NONE` |
//! | `thinking` block | `parts[].thoughtSignature` 回灌 (yaml `expose_reasoning: true` 时) |
//! | `thinking.budget_tokens` / `thinking.effort` | `generationConfig.thinkingConfig.{thinkingBudget,includeThoughts}` |
//!
//! 仅 Gemini 2.5+ 系列原生支持 `thinkingConfig`. 2.0/1.5 系列上游静默忽略, cc-router 不门控。
//!
//! ## 响应侧 (GeminiSseConverter)
//!
//! 每帧 JSON 触发一次状态机, 可能 emit 0..多个 Anthropic SSE 事件:
//! - 首帧: emit `message_start`
//! - text part: 累积到当前 text block, emit `content_block_delta (text_delta)`. 若当前不在
//!   text block, 先开新块 (close 上一块 if any).
//! - functionCall part: close text block if open, 开新 tool_use 块, args 一次性 dump 成
//!   `input_json_delta`, close 块. (Gemini 不流式 args.)
//! - 帧含 `finishReason` 非 UNSPECIFIED: close 当前块, 记录 stop_reason.
//! - `usageMetadata`: 累积到 `final_usage` (每帧都可能含, 取末值).
//! - 流结束 (finalize): emit 收尾 `content_block_stop` (if needed) + `message_delta` + `message_stop`.

use std::collections::HashMap;

use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

// ============================================================
// Extras (dispatch 层填入)
// ============================================================

/// `anthropic_to_gemini` 的可选项. dispatch 层从 yaml / 客户端 body 推导后传入.
#[derive(Debug, Clone, Default)]
pub struct GeminiExtras {
    /// `generationConfig.thinkingConfig.thinkingBudget` 值.
    /// `Some(-1)`: 动态预算; `Some(0)`: 关闭思考; `Some(>0)`: 显式上限; `None`: 不注入 thinkingBudget。
    pub thinking_budget: Option<i64>,
    /// `generationConfig.thinkingConfig.includeThoughts` 值. yaml `expose_reasoning` 字段映射来。
    /// 同时控制响应侧是否暴露 thought parts + 是否回灌 thinking block 的 thoughtSignature。
    pub include_thoughts: bool,
}

/// Anthropic `thinking.effort` → Gemini `thinkingBudget` 整数预算阈值. 与 openai.rs 的
/// `ReasoningEffort::from_budget_tokens` 阈值对齐 (反向)。
pub fn effort_to_budget(effort: &str) -> Option<i64> {
    match effort {
        "minimal" => Some(512),
        "low" => Some(4096),
        "medium" => Some(16384),
        "high" => Some(65536),
        _ => None,
    }
}

/// thinking budget 优先级链:
/// 1. `body.thinking.budget_tokens` (整数, 直接透传)
/// 2. `body.thinking.effort` (string, 走 [`effort_to_budget`] 映射)
/// 3. `body.extra_body.reasoning_effort` (string, 同上)
/// 4. `yaml_default_effort` (provider yaml `default_reasoning_effort`, 同上)
///
/// 任意一档为空/非法都视为缺失继续找。返回 None 表示不注入 thinkingBudget,
/// Gemini 后端会按默认 (2.5 系列约 -1 动态) 处理。
pub fn resolve_thinking_budget(body: &Value, yaml_default_effort: Option<&str>) -> Option<i64> {
    if let Some(thinking) = body.get("thinking") {
        if let Some(bt) = thinking.get("budget_tokens").and_then(|x| x.as_i64()) {
            return Some(bt);
        }
        if let Some(s) = thinking.get("effort").and_then(|x| x.as_str()) {
            if let Some(b) = effort_to_budget(s) {
                return Some(b);
            }
        }
    }
    if let Some(s) = body
        .get("extra_body")
        .and_then(|x| x.get("reasoning_effort"))
        .and_then(|x| x.as_str())
    {
        if let Some(b) = effort_to_budget(s) {
            return Some(b);
        }
    }
    yaml_default_effort.and_then(effort_to_budget)
}

// ============================================================
// thoughtSignature 编码 (gemini 多轮回灌)
// ============================================================

/// Gemini `parts[].thoughtSignature` 在 Anthropic thinking content_block 的 `signature` 字段
/// 编码格式: `base64url(JSON{v: 1, p: "gemini", ts: "<thoughtSignature原文>"})`.
///
/// `p` 字段是 provider 区分标识 (与 openai signature 的 `{v, id, ec}` 物理隔离): 避免订阅切换
/// 时 cc-router 把 openai signature 喂回 Gemini (或反之) 导致上游 400 校验失败。
const GEMINI_SIG_VERSION: u64 = 1;
const GEMINI_SIG_PROVIDER: &str = "gemini";

pub fn encode_gemini_thought_signature(thought_signature: &str) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    let payload = json!({
        "v": GEMINI_SIG_VERSION,
        "p": GEMINI_SIG_PROVIDER,
        "ts": thought_signature,
    });
    let json_str = serde_json::to_string(&payload).unwrap_or_default();
    URL_SAFE_NO_PAD.encode(json_str.as_bytes())
}

pub fn decode_gemini_thought_signature(signature: &str) -> Option<String> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    if signature.is_empty() {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(signature.as_bytes()).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    if v.get("v").and_then(|x| x.as_u64()) != Some(GEMINI_SIG_VERSION) {
        return None;
    }
    if v.get("p").and_then(|x| x.as_str()) != Some(GEMINI_SIG_PROVIDER) {
        // openai signature 错喂到 gemini, 静默 drop 避免污染上游
        return None;
    }
    v.get("ts").and_then(|x| x.as_str()).map(str::to_string)
}

// ============================================================
// 请求转换
// ============================================================

/// 把 Anthropic Messages 请求体翻译成 Gemini generateContent 请求体.
///
/// 注意: `body["model"]` 不写到输出 JSON 里 — Gemini 的 model 嵌在 URL 路径中,
/// 由 dispatch 层做 `{model}` 占位符替换. 但本函数仍要求 body 含 model 字段做校验.
pub fn anthropic_to_gemini(body: &Value, extras: &GeminiExtras) -> AppResult<Value> {
    body.get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::BadRequest("请求 body 缺少 model".into()))?;

    let mut out = json!({});

    // system → systemInstruction
    if let Some(sys) = body.get("system") {
        let text = anthropic_system_to_text(sys);
        if !text.is_empty() {
            out["systemInstruction"] = json!({
                "parts": [{"text": text}],
            });
        }
    }

    // messages[] → contents[]
    if let Some(msgs) = body.get("messages").and_then(|m| m.as_array()) {
        let contents = anthropic_messages_to_contents(msgs, extras)?;
        out["contents"] = json!(contents);
    }

    // generationConfig
    let mut gen_config = serde_json::Map::new();
    if let Some(v) = body.get("temperature") {
        gen_config.insert("temperature".into(), v.clone());
    }
    if let Some(v) = body.get("top_p") {
        gen_config.insert("topP".into(), v.clone());
    }
    if let Some(v) = body.get("max_tokens") {
        gen_config.insert("maxOutputTokens".into(), v.clone());
    }
    if let Some(v) = body.get("stop_sequences") {
        gen_config.insert("stopSequences".into(), v.clone());
    }
    // thinkingConfig: 仅 Gemini 2.5+ 支持; 2.0/1.5 静默忽略, 不门控。
    if extras.thinking_budget.is_some() || extras.include_thoughts {
        let mut tc = serde_json::Map::new();
        if let Some(b) = extras.thinking_budget {
            tc.insert("thinkingBudget".into(), json!(b));
        }
        if extras.include_thoughts {
            tc.insert("includeThoughts".into(), json!(true));
        }
        gen_config.insert("thinkingConfig".into(), Value::Object(tc));
    }
    if !gen_config.is_empty() {
        out["generationConfig"] = Value::Object(gen_config);
    }

    // tools[]
    if let Some(tools) = body.get("tools").and_then(|t| t.as_array()) {
        let declarations: Vec<Value> = tools.iter().filter_map(convert_tool).collect();
        if !declarations.is_empty() {
            out["tools"] = json!([
                {"functionDeclarations": declarations}
            ]);
        }
    }

    // tool_choice → toolConfig.functionCallingConfig
    if let Some(tc) = body.get("tool_choice") {
        if let Some(cfg) = map_tool_choice(tc) {
            out["toolConfig"] = json!({"functionCallingConfig": cfg});
        }
    }

    Ok(out)
}

fn anthropic_system_to_text(system: &Value) -> String {
    if let Some(s) = system.as_str() {
        return s.to_string();
    }
    if let Some(arr) = system.as_array() {
        return arr
            .iter()
            .filter_map(|item| item.get("text").and_then(|t| t.as_str()).map(str::to_string))
            .collect::<Vec<_>>()
            .join("\n\n");
    }
    String::new()
}

/// Anthropic messages → Gemini contents.
/// 关键映射:
/// - role: assistant → model, user 不变
/// - content 可能是 str (整段当 text part) 或 blocks 数组
/// - tool_use 提为 parts[].functionCall; tool_result 提为 parts[].functionResponse
/// - thinking 块: `extras.include_thoughts=true` 时解码 signature 生成 `parts[].thoughtSignature`
///   独立 part (无 text), 通常出现在 model role assistant 回复里。
fn anthropic_messages_to_contents(msgs: &[Value], extras: &GeminiExtras) -> AppResult<Vec<Value>> {
    let mut out: Vec<Value> = Vec::new();
    for m in msgs {
        let role_anth = m.get("role").and_then(|v| v.as_str()).unwrap_or("user");
        let role_gemini = if role_anth == "assistant" { "model" } else { "user" };
        let content = m.get("content");
        let parts = match content {
            Some(Value::String(text)) => vec![json!({"text": text})],
            Some(Value::Array(blocks)) => convert_content_blocks(blocks, extras),
            _ => Vec::new(),
        };
        if parts.is_empty() {
            continue;
        }
        out.push(json!({
            "role": role_gemini,
            "parts": parts,
        }));
    }
    Ok(out)
}

fn convert_content_blocks(blocks: &[Value], extras: &GeminiExtras) -> Vec<Value> {
    let mut parts: Vec<Value> = Vec::new();
    for blk in blocks {
        let blk_type = blk.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match blk_type {
            "text" => {
                if let Some(t) = blk.get("text").and_then(|v| v.as_str()) {
                    parts.push(json!({"text": t}));
                }
            }
            "image" => {
                // Anthropic image: { source: { type: "base64", media_type, data } }
                if let Some(src) = blk.get("source") {
                    let media_type = src.get("media_type").and_then(|v| v.as_str()).unwrap_or("image/png");
                    if let Some(data) = src.get("data").and_then(|v| v.as_str()) {
                        parts.push(json!({
                            "inlineData": {
                                "mimeType": media_type,
                                "data": data,
                            }
                        }));
                    }
                    // url 类型 Phase 1 跳过 (Gemini 需要 Files API)
                }
            }
            "tool_use" => {
                let name = blk.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let args = blk.get("input").cloned().unwrap_or(json!({}));
                parts.push(json!({
                    "functionCall": {
                        "name": name,
                        "args": args,
                    }
                }));
            }
            "tool_result" => {
                // tool_use_id 在 Gemini 没有对应字段; 用 tool_use.name 匹配回灌 functionResponse.name.
                // 我们尽量找当前 tool_result 对应的 name: tool_result block 本身没 name 字段,
                // 但用户在 Anthropic 协议里通常前一条 assistant message 里同 id 的 tool_use 知道 name.
                // Phase 1 简化: 优先用 blk.name (有些客户端会带), 否则用 tool_use_id 当 name 兜底.
                let name = blk
                    .get("name")
                    .and_then(|v| v.as_str())
                    .or_else(|| blk.get("tool_use_id").and_then(|v| v.as_str()))
                    .unwrap_or("");
                let response_value = match blk.get("content") {
                    Some(Value::String(s)) => json!({"result": s}),
                    Some(Value::Array(arr)) => {
                        let text: String = arr
                            .iter()
                            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                            .collect::<Vec<_>>()
                            .join("\n");
                        json!({"result": text})
                    }
                    Some(v) => json!({"result": v}),
                    None => json!({"result": ""}),
                };
                parts.push(json!({
                    "functionResponse": {
                        "name": name,
                        "response": response_value,
                    }
                }));
            }
            "thinking" if extras.include_thoughts => {
                // 多轮回灌: 解码 signature 还原 thoughtSignature, 生成独立 part。
                // Gemini 上游用此字段恢复 encrypted reasoning context 上下文。
                // thinking.text 内容本身被丢弃 — Gemini input parts 不接受 thought summary 文本。
                if let Some(sig) = blk.get("signature").and_then(|v| v.as_str()) {
                    if let Some(thought_signature) = decode_gemini_thought_signature(sig) {
                        if !thought_signature.is_empty() {
                            parts.push(json!({"thoughtSignature": thought_signature}));
                        }
                    }
                }
            }
            // thinking (include_thoughts=false) / redacted_thinking / document 等跳过
            _ => {}
        }
    }
    parts
}

fn convert_tool(t: &Value) -> Option<Value> {
    let name = t.get("name").and_then(|v| v.as_str())?;
    let description = t.get("description").and_then(|v| v.as_str()).unwrap_or("");
    let raw = t
        .get("input_schema")
        .cloned()
        .unwrap_or(json!({"type": "object"}));
    let parameters = sanitize_schema_for_gemini(raw);
    Some(json!({
        "name": name,
        "description": description,
        "parameters": parameters,
    }))
}

/// 把 Claude Code 默认输出的 JSON Schema draft-2020-12 子集裁剪成 Gemini API (OpenAPI 3.0 子集)
/// 接受的子集. Gemini 后端遇到不识别字段会直接 400 拒整个请求, 必须递归清理。
///
/// 处理策略 (递归下钻 `properties.*` / `items` / `anyOf[]`):
/// - **删除**: `$schema`, `$id`, `$ref`, `$defs`, `definitions`, `additionalProperties`,
///   `propertyNames`, `unevaluatedProperties`, `unevaluatedItems`, `dependentSchemas`,
///   `dependentRequired`, `if`, `then`, `else`, `not`, `allOf`, `prefixItems`, `contains`,
///   `patternProperties`, `readOnly`, `writeOnly`
/// - **转换**:
///   - `exclusiveMinimum: N` → 整数时 `minimum: N + 1`, 浮点时 `minimum: N` (轻微语义偏差,
///     模型一般不卡边界值, 比 400 拒整请求好)
///   - `exclusiveMaximum: N` → 同理
///   - `const: X` → `enum: [X]` (Gemini 不识 const)
///   - `oneOf` → `anyOf` (Gemini 不识 oneOf 严格语义, anyOf 行为接近)
///
/// 不变: `type`, `properties`, `required`, `items`, `description`, `enum`, `format`, `minimum`,
/// `maximum`, `minLength`, `maxLength`, `pattern`, `minItems`, `maxItems`, `anyOf`, `nullable` 等。
pub fn sanitize_schema_for_gemini(schema: Value) -> Value {
    match schema {
        Value::Object(mut obj) => {
            const DROP_KEYS: &[&str] = &[
                "$schema",
                "$id",
                "$ref",
                "$defs",
                "definitions",
                "additionalProperties",
                "propertyNames",
                "unevaluatedProperties",
                "unevaluatedItems",
                "dependentSchemas",
                "dependentRequired",
                "if",
                "then",
                "else",
                "not",
                "allOf",
                "prefixItems",
                "contains",
                "patternProperties",
                "readOnly",
                "writeOnly",
            ];
            for k in DROP_KEYS {
                obj.remove(*k);
            }

            if let Some(excl) = obj.remove("exclusiveMinimum") {
                if let Some(n) = excl.as_i64() {
                    obj.entry("minimum")
                        .or_insert(Value::from(n.saturating_add(1)));
                } else if let Some(f) = excl.as_f64() {
                    obj.entry("minimum").or_insert(json!(f));
                }
            }
            if let Some(excl) = obj.remove("exclusiveMaximum") {
                if let Some(n) = excl.as_i64() {
                    obj.entry("maximum")
                        .or_insert(Value::from(n.saturating_sub(1)));
                } else if let Some(f) = excl.as_f64() {
                    obj.entry("maximum").or_insert(json!(f));
                }
            }

            if let Some(c) = obj.remove("const") {
                obj.entry("enum").or_insert(Value::Array(vec![c]));
            }
            if let Some(one_of) = obj.remove("oneOf") {
                obj.entry("anyOf").or_insert(one_of);
            }

            // 递归下钻常见嵌套点
            if let Some(props) = obj.get_mut("properties") {
                if let Some(props_obj) = props.as_object_mut() {
                    for (_, v) in props_obj.iter_mut() {
                        let taken = std::mem::take(v);
                        *v = sanitize_schema_for_gemini(taken);
                    }
                }
            }
            if let Some(items) = obj.get_mut("items") {
                let taken = std::mem::take(items);
                *items = sanitize_schema_for_gemini(taken);
            }
            if let Some(any_of) = obj.get_mut("anyOf") {
                if let Some(arr) = any_of.as_array_mut() {
                    for v in arr.iter_mut() {
                        let taken = std::mem::take(v);
                        *v = sanitize_schema_for_gemini(taken);
                    }
                }
            }

            Value::Object(obj)
        }
        other => other,
    }
}

fn map_tool_choice(tc: &Value) -> Option<Value> {
    if let Some(s) = tc.as_str() {
        return match s {
            "auto" => Some(json!({"mode": "AUTO"})),
            "any" => Some(json!({"mode": "ANY"})),
            "none" => Some(json!({"mode": "NONE"})),
            _ => None,
        };
    }
    if let Some(obj) = tc.as_object() {
        let kind = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match kind {
            "auto" => return Some(json!({"mode": "AUTO"})),
            "any" => return Some(json!({"mode": "ANY"})),
            "none" => return Some(json!({"mode": "NONE"})),
            "tool" => {
                if let Some(name) = obj.get("name").and_then(|v| v.as_str()) {
                    return Some(json!({
                        "mode": "ANY",
                        "allowedFunctionNames": [name],
                    }));
                }
            }
            _ => {}
        }
    }
    None
}

// ============================================================
// 响应转换 (SSE 状态机)
// ============================================================

/// Anthropic SSE 事件最小输出表示. 调用方序列化成 `event: <name>\ndata: <json>\n\n`.
#[derive(Debug, Clone)]
pub struct AnthropicEvent {
    pub event: &'static str,
    pub data: Value,
}

impl AnthropicEvent {
    pub fn to_sse_bytes(&self) -> Vec<u8> {
        let payload = serde_json::to_string(&self.data).unwrap_or_else(|_| "{}".into());
        format!("event: {}\ndata: {}\n\n", self.event, payload).into_bytes()
    }
}

/// Gemini SSE 状态机. 流式调用方按 `\n\n` 切帧 → `parse_gemini_sse_frame` 解 JSON →
/// `GeminiSseConverter::feed(json)` 拿到 Anthropic 事件序列.
///
/// 当前块状态:
/// - `text_block_index = Some(i)` 表示有一个开着的 text content_block, 索引 i;
/// - tool_use 块是 atomic emit (start+delta+stop 一次性发完), 不存活到下一帧.
pub struct GeminiSseConverter {
    started: bool,
    stopped: bool,
    message_id: String,
    response_model: String,
    /// 下一个分配的 Anthropic content_block index
    next_index: u32,
    /// 当前开着的 text block index, None 表示无活跃 text 块
    text_block_index: Option<u32>,
    /// 当前开着的 thinking block index, None 表示无活跃 thinking 块
    thinking_block_index: Option<u32>,
    /// 当前 thinking 块累积到的最新 thoughtSignature (finalize 时通过 signature_delta 写入)
    pending_thought_signature: Option<String>,
    /// 是否暴露 thought parts. yaml `expose_reasoning` → dispatch 层传入。
    emit_thoughts: bool,
    /// 流末记录的 stop_reason (映射自 Gemini finishReason)
    stop_reason: String,
    /// 累计 usage. Gemini 每帧都可能携带 usageMetadata, 取末值.
    final_usage: Value,
    /// 是否已观测到 finishReason (避免重复关块)
    saw_finish: bool,
}

impl GeminiSseConverter {
    /// 默认 `emit_thoughts=false` (维持 v2.1 行为, thought parts 静默丢弃)。
    /// 暴露 thought 走 [`Self::new_with_extras`]。
    pub fn new(response_model: &str) -> Self {
        Self::new_with_extras(response_model, false)
    }

    pub fn new_with_extras(response_model: &str, emit_thoughts: bool) -> Self {
        Self {
            started: false,
            stopped: false,
            message_id: format!("msg_{}", Uuid::new_v4().simple()),
            response_model: response_model.to_string(),
            next_index: 0,
            text_block_index: None,
            thinking_block_index: None,
            pending_thought_signature: None,
            emit_thoughts,
            stop_reason: "end_turn".to_string(),
            final_usage: json!({"input_tokens": 0, "output_tokens": 0}),
            saw_finish: false,
        }
    }

    /// 喂一帧 Gemini SSE JSON, 返回 0..多个待发给客户端的 Anthropic 事件.
    pub fn feed(&mut self, frame: &Value) -> Vec<AnthropicEvent> {
        let mut out = Vec::new();

        if !self.started {
            out.push(self.emit_message_start());
            self.started = true;
        }

        // 处理 candidates[0].content.parts
        if let Some(candidate) = frame.get("candidates").and_then(|c| c.as_array()).and_then(|a| a.first()) {
            if let Some(parts) = candidate
                .get("content")
                .and_then(|c| c.get("parts"))
                .and_then(|p| p.as_array())
            {
                for part in parts {
                    self.handle_part(part, &mut out);
                }
            }

            // finishReason: STOP / MAX_TOKENS / SAFETY / FINISH_REASON_UNSPECIFIED / ...
            if let Some(reason) = candidate.get("finishReason").and_then(|v| v.as_str()) {
                if !reason.is_empty() && reason != "FINISH_REASON_UNSPECIFIED" {
                    self.stop_reason = map_finish_reason(reason).to_string();
                    self.saw_finish = true;
                    // 关掉残留块 (text 或 thinking)
                    self.close_thinking_block_if_open(&mut out);
                    self.close_text_block_if_open(&mut out);
                }
            }
        }

        // usageMetadata (每帧都可能携带, 取末值)
        if let Some(usage) = frame.get("usageMetadata") {
            self.absorb_usage(usage);
        }

        out
    }

    /// 流自然结束 (或异常中断) 时调用. 兜底 emit message_delta + message_stop.
    pub fn finalize(&mut self) -> Vec<AnthropicEvent> {
        let mut out = Vec::new();
        if !self.started || self.stopped {
            return out;
        }
        // 兜底关掉残留块 (thinking 优先 — 它通常先于 text 出现且不会被 text 关掉)
        self.close_thinking_block_if_open(&mut out);
        self.close_text_block_if_open(&mut out);
        out.push(AnthropicEvent {
            event: "message_delta",
            data: json!({
                "type": "message_delta",
                "delta": {"stop_reason": self.stop_reason, "stop_sequence": null},
                "usage": self.final_usage,
            }),
        });
        out.push(AnthropicEvent {
            event: "message_stop",
            data: json!({"type": "message_stop"}),
        });
        self.stopped = true;
        out
    }

    /// 响应模型名 (cc-router 从订阅 slot 传入). 给日志用.
    pub fn response_model(&self) -> &str {
        &self.response_model
    }

    /// 累计 usage 视图 (给 finalize 后日志用).
    pub fn usage(&self) -> &Value {
        &self.final_usage
    }

    // ---------- internals ----------

    fn emit_message_start(&self) -> AnthropicEvent {
        AnthropicEvent {
            event: "message_start",
            data: json!({
                "type": "message_start",
                "message": {
                    "id": self.message_id,
                    "type": "message",
                    "role": "assistant",
                    "model": self.response_model,
                    "content": [],
                    "stop_reason": null,
                    "stop_sequence": null,
                    "usage": {"input_tokens": 0, "output_tokens": 0},
                },
            }),
        }
    }

    fn handle_part(&mut self, part: &Value, out: &mut Vec<AnthropicEvent>) {
        let is_thought = part.get("thought").and_then(|v| v.as_bool()).unwrap_or(false);
        let thought_signature_present = part
            .get("thoughtSignature")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        // thoughtSignature 单独出现 (无 text 无 functionCall) — Gemini 用这种形态携带 reasoning context.
        // 累积到 pending, 关 thinking 块时 emit signature_delta。
        if self.emit_thoughts {
            if let Some(sig) = &thought_signature_present {
                self.pending_thought_signature = Some(sig.clone());
            }
        }

        if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
            if text.is_empty() {
                return;
            }
            if is_thought && self.emit_thoughts {
                // thought summary text → Anthropic thinking_delta
                self.close_text_block_if_open(out);
                let idx = self.ensure_thinking_block(out);
                out.push(AnthropicEvent {
                    event: "content_block_delta",
                    data: json!({
                        "type": "content_block_delta",
                        "index": idx,
                        "delta": {"type": "thinking_delta", "thinking": text},
                    }),
                });
                return;
            }
            // 普通 text — 关掉 thinking 块, 再开/续 text 块
            self.close_thinking_block_if_open(out);
            let idx = self.ensure_text_block(out);
            out.push(AnthropicEvent {
                event: "content_block_delta",
                data: json!({
                    "type": "content_block_delta",
                    "index": idx,
                    "delta": {"type": "text_delta", "text": text},
                }),
            });
            return;
        }
        if let Some(fc) = part.get("functionCall") {
            // functionCall 出现前关掉 thinking + text
            self.close_thinking_block_if_open(out);
            self.close_text_block_if_open(out);
            let name = fc.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = fc.get("args").cloned().unwrap_or(json!({}));
            let idx = self.next_index;
            self.next_index += 1;
            let tool_id = format!("toolu_{}", Uuid::new_v4().simple());
            out.push(AnthropicEvent {
                event: "content_block_start",
                data: json!({
                    "type": "content_block_start",
                    "index": idx,
                    "content_block": {
                        "type": "tool_use",
                        "id": tool_id,
                        "name": name,
                        "input": {},
                    },
                }),
            });
            // Gemini args 是完整 JSON 对象, 不流式. 一次性 dump 成 input_json_delta.
            let args_str = serde_json::to_string(&args).unwrap_or_else(|_| "{}".into());
            out.push(AnthropicEvent {
                event: "content_block_delta",
                data: json!({
                    "type": "content_block_delta",
                    "index": idx,
                    "delta": {"type": "input_json_delta", "partial_json": args_str},
                }),
            });
            out.push(AnthropicEvent {
                event: "content_block_stop",
                data: json!({"type": "content_block_stop", "index": idx}),
            });
        }
        // inlineData / fileData 等跳过
    }

    fn ensure_thinking_block(&mut self, out: &mut Vec<AnthropicEvent>) -> u32 {
        if let Some(idx) = self.thinking_block_index {
            return idx;
        }
        let idx = self.next_index;
        self.next_index += 1;
        self.thinking_block_index = Some(idx);
        out.push(AnthropicEvent {
            event: "content_block_start",
            data: json!({
                "type": "content_block_start",
                "index": idx,
                "content_block": {"type": "thinking", "thinking": ""},
            }),
        });
        idx
    }

    fn close_thinking_block_if_open(&mut self, out: &mut Vec<AnthropicEvent>) {
        if let Some(idx) = self.thinking_block_index.take() {
            // 先 emit pending signature_delta (若有), 再关块
            if let Some(thought_signature) = self.pending_thought_signature.take() {
                let signature = encode_gemini_thought_signature(&thought_signature);
                out.push(AnthropicEvent {
                    event: "content_block_delta",
                    data: json!({
                        "type": "content_block_delta",
                        "index": idx,
                        "delta": {"type": "signature_delta", "signature": signature},
                    }),
                });
            }
            out.push(AnthropicEvent {
                event: "content_block_stop",
                data: json!({"type": "content_block_stop", "index": idx}),
            });
        }
    }

    fn ensure_text_block(&mut self, out: &mut Vec<AnthropicEvent>) -> u32 {
        if let Some(idx) = self.text_block_index {
            return idx;
        }
        let idx = self.next_index;
        self.next_index += 1;
        self.text_block_index = Some(idx);
        out.push(AnthropicEvent {
            event: "content_block_start",
            data: json!({
                "type": "content_block_start",
                "index": idx,
                "content_block": {"type": "text", "text": ""},
            }),
        });
        idx
    }

    fn close_text_block_if_open(&mut self, out: &mut Vec<AnthropicEvent>) {
        if let Some(idx) = self.text_block_index.take() {
            out.push(AnthropicEvent {
                event: "content_block_stop",
                data: json!({"type": "content_block_stop", "index": idx}),
            });
        }
    }

    fn absorb_usage(&mut self, usage: &Value) {
        let prompt = usage
            .get("promptTokenCount")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let candidates = usage
            .get("candidatesTokenCount")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let cached = usage
            .get("cachedContentTokenCount")
            .and_then(|v| v.as_u64());
        let mut usage_obj = json!({
            "input_tokens": prompt,
            "output_tokens": candidates,
        });
        if let Some(c) = cached {
            usage_obj["cache_read_input_tokens"] = json!(c);
        }
        self.final_usage = usage_obj;
    }
}

fn map_finish_reason(reason: &str) -> &'static str {
    match reason {
        "STOP" => "end_turn",
        "MAX_TOKENS" => "max_tokens",
        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" => "stop_sequence",
        // tool_use 没显式 finishReason; Gemini 输出 functionCall 后通常 STOP
        _ => "end_turn",
    }
}

// ============================================================
// 非流式: 把 SSE 帧吃完, 还原成 Anthropic Messages 最终 JSON
// ============================================================

/// 给客户端非流式请求用. cc-router 始终发起流式上游 (Gemini streamGenerateContent),
/// 这里累积 SSE 帧到完整 Anthropic 响应 JSON.
pub struct NonStreamingCollector {
    converter: GeminiSseConverter,
    /// content_block index → 累积的 text
    text_acc: HashMap<u32, String>,
    /// content_block index → 累积的 thinking
    thinking_acc: HashMap<u32, String>,
    /// content_block index → 累积的 signature (signature_delta)
    signature_acc: HashMap<u32, String>,
    /// content_block index → 块元信息 (block_start 时记录)
    block_meta: HashMap<u32, Value>,
    /// content_block index 出现顺序
    order: Vec<u32>,
}

impl NonStreamingCollector {
    pub fn new(response_model: &str) -> Self {
        Self::new_with_extras(response_model, false)
    }

    pub fn new_with_extras(response_model: &str, emit_thoughts: bool) -> Self {
        Self {
            converter: GeminiSseConverter::new_with_extras(response_model, emit_thoughts),
            text_acc: HashMap::new(),
            thinking_acc: HashMap::new(),
            signature_acc: HashMap::new(),
            block_meta: HashMap::new(),
            order: Vec::new(),
        }
    }

    pub fn feed(&mut self, frame: &Value) {
        let events = self.converter.feed(frame);
        for evt in events {
            self.absorb(&evt);
        }
    }

    pub fn finalize(mut self) -> Value {
        let tail = self.converter.finalize();
        for evt in tail {
            self.absorb(&evt);
        }

        let content: Vec<Value> = self
            .order
            .iter()
            .filter_map(|idx| {
                let meta = self.block_meta.get(idx)?.clone();
                let mtype = meta.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match mtype {
                    "text" => {
                        let text = self.text_acc.get(idx).cloned().unwrap_or_default();
                        Some(json!({"type": "text", "text": text}))
                    }
                    "thinking" => {
                        let thinking = self.thinking_acc.get(idx).cloned().unwrap_or_default();
                        let signature = self.signature_acc.get(idx).cloned().unwrap_or_default();
                        Some(json!({
                            "type": "thinking",
                            "thinking": thinking,
                            "signature": signature,
                        }))
                    }
                    "tool_use" => Some(json!({
                        "type": "tool_use",
                        "id": meta.get("id").cloned().unwrap_or(Value::Null),
                        "name": meta.get("name").cloned().unwrap_or(Value::Null),
                        "input": meta.get("_args_json").cloned().unwrap_or(json!({})),
                    })),
                    _ => None,
                }
            })
            .collect();

        json!({
            "id": self.converter.message_id,
            "type": "message",
            "role": "assistant",
            "model": self.converter.response_model,
            "content": content,
            "stop_reason": self.converter.stop_reason,
            "stop_sequence": null,
            "usage": self.converter.final_usage,
        })
    }

    fn absorb(&mut self, evt: &AnthropicEvent) {
        match evt.event {
            "content_block_start" => {
                let index = evt.data.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                if !self.order.contains(&index) {
                    self.order.push(index);
                }
                let block = evt.data.get("content_block").cloned().unwrap_or(json!({}));
                self.block_meta.insert(index, block);
            }
            "content_block_delta" => {
                let index = evt.data.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                let delta = match evt.data.get("delta") {
                    Some(d) => d,
                    None => return,
                };
                let dtype = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match dtype {
                    "text_delta" => {
                        if let Some(t) = delta.get("text").and_then(|v| v.as_str()) {
                            self.text_acc.entry(index).or_default().push_str(t);
                        }
                    }
                    "thinking_delta" => {
                        if let Some(t) = delta.get("thinking").and_then(|v| v.as_str()) {
                            self.thinking_acc.entry(index).or_default().push_str(t);
                        }
                    }
                    "signature_delta" => {
                        if let Some(s) = delta.get("signature").and_then(|v| v.as_str()) {
                            self.signature_acc.insert(index, s.to_string());
                        }
                    }
                    "input_json_delta" => {
                        // tool_use args 是 atomic 一次 dump, 直接 parse 存到 block_meta._args_json
                        if let Some(s) = delta.get("partial_json").and_then(|v| v.as_str()) {
                            let parsed: Value = serde_json::from_str(s).unwrap_or(json!({}));
                            if let Some(meta) = self.block_meta.get_mut(&index) {
                                meta["_args_json"] = parsed;
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

// ============================================================
// SSE 帧解析
// ============================================================

/// 解 Gemini SSE 帧. Gemini 不发 `event: foo`, 只有 `data: {json}` (和可能的 `:` 心跳行).
/// 返回 None 表示该帧没有可解析的 JSON (心跳或空帧).
pub fn parse_gemini_sse_frame(raw: &str) -> Option<Value> {
    let mut data = String::new();
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("data: ") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest);
        } else if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest);
        }
    }
    if data.is_empty() || data == "[DONE]" {
        return None;
    }
    serde_json::from_str(&data).ok()
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_basic_text() {
        let body = json!({
            "model": "gemini-2.5-flash",
            "messages": [{"role": "user", "content": "hello"}],
        });
        let out = anthropic_to_gemini(&body, &GeminiExtras::default()).unwrap();
        assert!(out.get("model").is_none(), "model 不应进 body");
        let contents = out["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "hello");
    }

    #[test]
    fn request_role_swap_assistant_to_model() {
        let body = json!({
            "model": "gemini-2.5-flash",
            "messages": [
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": "hi back"},
                {"role": "user", "content": "more"},
            ],
        });
        let out = anthropic_to_gemini(&body, &GeminiExtras::default()).unwrap();
        let contents = out["contents"].as_array().unwrap();
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[2]["role"], "user");
    }

    #[test]
    fn request_system_string_to_instruction() {
        let body = json!({
            "model": "gemini-2.5-flash",
            "system": "你是助手",
            "messages": [{"role": "user", "content": "hi"}],
        });
        let out = anthropic_to_gemini(&body, &GeminiExtras::default()).unwrap();
        assert_eq!(out["systemInstruction"]["parts"][0]["text"], "你是助手");
    }

    #[test]
    fn request_system_array_to_instruction() {
        let body = json!({
            "model": "gemini-2.5-flash",
            "system": [
                {"type": "text", "text": "段 A"},
                {"type": "text", "text": "段 B"},
            ],
            "messages": [{"role": "user", "content": "hi"}],
        });
        let out = anthropic_to_gemini(&body, &GeminiExtras::default()).unwrap();
        assert_eq!(out["systemInstruction"]["parts"][0]["text"], "段 A\n\n段 B");
    }

    #[test]
    fn request_drops_no_system_field() {
        let body = json!({
            "model": "gemini-2.5-flash",
            "messages": [{"role": "user", "content": "hi"}],
        });
        let out = anthropic_to_gemini(&body, &GeminiExtras::default()).unwrap();
        assert!(out.get("systemInstruction").is_none());
    }

    #[test]
    fn request_generation_config() {
        let body = json!({
            "model": "gemini-2.5-flash",
            "temperature": 0.7,
            "top_p": 0.95,
            "max_tokens": 1024,
            "stop_sequences": ["STOP"],
            "messages": [{"role": "user", "content": "hi"}],
        });
        let out = anthropic_to_gemini(&body, &GeminiExtras::default()).unwrap();
        let gc = &out["generationConfig"];
        assert_eq!(gc["temperature"], 0.7);
        assert_eq!(gc["topP"], 0.95);
        assert_eq!(gc["maxOutputTokens"], 1024);
        assert_eq!(gc["stopSequences"], json!(["STOP"]));
    }

    #[test]
    fn request_tools_converted_to_function_declarations() {
        let body = json!({
            "model": "gemini-2.5-flash",
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [{
                "name": "get_weather",
                "description": "查询天气",
                "input_schema": {
                    "type": "object",
                    "properties": {"city": {"type": "string"}},
                    "required": ["city"],
                }
            }],
        });
        let out = anthropic_to_gemini(&body, &GeminiExtras::default()).unwrap();
        let decls = out["tools"][0]["functionDeclarations"].as_array().unwrap();
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0]["name"], "get_weather");
        assert_eq!(decls[0]["description"], "查询天气");
        assert_eq!(decls[0]["parameters"]["type"], "object");
        assert!(decls[0].get("input_schema").is_none());
    }

    #[test]
    fn request_tool_choice_auto_and_any_and_tool() {
        for (input, expected_mode) in [
            (json!("auto"), "AUTO"),
            (json!("any"), "ANY"),
            (json!("none"), "NONE"),
        ] {
            let body = json!({
                "model": "gemini-2.5-flash",
                "messages": [{"role": "user", "content": "hi"}],
                "tool_choice": input,
            });
            let out = anthropic_to_gemini(&body, &GeminiExtras::default()).unwrap();
            assert_eq!(out["toolConfig"]["functionCallingConfig"]["mode"], expected_mode);
        }

        let body = json!({
            "model": "gemini-2.5-flash",
            "messages": [{"role": "user", "content": "hi"}],
            "tool_choice": {"type": "tool", "name": "get_weather"},
        });
        let out = anthropic_to_gemini(&body, &GeminiExtras::default()).unwrap();
        let cfg = &out["toolConfig"]["functionCallingConfig"];
        assert_eq!(cfg["mode"], "ANY");
        assert_eq!(cfg["allowedFunctionNames"], json!(["get_weather"]));
    }

    #[test]
    fn request_tool_use_and_tool_result_round_trip() {
        let body = json!({
            "model": "gemini-2.5-flash",
            "messages": [
                {"role": "user", "content": "今天北京天气?"},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "我去查"},
                    {"type": "tool_use", "id": "toolu_1", "name": "get_weather",
                     "input": {"city": "Beijing"}},
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_1", "name": "get_weather",
                     "content": "晴, 20°C"},
                ]},
            ],
        });
        let out = anthropic_to_gemini(&body, &GeminiExtras::default()).unwrap();
        let contents = out["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 3);
        // 第二条 assistant→model 含 text + functionCall
        let parts2 = contents[1]["parts"].as_array().unwrap();
        assert_eq!(parts2.len(), 2);
        assert_eq!(parts2[0]["text"], "我去查");
        assert_eq!(parts2[1]["functionCall"]["name"], "get_weather");
        assert_eq!(parts2[1]["functionCall"]["args"], json!({"city": "Beijing"}));
        // 第三条 user 含 functionResponse
        let parts3 = contents[2]["parts"].as_array().unwrap();
        assert_eq!(parts3.len(), 1);
        assert_eq!(parts3[0]["functionResponse"]["name"], "get_weather");
        assert_eq!(parts3[0]["functionResponse"]["response"]["result"], "晴, 20°C");
    }

    #[test]
    fn request_image_inline_data() {
        let body = json!({
            "model": "gemini-2.5-flash",
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "看图"},
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "BASE64XYZ"}},
            ]}],
        });
        let out = anthropic_to_gemini(&body, &GeminiExtras::default()).unwrap();
        let parts = out["contents"][0]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["text"], "看图");
        assert_eq!(parts[1]["inlineData"]["mimeType"], "image/png");
        assert_eq!(parts[1]["inlineData"]["data"], "BASE64XYZ");
    }

    #[test]
    fn sse_text_basic_flow() {
        let mut conv = GeminiSseConverter::new("gemini-2.5-flash");
        // 首帧带文本
        let events = conv.feed(&json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "Hello "}]},
            }],
        }));
        assert_eq!(events.len(), 3); // message_start + content_block_start + content_block_delta
        assert_eq!(events[0].event, "message_start");
        assert_eq!(events[1].event, "content_block_start");
        assert_eq!(events[1].data["content_block"]["type"], "text");
        assert_eq!(events[2].event, "content_block_delta");
        assert_eq!(events[2].data["delta"]["text"], "Hello ");

        // 第二帧再来一段文本 (同一个 text block)
        let events = conv.feed(&json!({
            "candidates": [{"content": {"parts": [{"text": "world"}]}}],
        }));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "content_block_delta");
        assert_eq!(events[0].data["delta"]["text"], "world");

        // 末帧带 finishReason + usage
        let events = conv.feed(&json!({
            "candidates": [{"content": {"parts": []}, "finishReason": "STOP"}],
            "usageMetadata": {"promptTokenCount": 5, "candidatesTokenCount": 2},
        }));
        // 应 emit content_block_stop (关掉 text block)
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "content_block_stop");

        let tail = conv.finalize();
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0].event, "message_delta");
        assert_eq!(tail[0].data["delta"]["stop_reason"], "end_turn");
        assert_eq!(tail[0].data["usage"]["input_tokens"], 5);
        assert_eq!(tail[0].data["usage"]["output_tokens"], 2);
        assert_eq!(tail[1].event, "message_stop");
    }

    #[test]
    fn sse_tool_call_atomic() {
        let mut conv = GeminiSseConverter::new("gemini-2.5-flash");
        // 单帧含 functionCall
        let events = conv.feed(&json!({
            "candidates": [{
                "content": {"role": "model", "parts": [{
                    "functionCall": {"name": "get_weather", "args": {"city": "Beijing"}}
                }]},
                "finishReason": "STOP",
            }],
            "usageMetadata": {"promptTokenCount": 8, "candidatesTokenCount": 4},
        }));
        // message_start + content_block_start(tool_use) + content_block_delta(input_json_delta) + content_block_stop
        // 注: finishReason 关 text block 时, 由于 tool_use 块 atomic 立即关闭, text_block_index=None, 不再额外发 content_block_stop
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].event, "message_start");
        assert_eq!(events[1].event, "content_block_start");
        assert_eq!(events[1].data["content_block"]["type"], "tool_use");
        assert_eq!(events[1].data["content_block"]["name"], "get_weather");
        assert_eq!(events[2].event, "content_block_delta");
        assert_eq!(events[2].data["delta"]["type"], "input_json_delta");
        assert_eq!(events[3].event, "content_block_stop");

        let tail = conv.finalize();
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0].event, "message_delta");
    }

    #[test]
    fn non_streaming_collector_assembles_text() {
        let mut col = NonStreamingCollector::new("gemini-2.5-flash");
        col.feed(&json!({"candidates":[{"content":{"parts":[{"text":"你好 "}]}}]}));
        col.feed(&json!({"candidates":[{"content":{"parts":[{"text":"世界"}]}}]}));
        col.feed(&json!({
            "candidates":[{"content":{"parts":[]},"finishReason":"STOP"}],
            "usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":4},
        }));
        let msg = col.finalize();
        assert_eq!(msg["type"], "message");
        assert_eq!(msg["role"], "assistant");
        assert_eq!(msg["model"], "gemini-2.5-flash");
        let content = msg["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "你好 世界");
        assert_eq!(msg["usage"]["input_tokens"], 3);
        assert_eq!(msg["usage"]["output_tokens"], 4);
        assert_eq!(msg["stop_reason"], "end_turn");
    }

    #[test]
    fn non_streaming_collector_assembles_tool_use() {
        let mut col = NonStreamingCollector::new("gemini-2.5-flash");
        col.feed(&json!({
            "candidates":[{"content":{"parts":[
                {"functionCall":{"name":"get_weather","args":{"city":"Beijing"}}}
            ]},"finishReason":"STOP"}],
            "usageMetadata":{"promptTokenCount":8,"candidatesTokenCount":4},
        }));
        let msg = col.finalize();
        let content = msg["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "tool_use");
        assert_eq!(content[0]["name"], "get_weather");
        assert_eq!(content[0]["input"], json!({"city": "Beijing"}));
    }

    #[test]
    fn non_streaming_collector_assembles_text_then_tool_use() {
        let mut col = NonStreamingCollector::new("gemini-2.5-flash");
        col.feed(&json!({
            "candidates":[{"content":{"parts":[
                {"text":"我去查"},
                {"functionCall":{"name":"get_weather","args":{"city":"Beijing"}}}
            ]},"finishReason":"STOP"}],
        }));
        let msg = col.finalize();
        let content = msg["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "我去查");
        assert_eq!(content[1]["type"], "tool_use");
        assert_eq!(content[1]["name"], "get_weather");
    }

    #[test]
    fn parse_gemini_sse_frame_basic() {
        let raw = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hi\"}]}}]}";
        let parsed = parse_gemini_sse_frame(raw).unwrap();
        assert_eq!(parsed["candidates"][0]["content"]["parts"][0]["text"], "hi");
    }

    #[test]
    fn parse_gemini_sse_frame_done_returns_none() {
        assert!(parse_gemini_sse_frame("data: [DONE]").is_none());
        assert!(parse_gemini_sse_frame("").is_none());
        assert!(parse_gemini_sse_frame(": ping").is_none());
    }

    #[test]
    fn finish_reason_map() {
        assert_eq!(map_finish_reason("STOP"), "end_turn");
        assert_eq!(map_finish_reason("MAX_TOKENS"), "max_tokens");
        assert_eq!(map_finish_reason("SAFETY"), "stop_sequence");
        assert_eq!(map_finish_reason("UNKNOWN_FUTURE"), "end_turn");
    }

    // ============================================================
    // Thinking 双向翻译 (v2.x 新增)
    // ============================================================

    #[test]
    fn request_omits_thinking_config_when_extras_default() {
        let body = json!({
            "model": "gemini-2.5-flash",
            "messages": [{"role": "user", "content": "hi"}],
        });
        let out = anthropic_to_gemini(&body, &GeminiExtras::default()).unwrap();
        assert!(
            out.get("generationConfig")
                .and_then(|g| g.get("thinkingConfig"))
                .is_none(),
            "默认 extras 不应注入 thinkingConfig"
        );
    }

    #[test]
    fn request_injects_thinking_config_with_budget_and_includes() {
        let body = json!({
            "model": "gemini-2.5-flash",
            "messages": [{"role": "user", "content": "hi"}],
        });
        let extras = GeminiExtras {
            thinking_budget: Some(16384),
            include_thoughts: true,
        };
        let out = anthropic_to_gemini(&body, &extras).unwrap();
        let tc = &out["generationConfig"]["thinkingConfig"];
        assert_eq!(tc["thinkingBudget"], 16384);
        assert_eq!(tc["includeThoughts"], true);
    }

    #[test]
    fn resolve_thinking_budget_priority_chain() {
        // 1. budget_tokens 直接透传
        let body = json!({"thinking": {"budget_tokens": 12345}});
        assert_eq!(resolve_thinking_budget(&body, Some("low")), Some(12345));

        // 2. thinking.effort 映射
        let body = json!({"thinking": {"effort": "high"}});
        assert_eq!(resolve_thinking_budget(&body, Some("low")), Some(65536));

        // 3. extra_body.reasoning_effort 映射
        let body = json!({"extra_body": {"reasoning_effort": "minimal"}});
        assert_eq!(resolve_thinking_budget(&body, Some("low")), Some(512));

        // 4. yaml default
        let body = json!({});
        assert_eq!(resolve_thinking_budget(&body, Some("medium")), Some(16384));

        // 5. 全空 → None
        let body = json!({});
        assert_eq!(resolve_thinking_budget(&body, None), None);

        // 非法 effort → 继续往下找
        let body = json!({"thinking": {"effort": "garbage"}, "extra_body": {"reasoning_effort": "high"}});
        assert_eq!(resolve_thinking_budget(&body, None), Some(65536));
    }

    #[test]
    fn gemini_thought_signature_roundtrip_encoding() {
        let sig = encode_gemini_thought_signature("Gemini_TS_BYTES_42");
        assert!(!sig.is_empty());
        let ts = decode_gemini_thought_signature(&sig).unwrap();
        assert_eq!(ts, "Gemini_TS_BYTES_42");
    }

    #[test]
    fn gemini_signature_rejects_openai_signature() {
        // 用 openai 的 signature 编码喂给 gemini decoder, 应返回 None 避免错喂上游
        let openai_sig =
            crate::proxy::transform::responses_common::encode_reasoning_signature("rs_x", "ENC");
        assert!(
            decode_gemini_thought_signature(&openai_sig).is_none(),
            "openai signature 不能被 gemini decoder 解出, 否则会污染上游"
        );
    }

    #[test]
    fn request_thinking_block_roundtrip_into_thought_signature_part() {
        // 客户端回灌一轮: assistant 消息包含 thinking 块, signature 编码自上一轮的 thoughtSignature
        let encoded = encode_gemini_thought_signature("PREV_THOUGHT_SIG");
        let body = json!({
            "model": "gemini-2.5-flash",
            "messages": [
                {"role": "user", "content": "step 1?"},
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "summary", "signature": encoded},
                    {"type": "text", "text": "answer 1"},
                ]},
                {"role": "user", "content": "next"},
            ],
        });
        let extras = GeminiExtras {
            thinking_budget: None,
            include_thoughts: true,
        };
        let out = anthropic_to_gemini(&body, &extras).unwrap();
        let assistant_parts = out["contents"][1]["parts"].as_array().unwrap();
        // 期望: 第一个 part 是 thoughtSignature, 第二个是 text
        let has_signature_part = assistant_parts.iter().any(|p| {
            p.get("thoughtSignature")
                .and_then(|v| v.as_str())
                .map(|s| s == "PREV_THOUGHT_SIG")
                .unwrap_or(false)
        });
        assert!(has_signature_part, "缺少 thoughtSignature part: {:?}", assistant_parts);
    }

    #[test]
    fn request_thinking_block_dropped_when_extras_disabled() {
        let encoded = encode_gemini_thought_signature("SIG");
        let body = json!({
            "model": "gemini-2.5-flash",
            "messages": [{"role": "assistant", "content": [
                {"type": "thinking", "thinking": "x", "signature": encoded},
                {"type": "text", "text": "y"},
            ]}],
        });
        let out = anthropic_to_gemini(&body, &GeminiExtras::default()).unwrap();
        let parts = out["contents"][0]["parts"].as_array().unwrap();
        assert!(
            parts.iter().all(|p| p.get("thoughtSignature").is_none()),
            "extras.include_thoughts=false 时 thinking 块应被丢弃"
        );
    }

    #[test]
    fn response_thought_part_emits_thinking_block() {
        let mut conv = GeminiSseConverter::new_with_extras("gemini-2.5-flash", true);
        let evts = conv.feed(&json!({
            "candidates": [{"content": {"parts": [
                {"thought": true, "text": "step 1"},
            ]}}],
        }));
        // message_start + content_block_start(thinking) + content_block_delta(thinking_delta)
        assert!(evts.iter().any(|e| e.event == "message_start"));
        let has_start = evts.iter().any(|e| {
            e.event == "content_block_start"
                && e.data.get("content_block").and_then(|cb| cb.get("type"))
                    == Some(&json!("thinking"))
        });
        assert!(has_start, "应 emit thinking content_block_start");
        let has_delta = evts.iter().any(|e| {
            e.event == "content_block_delta"
                && e.data
                    .get("delta")
                    .and_then(|d| d.get("type"))
                    .and_then(|t| t.as_str())
                    == Some("thinking_delta")
        });
        assert!(has_delta, "应 emit thinking_delta");
    }

    #[test]
    fn response_text_after_thought_closes_thinking_block() {
        let mut conv = GeminiSseConverter::new_with_extras("gemini-2.5-flash", true);
        let _ = conv.feed(&json!({
            "candidates": [{"content": {"parts": [
                {"thought": true, "text": "thinking summary"},
            ]}}],
        }));
        let evts = conv.feed(&json!({
            "candidates": [{"content": {"parts": [
                {"text": "real answer"},
            ]}}],
        }));
        // 应先 emit content_block_stop(thinking), 再 content_block_start(text)
        let stop_pos = evts.iter().position(|e| e.event == "content_block_stop");
        let start_pos = evts.iter().position(|e| {
            e.event == "content_block_start"
                && e.data.get("content_block").and_then(|cb| cb.get("type"))
                    == Some(&json!("text"))
        });
        assert!(
            stop_pos.is_some() && start_pos.is_some() && stop_pos.unwrap() < start_pos.unwrap(),
            "thinking 块应在 text 块开始前关闭: {:?}",
            evts.iter().map(|e| e.event).collect::<Vec<_>>()
        );
    }

    #[test]
    fn response_thought_signature_emitted_as_signature_delta() {
        let mut conv = GeminiSseConverter::new_with_extras("gemini-2.5-flash", true);
        let _ = conv.feed(&json!({
            "candidates": [{"content": {"parts": [
                {"thought": true, "text": "x"},
                {"thoughtSignature": "BUFFER_SIG_BYTES"},
            ]}}],
        }));
        // close 块时 emit signature_delta + content_block_stop
        let evts = conv.feed(&json!({
            "candidates": [{"content": {"parts": [{"text": "answer"}]}}],
        }));
        let sig_delta = evts.iter().find(|e| {
            e.event == "content_block_delta"
                && e.data
                    .get("delta")
                    .and_then(|d| d.get("type"))
                    .and_then(|t| t.as_str())
                    == Some("signature_delta")
        });
        let sig_delta = sig_delta.expect("应 emit signature_delta 关 thinking 块");
        let sig = sig_delta.data["delta"]["signature"].as_str().unwrap();
        let decoded = decode_gemini_thought_signature(sig).unwrap();
        assert_eq!(decoded, "BUFFER_SIG_BYTES");
    }

    #[test]
    fn response_thoughts_skipped_when_emit_disabled() {
        let mut conv = GeminiSseConverter::new_with_extras("gemini-2.5-flash", false);
        let evts = conv.feed(&json!({
            "candidates": [{"content": {"parts": [
                {"thought": true, "text": "should be dropped"},
                {"text": "kept"},
            ]}}],
        }));
        // 只应有 message_start + text 相关事件, 没有 thinking 块
        let has_thinking = evts.iter().any(|e| {
            e.data.get("content_block").and_then(|cb| cb.get("type"))
                == Some(&json!("thinking"))
                || e.data
                    .get("delta")
                    .and_then(|d| d.get("type"))
                    .and_then(|t| t.as_str())
                    == Some("thinking_delta")
        });
        assert!(!has_thinking, "emit_thoughts=false 时不应有 thinking 事件");
    }

    #[test]
    fn nonstreaming_collector_assembles_thinking_block_with_signature() {
        let mut col = NonStreamingCollector::new_with_extras("gemini-2.5-flash", true);
        col.feed(&json!({
            "candidates": [{"content": {"parts": [
                {"thought": true, "text": "step 1 "},
                {"thought": true, "text": "step 2"},
                {"thoughtSignature": "TS_FULL"},
                {"text": "final answer"},
            ]}, "finishReason": "STOP"}],
        }));
        let msg = col.finalize();
        let content = msg["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["thinking"], "step 1 step 2");
        let sig = content[0]["signature"].as_str().unwrap();
        assert_eq!(decode_gemini_thought_signature(sig).unwrap(), "TS_FULL");
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "final answer");
    }

    // ============================================================
    // schema sanitize 测试 (修 Gemini 400 "Unknown name $schema/additionalProperties/...")
    // ============================================================

    #[test]
    fn sanitize_strips_top_level_schema_keyword() {
        let raw = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": { "x": { "type": "string" } }
        });
        let out = sanitize_schema_for_gemini(raw);
        assert!(out.get("$schema").is_none());
        assert_eq!(out["type"], "object");
        assert_eq!(out["properties"]["x"]["type"], "string");
    }

    #[test]
    fn sanitize_strips_additional_properties_recursively() {
        let raw = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "nested": {
                    "type": "object",
                    "additionalProperties": false,
                    "propertyNames": { "pattern": "^[A-Z]" }
                }
            }
        });
        let out = sanitize_schema_for_gemini(raw);
        assert!(out.get("additionalProperties").is_none());
        assert!(out["properties"]["nested"].get("additionalProperties").is_none());
        assert!(out["properties"]["nested"].get("propertyNames").is_none());
    }

    #[test]
    fn sanitize_converts_exclusive_minimum_int() {
        let raw = json!({ "type": "integer", "exclusiveMinimum": 0 });
        let out = sanitize_schema_for_gemini(raw);
        assert!(out.get("exclusiveMinimum").is_none());
        assert_eq!(out["minimum"], 1);
    }

    #[test]
    fn sanitize_converts_exclusive_minimum_float() {
        let raw = json!({ "type": "number", "exclusiveMinimum": 0.5 });
        let out = sanitize_schema_for_gemini(raw);
        assert!(out.get("exclusiveMinimum").is_none());
        assert_eq!(out["minimum"].as_f64().unwrap(), 0.5);
    }

    #[test]
    fn sanitize_keeps_existing_minimum_when_exclusive_also_present() {
        let raw = json!({ "type": "integer", "minimum": 5, "exclusiveMinimum": 0 });
        let out = sanitize_schema_for_gemini(raw);
        assert_eq!(out["minimum"], 5, "已有的 minimum 不能被 exclusive 转换覆盖");
    }

    #[test]
    fn sanitize_converts_const_to_enum() {
        let raw = json!({ "const": "ACTIVE" });
        let out = sanitize_schema_for_gemini(raw);
        assert!(out.get("const").is_none());
        assert_eq!(out["enum"], json!(["ACTIVE"]));
    }

    #[test]
    fn sanitize_converts_one_of_to_any_of() {
        let raw = json!({
            "oneOf": [{ "type": "string" }, { "type": "number" }]
        });
        let out = sanitize_schema_for_gemini(raw);
        assert!(out.get("oneOf").is_none());
        assert!(out["anyOf"].is_array());
        assert_eq!(out["anyOf"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn sanitize_recurses_into_items() {
        let raw = json!({
            "type": "array",
            "items": {
                "type": "object",
                "$schema": "drop me",
                "additionalProperties": false
            }
        });
        let out = sanitize_schema_for_gemini(raw);
        assert!(out["items"].get("$schema").is_none());
        assert!(out["items"].get("additionalProperties").is_none());
    }

    #[test]
    fn sanitize_recurses_into_any_of_branches() {
        let raw = json!({
            "anyOf": [
                { "type": "string", "$schema": "x" },
                { "const": "OK" }
            ]
        });
        let out = sanitize_schema_for_gemini(raw);
        assert!(out["anyOf"][0].get("$schema").is_none());
        assert!(out["anyOf"][1].get("const").is_none());
        assert_eq!(out["anyOf"][1]["enum"], json!(["OK"]));
    }

    #[test]
    fn convert_tool_produces_clean_gemini_parameters_for_read_tool() {
        // 模拟 dump 里 Claude Code 真实的 Read tool schema
        let read_tool = json!({
            "name": "Read",
            "description": "Reads a file from the local filesystem.",
            "input_schema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "additionalProperties": false,
                "type": "object",
                "required": ["file_path"],
                "properties": {
                    "file_path": { "description": "absolute path", "type": "string" },
                    "limit": {
                        "description": "The number of lines to read.",
                        "exclusiveMinimum": 0,
                        "maximum": 9007199254740991_i64,
                        "type": "integer"
                    }
                }
            }
        });
        let out = convert_tool(&read_tool).unwrap();
        let params = &out["parameters"];
        assert!(params.get("$schema").is_none());
        assert!(params.get("additionalProperties").is_none());
        assert!(params["properties"]["limit"].get("exclusiveMinimum").is_none());
        assert_eq!(params["properties"]["limit"]["minimum"], 1);
        assert_eq!(params["properties"]["limit"]["maximum"], 9007199254740991_i64);
    }
}
