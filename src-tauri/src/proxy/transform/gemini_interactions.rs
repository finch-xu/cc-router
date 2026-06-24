//! Google Gemini **Interactions API** (`/v1beta/interactions`) ↔ Anthropic Messages 协议翻译.
//!
//! 与 [`super::gemini`] (旧 generateContent) **完全不同的协议**, 走独立翻译层:
//! - model 在 **body** 里 (不嵌 URL), dispatch 层 URL 固定 `/v1beta/interactions?alt=sse`
//! - 请求用 `input` (**step_list**) + `system_instruction`(string), 不是 `contents[]`
//! - 响应是 `steps[]`; SSE 是标准 `event: X\ndata: {...}\n\n` (LF), 有 `step.start/delta/stop` 事件
//! - usage 是 snake_case (`total_input_tokens`/`total_output_tokens`/`total_cached_tokens`/`total_thought_tokens`)
//!
//! 协议事实来自真实 curl 实测 (2026-06-24). 复用 [`super::gemini::sanitize_schema_for_gemini`]
//! 清理工具参数 JSON Schema; 复用 [`super::gemini::AnthropicEvent`] 作为输出表示。
//!
//! ## 请求侧 (`anthropic_to_interactions`)
//!
//! | Anthropic | Interactions input step |
//! |---|---|
//! | user text | `{type:"user_input", content:[{type:"text",text}]}` |
//! | assistant text | `{type:"model_output", content:[{type:"text",text}]}` |
//! | assistant thinking(signature) | `{type:"thought", signature:<解码原文>}` (排在同条 assistant 的 function_call 前) |
//! | assistant tool_use | `{type:"function_call", id, name, arguments:<对象>}` |
//! | user tool_result | `{type:"function_result", name:<查表>, result:<内容>}` |
//! | tools[] | `tools:[{type:"function", name, description, parameters}]` (扁平) |
//! | system | `system_instruction`(string) |
//! | temperature/top_p/max_tokens | `generation_config.{temperature, top_p, max_output_tokens}` |
//! | thinking.effort/budget | `generation_config.thinking_level`(low/medium/high) |
//!
//! ## 响应侧 (`InteractionsSseConverter`)
//!
//! SSE 事件按 `event_type` 分发, `index` = step 序号 ↔ 一个 Anthropic content_block:
//! - `interaction.created` → `message_start`
//! - `step.start{step.type}` → `content_block_start` (thought→thinking, model_output→text, function_call→tool_use)
//! - `step.delta{delta.type}` → text→text_delta / thought_signature→signature_delta(累积) / arguments→input_json_delta
//! - `step.stop` → `content_block_stop`
//! - `interaction.completed` → `message_delta`(stop_reason+usage) + `message_stop`

use std::collections::HashMap;

use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::AppResult;
use crate::proxy::transform::gemini::{sanitize_schema_for_gemini, AnthropicEvent};
use crate::proxy::transform::responses_common::anthropic_system_to_text;

// ============================================================
// Extras (dispatch 层填入)
// ============================================================

/// `anthropic_to_interactions` 的可选项. dispatch 层从 yaml / 客户端 body 推导后传入.
#[derive(Debug, Clone, Default)]
pub struct InteractionsExtras {
    /// `generation_config.thinking_level` 值 (low/medium/high). None 表示不注入。
    pub thinking_level: Option<String>,
    /// 是否暴露 thought (reasoning) + 回灌 thought signature. yaml `expose_reasoning` 映射来。
    pub include_thoughts: bool,
}

// ============================================================
// effort → thinking_level
// ============================================================

/// Anthropic `thinking.effort` → Interactions `generation_config.thinking_level`.
///
/// Interactions 用字符串档位 (实测 `high` 可用; low/medium 按文档推断 — 见 Phase 0 TODO),
/// 不像旧 generateContent 用整数 thinkingBudget。`xhigh`/`max` 暂统一收敛到 `high`。
pub fn effort_to_thinking_level(effort: &str) -> Option<&'static str> {
    match effort {
        "minimal" | "low" => Some("low"),
        "medium" => Some("medium"),
        "high" | "xhigh" | "max" => Some("high"),
        _ => None,
    }
}

/// 整数 budget_tokens 阈值 → thinking_level (与 [`super::gemini::effort_to_budget`] 反向对齐)。
fn budget_to_thinking_level(budget: i64) -> &'static str {
    if budget < 0 {
        "high" // -1 dynamic → 最高档
    } else if budget <= 4096 {
        "low"
    } else if budget <= 24576 {
        "medium"
    } else {
        "high"
    }
}

/// thinking_level 优先级链:
/// 1. `body.thinking.effort` (string) — 直接映射到 level, 最无损, 故置首
/// 2. `body.thinking.budget_tokens` (整数, 阈值映射)
/// 3. `body.extra_body.reasoning_effort` (string)
/// 4. `yaml_default_effort`
///
/// 注: 与 [`super::gemini::resolve_thinking_budget`] 的来源集相同, 但**刻意把 effort 置于
/// budget_tokens 之前** — 这里目标是离散档位 (low/medium/high), effort 字符串直接对应档位无损,
/// 而 budget_tokens 是阈值近似; budget 版返回整数预算, 故先取 budget_tokens。两者语义不同, 不强求同序。
pub fn resolve_thinking_level(body: &Value, yaml_default_effort: Option<&str>) -> Option<String> {
    if let Some(thinking) = body.get("thinking") {
        if let Some(s) = thinking.get("effort").and_then(|x| x.as_str()) {
            if let Some(l) = effort_to_thinking_level(s) {
                return Some(l.to_string());
            }
        }
        if let Some(bt) = thinking.get("budget_tokens").and_then(|x| x.as_i64()) {
            return Some(budget_to_thinking_level(bt).to_string());
        }
    }
    if let Some(s) = body
        .get("extra_body")
        .and_then(|x| x.get("reasoning_effort"))
        .and_then(|x| x.as_str())
    {
        if let Some(l) = effort_to_thinking_level(s) {
            return Some(l.to_string());
        }
    }
    yaml_default_effort
        .and_then(effort_to_thinking_level)
        .map(str::to_string)
}

// ============================================================
// thought signature 编码 (interactions 多轮回灌)
// ============================================================

/// Interactions `thought.signature` 在 Anthropic thinking content_block 的 `signature` 字段
/// 编码格式: `base64url(JSON{v:1, p:"gemini_interactions", ts:"<signature原文>"})`.
///
/// `p` 字段是 provider 区分标识 — 与 gemini (generateContent) 的 `p:"gemini"` / openai 的 `{v,id,ec}`
/// 物理隔离, 避免订阅切换时把别家 signature 喂回 Interactions 上游导致 400 (对齐
/// `project-think-effort-unification` 记忆)。
const SIG_VERSION: u64 = 1;
const SIG_PROVIDER: &str = "gemini_interactions";

pub fn encode_interactions_thought_signature(signature: &str) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    let payload = json!({
        "v": SIG_VERSION,
        "p": SIG_PROVIDER,
        "ts": signature,
    });
    let json_str = serde_json::to_string(&payload).unwrap_or_default();
    URL_SAFE_NO_PAD.encode(json_str.as_bytes())
}

pub fn decode_interactions_thought_signature(signature: &str) -> Option<String> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    if signature.is_empty() {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(signature.as_bytes()).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    if v.get("v").and_then(|x| x.as_u64()) != Some(SIG_VERSION) {
        return None;
    }
    if v.get("p").and_then(|x| x.as_str()) != Some(SIG_PROVIDER) {
        // 别家 (gemini / openai) signature 错喂到 interactions, 静默 drop 避免污染上游
        return None;
    }
    v.get("ts").and_then(|x| x.as_str()).map(str::to_string)
}

// ============================================================
// 请求转换
// ============================================================

/// 把 Anthropic Messages 请求体翻译成 Gemini Interactions 请求体.
/// `real_model` 写进 body 的 `model` (Interactions 的 model 在 body, 不在 URL).
pub fn anthropic_to_interactions(
    body: &Value,
    real_model: &str,
    extras: &InteractionsExtras,
) -> AppResult<Value> {
    let mut out = json!({
        "model": real_model,
        // 上游始终 streaming (dispatch 已拼 ?alt=sse), 客户端要非流式时由 collector 收齐。
        "stream": true,
        // 无状态代理: 不让上游存档 interaction (也使响应不返顶层 id)。
        "store": false,
    });

    // system → system_instruction (纯字符串). 复用 responses_common 的 pub helper, 不再自带一份。
    if let Some(sys) = body.get("system") {
        let text = anthropic_system_to_text(sys);
        if !text.is_empty() {
            out["system_instruction"] = json!(text);
        }
    }

    // messages[] → input (step_list)
    if let Some(msgs) = body.get("messages").and_then(|m| m.as_array()) {
        out["input"] = json!(messages_to_steps(msgs));
    }

    // tools[] → 扁平 function 声明
    if let Some(tools) = body.get("tools").and_then(|t| t.as_array()) {
        let decls: Vec<Value> = tools.iter().filter_map(convert_tool).collect();
        if !decls.is_empty() {
            out["tools"] = json!(decls);
        }
    }
    // tool_choice: Interactions 映射待 Phase 0 确认, CC 极少传, 暂不翻译。

    // generation_config (snake_case)
    let mut gen_config = serde_json::Map::new();
    if let Some(v) = body.get("temperature") {
        gen_config.insert("temperature".into(), v.clone());
    }
    if let Some(v) = body.get("top_p") {
        gen_config.insert("top_p".into(), v.clone());
    }
    if let Some(v) = body.get("max_tokens") {
        gen_config.insert("max_output_tokens".into(), v.clone());
    }
    if let Some(level) = &extras.thinking_level {
        gen_config.insert("thinking_level".into(), json!(level));
    }
    // stop_sequences: Interactions 字段名未实测, 注入未知字段会 400 拒整请求, 暂不译 (Phase 0)。
    if !gen_config.is_empty() {
        out["generation_config"] = Value::Object(gen_config);
    }

    Ok(out)
}

/// Anthropic messages → Interactions `input` step_list.
///
/// 维护 `tool_use_id → name` 映射: Anthropic tool_result 只带 `tool_use_id`, 而 Interactions
/// `function_result` 用 `name` 关联, 所以走 assistant tool_use 时记录, user tool_result 查表。
fn messages_to_steps(msgs: &[Value]) -> Vec<Value> {
    let mut steps: Vec<Value> = Vec::new();
    let mut tool_names: HashMap<String, String> = HashMap::new();

    for m in msgs {
        let role = m.get("role").and_then(|v| v.as_str()).unwrap_or("user");
        let content = m.get("content");
        match content {
            Some(Value::String(text)) => {
                if !text.is_empty() {
                    steps.push(text_step(role, text));
                }
            }
            Some(Value::Array(blocks)) => {
                if role == "assistant" {
                    push_assistant_steps(blocks, &mut steps, &mut tool_names);
                } else {
                    push_user_steps(blocks, &mut steps, &tool_names);
                }
            }
            _ => {}
        }
    }
    steps
}

fn text_step(role: &str, text: &str) -> Value {
    let step_type = if role == "assistant" {
        "model_output"
    } else {
        "user_input"
    };
    json!({
        "type": step_type,
        "content": [{"type": "text", "text": text}],
    })
}

/// assistant content blocks → steps (thought / model_output / function_call).
/// text 块合并进一个 model_output content[]; thinking 与 tool_use 各成独立 step。
fn push_assistant_steps(
    blocks: &[Value],
    steps: &mut Vec<Value>,
    tool_names: &mut HashMap<String, String>,
) {
    let mut text_content: Vec<Value> = Vec::new();
    let flush_text = |text_content: &mut Vec<Value>, steps: &mut Vec<Value>| {
        if !text_content.is_empty() {
            steps.push(json!({
                "type": "model_output",
                "content": std::mem::take(text_content),
            }));
        }
    };

    for blk in blocks {
        match blk.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "text" => {
                if let Some(t) = blk.get("text").and_then(|v| v.as_str()) {
                    text_content.push(json!({"type": "text", "text": t}));
                }
            }
            "thinking" => {
                // 回灌 thought signature (排在 function_call 之前). thinking 文本不回传 —
                // 上游靠 signature 恢复 reasoning context。
                if let Some(sig) = blk.get("signature").and_then(|v| v.as_str()) {
                    if let Some(raw) = decode_interactions_thought_signature(sig) {
                        if !raw.is_empty() {
                            // 同条 assistant 里 thought 应在 model_output / function_call 之前
                            flush_text(&mut text_content, steps);
                            steps.push(json!({"type": "thought", "signature": raw}));
                        }
                    }
                }
            }
            "tool_use" => {
                flush_text(&mut text_content, steps);
                let id = blk.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let name = blk.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let args = blk.get("input").cloned().unwrap_or(json!({}));
                if !id.is_empty() && !name.is_empty() {
                    tool_names.insert(id.to_string(), name.to_string());
                }
                let mut fc = json!({
                    "type": "function_call",
                    "name": name,
                    "arguments": args,
                });
                if !id.is_empty() {
                    fc["id"] = json!(id);
                }
                steps.push(fc);
            }
            _ => {}
        }
    }
    flush_text(&mut text_content, steps);
}

/// user content blocks → steps. text/image 合并进 user_input; tool_result → function_result。
fn push_user_steps(blocks: &[Value], steps: &mut Vec<Value>, tool_names: &HashMap<String, String>) {
    let mut input_content: Vec<Value> = Vec::new();
    let flush_input = |input_content: &mut Vec<Value>, steps: &mut Vec<Value>| {
        if !input_content.is_empty() {
            steps.push(json!({
                "type": "user_input",
                "content": std::mem::take(input_content),
            }));
        }
    };

    for blk in blocks {
        match blk.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "text" => {
                if let Some(t) = blk.get("text").and_then(|v| v.as_str()) {
                    input_content.push(json!({"type": "text", "text": t}));
                }
            }
            "image" => {
                // Anthropic image base64 → Interactions image content (字段形态待 Phase 0; 暂跳过)
            }
            "tool_result" => {
                flush_input(&mut input_content, steps);
                let name = blk
                    .get("tool_use_id")
                    .and_then(|v| v.as_str())
                    .and_then(|id| tool_names.get(id).map(String::as_str))
                    .or_else(|| blk.get("name").and_then(|v| v.as_str()))
                    .unwrap_or("")
                    .to_string();
                // 实测确认 (2026-06-24): function_result 用 `name` (不接受 id) + `result` (字符串)。
                // 关键: function_call 必须连同前序 thought signature 一起回灌, 否则上游 400
                // "invalid argument" (Gemini 2.5+ 强制 thoughtSignature)。本翻译层把 assistant 的
                // thinking 块解码成 thought step 排在 function_call 前 — 依赖 yaml expose_reasoning=true
                // 时客户端能拿到并回传 thinking 块。
                steps.push(json!({
                    "type": "function_result",
                    "name": name,
                    "result": tool_result_value(blk.get("content")),
                }));
            }
            _ => {}
        }
    }
    flush_input(&mut input_content, steps);
}

/// 把 Anthropic tool_result.content 渲染成 Interactions function_result.result 值。
fn tool_result_value(content: Option<&Value>) -> Value {
    match content {
        Some(Value::String(s)) => json!(s),
        Some(Value::Array(arr)) => {
            let text: String = arr
                .iter()
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n");
            json!(text)
        }
        Some(v) => v.clone(),
        None => json!(""),
    }
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
        "type": "function",
        "name": name,
        "description": description,
        "parameters": parameters,
    }))
}

// ============================================================
// 响应转换 (SSE 状态机)
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Thinking,
    Text,
    ToolUse,
}

struct BlockState {
    /// 对应的 Anthropic content_block index. `None` = 隐藏块 (emit_thoughts=false 的 thought),
    /// 仍占一个 step 槽位以消费其 delta/stop, 但不向客户端 emit 任何事件。
    anth_index: Option<u32>,
    kind: BlockKind,
    /// thought signature 原文 (close 时 encode 成 signature_delta)
    pending_signature: Option<String>,
}

/// Interactions SSE 状态机. 流式调用方按 `\n\n` 切帧 → `parse_interactions_sse_frame` 解 JSON →
/// `feed(json)` 拿到 Anthropic 事件序列。
pub struct InteractionsSseConverter {
    started: bool,
    stopped: bool,
    message_id: String,
    response_model: String,
    next_index: u32,
    /// upstream step index → 当前块状态
    blocks: HashMap<u64, BlockState>,
    emit_thoughts: bool,
    stop_reason: String,
    saw_function_call: bool,
    final_usage: Value,
}

impl InteractionsSseConverter {
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
            blocks: HashMap::new(),
            emit_thoughts,
            stop_reason: "end_turn".to_string(),
            saw_function_call: false,
            final_usage: json!({"input_tokens": 0, "output_tokens": 0}),
        }
    }

    pub fn feed(&mut self, frame: &Value) -> Vec<AnthropicEvent> {
        let mut out = Vec::new();
        let event_type = frame
            .get("event_type")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        match event_type {
            "interaction.created" => {
                if let Some(model) = frame
                    .get("interaction")
                    .and_then(|i| i.get("model"))
                    .and_then(|v| v.as_str())
                {
                    if !model.is_empty() {
                        self.response_model = model.to_string();
                    }
                }
                self.ensure_started(&mut out);
            }
            "step.start" => {
                self.ensure_started(&mut out);
                self.handle_step_start(frame, &mut out);
            }
            "step.delta" => {
                self.ensure_started(&mut out);
                self.handle_step_delta(frame, &mut out);
            }
            "step.stop" => {
                self.handle_step_stop(frame, &mut out);
            }
            "interaction.completed" => {
                if let Some(usage) = frame
                    .get("interaction")
                    .and_then(|i| i.get("usage"))
                {
                    self.absorb_usage(usage);
                }
                if let Some(status) = frame
                    .get("interaction")
                    .and_then(|i| i.get("status"))
                    .and_then(|v| v.as_str())
                {
                    self.stop_reason = self.resolve_stop_reason(status);
                }
                out.extend(self.finalize());
            }
            // interaction.status_update / error / 其他: 忽略 (error 由 dispatch 层处理)
            _ => {}
        }
        out
    }

    /// 流自然结束 (或 interaction.completed) 时收尾: 关掉残留块 + message_delta + message_stop。
    pub fn finalize(&mut self) -> Vec<AnthropicEvent> {
        let mut out = Vec::new();
        if !self.started || self.stopped {
            return out;
        }
        // 关掉所有残留 (可见) 块, 按 anth_index 顺序; 隐藏块 (None) 无需 emit, 跳过。
        let mut remaining: Vec<(u64, u32)> = self
            .blocks
            .iter()
            .filter_map(|(k, b)| b.anth_index.map(|idx| (*k, idx)))
            .collect();
        remaining.sort_by_key(|(_, idx)| *idx);
        for (step_idx, _) in remaining {
            self.close_block(step_idx, &mut out);
        }
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

    pub fn response_model(&self) -> &str {
        &self.response_model
    }

    pub fn usage(&self) -> &Value {
        &self.final_usage
    }

    // ---------- internals ----------

    fn ensure_started(&mut self, out: &mut Vec<AnthropicEvent>) {
        if self.started {
            return;
        }
        self.started = true;
        out.push(AnthropicEvent {
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
        });
    }

    fn handle_step_start(&mut self, frame: &Value, out: &mut Vec<AnthropicEvent>) {
        let step_idx = frame.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
        let step = frame.get("step");
        let step_type = step
            .and_then(|s| s.get("type"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let (kind, content_block) = match step_type {
            "thought" => {
                if !self.emit_thoughts {
                    // 不暴露 thinking: 记录块 (anth_index=None) 但不 emit, delta 阶段静默丢弃
                    self.blocks.insert(
                        step_idx,
                        BlockState {
                            anth_index: None,
                            kind: BlockKind::Thinking,
                            pending_signature: None,
                        },
                    );
                    return;
                }
                (BlockKind::Thinking, json!({"type": "thinking", "thinking": ""}))
            }
            "function_call" => {
                self.saw_function_call = true;
                let name = step
                    .and_then(|s| s.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let tool_id = step
                    .and_then(|s| s.get("id"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("toolu_{}", Uuid::new_v4().simple()));
                (
                    BlockKind::ToolUse,
                    json!({"type": "tool_use", "id": tool_id, "name": name, "input": {}}),
                )
            }
            // model_output (text) 与未知类型都按 text 块处理
            _ => (BlockKind::Text, json!({"type": "text", "text": ""})),
        };

        let anth_index = self.next_index;
        self.next_index += 1;
        self.blocks.insert(
            step_idx,
            BlockState {
                anth_index: Some(anth_index),
                kind,
                pending_signature: None,
            },
        );
        out.push(AnthropicEvent {
            event: "content_block_start",
            data: json!({
                "type": "content_block_start",
                "index": anth_index,
                "content_block": content_block,
            }),
        });

        // function_call 的 arguments 可能在 step.start 时已完整给出 (非流式形态)
        if kind == BlockKind::ToolUse {
            if let Some(args) = step.and_then(|s| s.get("arguments")) {
                if !args.is_null() {
                    let args_str = serde_json::to_string(args).unwrap_or_else(|_| "{}".into());
                    if args_str != "{}" {
                        out.push(input_json_delta(anth_index, &args_str));
                    }
                }
            }
        }
    }

    fn handle_step_delta(&mut self, frame: &Value, out: &mut Vec<AnthropicEvent>) {
        let step_idx = frame.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
        let Some(delta) = frame.get("delta") else {
            return;
        };
        let dtype = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let Some(block) = self.blocks.get_mut(&step_idx) else {
            return;
        };
        // 隐藏块 (anth_index=None) 不 emit 任何 delta — 一处 guard 覆盖所有 arm。
        let Some(anth_index) = block.anth_index else {
            return;
        };

        match dtype {
            "text" => {
                let Some(text) = delta.get("text").and_then(|v| v.as_str()) else {
                    return;
                };
                if text.is_empty() {
                    return;
                }
                let delta_type = if block.kind == BlockKind::Thinking {
                    ("thinking_delta", "thinking")
                } else {
                    ("text_delta", "text")
                };
                out.push(AnthropicEvent {
                    event: "content_block_delta",
                    data: json!({
                        "type": "content_block_delta",
                        "index": anth_index,
                        "delta": {"type": delta_type.0, delta_type.1: text},
                    }),
                });
            }
            "thought_signature" => {
                // 累积原文, close 块时 encode 成 signature_delta
                if let Some(sig) = delta.get("signature").and_then(|v| v.as_str()) {
                    if !sig.is_empty() {
                        block.pending_signature = Some(sig.to_string());
                    }
                }
            }
            "arguments" => {
                // arguments 增量 (ArgumentsDelta). 字段形态待 Phase 0 实测确认 — 兼容
                // arguments(string) / partial_json 两种字符串来源, 当 partial_json 透传。
                let chunk = delta
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .or_else(|| delta.get("partial_json").and_then(|v| v.as_str()));
                if let Some(s) = chunk {
                    if !s.is_empty() {
                        out.push(input_json_delta(anth_index, s));
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_step_stop(&mut self, frame: &Value, out: &mut Vec<AnthropicEvent>) {
        // step.stop 可能带阶段 usage (取末值, 最终以 interaction.completed 为准)
        if let Some(usage) = frame.get("usage") {
            self.absorb_usage(usage);
        }
        let step_idx = frame.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
        self.close_block(step_idx, out);
    }

    fn close_block(&mut self, step_idx: u64, out: &mut Vec<AnthropicEvent>) {
        let Some(block) = self.blocks.remove(&step_idx) else {
            return;
        };
        let Some(anth_index) = block.anth_index else {
            return; // hidden thought 块, 无 emit
        };
        // thinking 块: 先 emit signature_delta (若有) 再关
        if let Some(raw_sig) = block.pending_signature {
            let signature = encode_interactions_thought_signature(&raw_sig);
            out.push(AnthropicEvent {
                event: "content_block_delta",
                data: json!({
                    "type": "content_block_delta",
                    "index": anth_index,
                    "delta": {"type": "signature_delta", "signature": signature},
                }),
            });
        }
        out.push(AnthropicEvent {
            event: "content_block_stop",
            data: json!({"type": "content_block_stop", "index": anth_index}),
        });
    }

    fn resolve_stop_reason(&self, status: &str) -> String {
        if self.saw_function_call || status == "requires_action" {
            return "tool_use".to_string();
        }
        match status {
            "completed" => "end_turn",
            "incomplete" | "budget_exceeded" => "max_tokens",
            _ => "end_turn",
        }
        .to_string()
    }

    fn absorb_usage(&mut self, usage: &Value) {
        let input = usage
            .get("total_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let output = usage
            .get("total_output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let cached = usage.get("total_cached_tokens").and_then(|v| v.as_u64());
        let mut obj = json!({
            "input_tokens": input,
            "output_tokens": output,
        });
        if let Some(c) = cached {
            if c > 0 {
                obj["cache_read_input_tokens"] = json!(c);
            }
        }
        self.final_usage = obj;
    }
}

// ============================================================
// 非流式: 把 SSE 帧吃完, 还原成 Anthropic Messages 最终 JSON
// ============================================================

/// 给客户端非流式请求用. cc-router 始终发起流式上游, 这里累积 SSE 帧到完整 Anthropic 响应 JSON。
pub struct InteractionsNonStreamingCollector {
    converter: InteractionsSseConverter,
    text_acc: HashMap<u32, String>,
    thinking_acc: HashMap<u32, String>,
    signature_acc: HashMap<u32, String>,
    /// tool_use args 的原始 partial_json 分片累积 (finalize 时一次性 parse, 避免每片重 parse 全量)
    tool_args_acc: HashMap<u32, String>,
    block_meta: HashMap<u32, Value>,
    order: Vec<u32>,
}

impl InteractionsNonStreamingCollector {
    pub fn new(response_model: &str) -> Self {
        Self::new_with_extras(response_model, false)
    }

    pub fn new_with_extras(response_model: &str, emit_thoughts: bool) -> Self {
        Self {
            converter: InteractionsSseConverter::new_with_extras(response_model, emit_thoughts),
            text_acc: HashMap::new(),
            thinking_acc: HashMap::new(),
            signature_acc: HashMap::new(),
            tool_args_acc: HashMap::new(),
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
                    "tool_use" => {
                        // 累积的 args 分片此刻才整体 parse 一次。
                        let input = self
                            .tool_args_acc
                            .get(idx)
                            .and_then(|s| serde_json::from_str::<Value>(s).ok())
                            .unwrap_or(json!({}));
                        Some(json!({
                            "type": "tool_use",
                            "id": meta.get("id").cloned().unwrap_or(Value::Null),
                            "name": meta.get("name").cloned().unwrap_or(Value::Null),
                            "input": input,
                        }))
                    }
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
                        // tool_use args 分片直接 append (O(1) 摊还), finalize 时一次性 parse。
                        // (Interactions 可能流式分片下发 arguments, 也可能 step.start 一次性给全。)
                        if let Some(s) = delta.get("partial_json").and_then(|v| v.as_str()) {
                            self.tool_args_acc.entry(index).or_default().push_str(s);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// 构造 tool_use 的 `input_json_delta` 事件 (自由函数, 不依赖 converter 状态 — 避免借用冲突)。
fn input_json_delta(anth_index: u32, partial_json: &str) -> AnthropicEvent {
    AnthropicEvent {
        event: "content_block_delta",
        data: json!({
            "type": "content_block_delta",
            "index": anth_index,
            "delta": {"type": "input_json_delta", "partial_json": partial_json},
        }),
    }
}

// ============================================================
// SSE 帧解析
// ============================================================

/// 解 Interactions SSE 帧. 标准 SSE: `event: <type>\ndata: <json>\n\n` (判别器在 data 的 `event_type`)。
/// 取 `data:` 行拼 JSON; 忽略 `event:` / `:` 心跳行; `data: [DONE]` 或空 → None。
pub fn parse_interactions_sse_frame(raw: &str) -> Option<Value> {
    let mut data = String::new();
    for line in raw.lines() {
        let rest = line
            .strip_prefix("data: ")
            .or_else(|| line.strip_prefix("data:"));
        if let Some(rest) = rest {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest);
        }
    }
    let trimmed = data.trim();
    if trimmed.is_empty() || trimmed == "[DONE]" {
        return None;
    }
    serde_json::from_str(&data).ok()
}

// ============================================================
// 测试 (fixture 取自 2026-06-24 真实 curl 实测字节)
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn extras(thinking_level: Option<&str>, include_thoughts: bool) -> InteractionsExtras {
        InteractionsExtras {
            thinking_level: thinking_level.map(str::to_string),
            include_thoughts,
        }
    }

    // ---------- 请求翻译 ----------

    #[test]
    fn request_sets_model_stream_store() {
        let body = json!({"model": "model-sonnet", "messages": [{"role": "user", "content": "hi"}]});
        let out = anthropic_to_interactions(&body, "gemini-2.5-flash", &extras(None, false)).unwrap();
        assert_eq!(out["model"], "gemini-2.5-flash");
        assert_eq!(out["stream"], true);
        assert_eq!(out["store"], false);
    }

    #[test]
    fn request_string_content_becomes_user_input_step() {
        let body = json!({"model": "m", "messages": [{"role": "user", "content": "hello"}]});
        let out = anthropic_to_interactions(&body, "gemini-2.5-flash", &extras(None, false)).unwrap();
        let input = out["input"].as_array().unwrap();
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["type"], "user_input");
        assert_eq!(input[0]["content"][0]["type"], "text");
        assert_eq!(input[0]["content"][0]["text"], "hello");
    }

    #[test]
    fn request_multi_turn_step_list() {
        // 对应 probe 6b 实测通过的 step_list 形态。
        let body = json!({
            "model": "m",
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "My name is Bob."}]},
                {"role": "assistant", "content": [{"type": "text", "text": "Nice to meet you, Bob."}]},
                {"role": "user", "content": [{"type": "text", "text": "What is my name?"}]}
            ]
        });
        let out = anthropic_to_interactions(&body, "m", &extras(None, false)).unwrap();
        let input = out["input"].as_array().unwrap();
        assert_eq!(input.len(), 3);
        assert_eq!(input[0]["type"], "user_input");
        assert_eq!(input[1]["type"], "model_output");
        assert_eq!(input[1]["content"][0]["text"], "Nice to meet you, Bob.");
        assert_eq!(input[2]["type"], "user_input");
    }

    #[test]
    fn request_system_becomes_string_instruction() {
        let body = json!({
            "model": "m",
            "system": [{"type": "text", "text": "Be terse."}, {"type": "text", "text": "Reply once."}],
            "messages": [{"role": "user", "content": "hi"}]
        });
        let out = anthropic_to_interactions(&body, "m", &extras(None, false)).unwrap();
        assert_eq!(out["system_instruction"], "Be terse.\n\nReply once.");
    }

    #[test]
    fn request_tools_are_flat_function_declarations() {
        let body = json!({
            "model": "m",
            "messages": [{"role": "user", "content": "weather?"}],
            "tools": [{
                "name": "get_weather",
                "description": "Get weather",
                "input_schema": {"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}
            }]
        });
        let out = anthropic_to_interactions(&body, "m", &extras(None, false)).unwrap();
        let tools = out["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["name"], "get_weather");
        assert_eq!(tools[0]["parameters"]["properties"]["city"]["type"], "string");
    }

    #[test]
    fn request_thinking_level_into_generation_config() {
        let body = json!({"model": "m", "messages": [{"role": "user", "content": "hi"}], "max_tokens": 100, "temperature": 0.5});
        let out = anthropic_to_interactions(&body, "m", &extras(Some("high"), true)).unwrap();
        let gc = &out["generation_config"];
        assert_eq!(gc["thinking_level"], "high");
        assert_eq!(gc["max_output_tokens"], 100);
        assert_eq!(gc["temperature"], 0.5);
    }

    #[test]
    fn request_tool_roundtrip_maps_id_to_name_and_replays_thought() {
        // assistant 含 thinking(signature) + tool_use; 随后 user 回 tool_result。
        let sig = encode_interactions_thought_signature("RAWSIG123");
        let body = json!({
            "model": "m",
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "weather in Tokyo?"}]},
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "...", "signature": sig},
                    {"type": "tool_use", "id": "toolu_abc", "name": "get_weather", "input": {"city": "Tokyo"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_abc", "content": "22C sunny"}
                ]}
            ]
        });
        let out = anthropic_to_interactions(&body, "m", &extras(None, true)).unwrap();
        let input = out["input"].as_array().unwrap();
        // user_input, thought, function_call, function_result
        assert_eq!(input[0]["type"], "user_input");
        assert_eq!(input[1]["type"], "thought");
        assert_eq!(input[1]["signature"], "RAWSIG123"); // 解码还原原文
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[2]["name"], "get_weather");
        assert_eq!(input[2]["arguments"]["city"], "Tokyo");
        // function_result 用 tool_use_id 查表得到 name
        assert_eq!(input[3]["type"], "function_result");
        assert_eq!(input[3]["name"], "get_weather");
    }

    #[test]
    fn assistant_blocks_become_steps_in_source_order() {
        // 各 block (tool_use / thinking) 各成独立 step, 顺序严格跟随 Anthropic 源序。
        // (thinking-before-tool 的正常顺序由 request_tool_roundtrip_... 验证。)
        let sig = encode_interactions_thought_signature("S");
        let body = json!({
            "model": "m",
            "messages": [{"role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "f", "input": {}},
                {"type": "thinking", "thinking": "x", "signature": sig}
            ]}]
        });
        let out = anthropic_to_interactions(&body, "m", &extras(None, true)).unwrap();
        let types: Vec<&str> = out["input"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["type"].as_str().unwrap())
            .collect();
        // 源序是 [tool_use, thinking] → 输出严格 [function_call, thought]
        assert_eq!(types, ["function_call", "thought"]);
    }

    // ---------- signature 编解码 ----------

    #[test]
    fn signature_roundtrip_and_provider_isolation() {
        let enc = encode_interactions_thought_signature("ABC123");
        assert_eq!(decode_interactions_thought_signature(&enc), Some("ABC123".to_string()));
        // 别家 signature (gemini generateContent) 错喂进来应被拒。
        let gemini_sig = crate::proxy::transform::gemini::encode_gemini_thought_signature("ABC123");
        assert_eq!(decode_interactions_thought_signature(&gemini_sig), None);
        // 反向: 我们的 signature 不应被 gemini decode 接受。
        assert_eq!(
            crate::proxy::transform::gemini::decode_gemini_thought_signature(&enc),
            None
        );
        // 垃圾输入
        assert_eq!(decode_interactions_thought_signature("not-base64!"), None);
        assert_eq!(decode_interactions_thought_signature(""), None);
    }

    #[test]
    fn effort_to_thinking_level_mapping() {
        assert_eq!(effort_to_thinking_level("minimal"), Some("low"));
        assert_eq!(effort_to_thinking_level("medium"), Some("medium"));
        assert_eq!(effort_to_thinking_level("max"), Some("high"));
        assert_eq!(effort_to_thinking_level("bogus"), None);

        let body = json!({"thinking": {"effort": "high"}});
        assert_eq!(resolve_thinking_level(&body, None), Some("high".to_string()));
        let body = json!({"thinking": {"budget_tokens": 30000}});
        assert_eq!(resolve_thinking_level(&body, None), Some("high".to_string()));
        let body = json!({});
        assert_eq!(resolve_thinking_level(&body, Some("low")), Some("low".to_string()));
    }

    // ---------- SSE 帧解析 (真实字节) ----------

    #[test]
    fn parse_frame_extracts_data_ignores_event_line() {
        let frame = "event: step.delta\ndata: {\"index\":1,\"delta\":{\"text\":\"hi\",\"type\":\"text\"},\"event_type\":\"step.delta\"}";
        let v = parse_interactions_sse_frame(frame).unwrap();
        assert_eq!(v["event_type"], "step.delta");
        assert_eq!(v["delta"]["text"], "hi");
    }

    #[test]
    fn parse_frame_done_and_empty_return_none() {
        assert!(parse_interactions_sse_frame("event: done\ndata: [DONE]").is_none());
        assert!(parse_interactions_sse_frame(": heartbeat").is_none());
        assert!(parse_interactions_sse_frame("").is_none());
    }

    // ---------- 响应 SSE 状态机 (真实事件序列) ----------

    /// probe 3b 实测的完整事件序列 (thought signature + text)。
    fn real_event_sequence() -> Vec<Value> {
        vec![
            json!({"interaction":{"id":"","status":"in_progress","object":"interaction","model":"gemini-2.5-flash-lite"},"event_type":"interaction.created"}),
            json!({"interaction_id":"","status":"in_progress","event_type":"interaction.status_update"}),
            json!({"index":0,"step":{"type":"thought"},"event_type":"step.start"}),
            json!({"index":0,"delta":{"signature":"CiRlMjQ4MzBhNy01Y2Q2","type":"thought_signature"},"event_type":"step.delta"}),
            json!({"index":0,"usage":{"total_input_tokens":8,"total_output_tokens":1},"event_type":"step.stop"}),
            json!({"index":1,"step":{"type":"model_output"},"event_type":"step.start"}),
            json!({"index":1,"delta":{"text":"pineapple","type":"text"},"event_type":"step.delta"}),
            json!({"index":1,"event_type":"step.stop"}),
            json!({"interaction":{"id":"","status":"completed","usage":{"total_input_tokens":8,"total_output_tokens":1,"total_cached_tokens":0},"model":"gemini-2.5-flash-lite"},"event_type":"interaction.completed"}),
        ]
    }

    #[test]
    fn sse_converter_text_with_thinking() {
        let mut conv = InteractionsSseConverter::new_with_extras("gemini-2.5-flash-lite", true);
        let mut events: Vec<AnthropicEvent> = Vec::new();
        for frame in real_event_sequence() {
            events.extend(conv.feed(&frame));
        }
        let names: Vec<&str> = events.iter().map(|e| e.event).collect();
        // message_start 一次
        assert_eq!(names.iter().filter(|n| **n == "message_start").count(), 1);
        // thinking 块 (index 0) start + signature_delta + stop
        let starts: Vec<&Value> = events
            .iter()
            .filter(|e| e.event == "content_block_start")
            .map(|e| &e.data)
            .collect();
        assert_eq!(starts[0]["content_block"]["type"], "thinking");
        assert_eq!(starts[1]["content_block"]["type"], "text");
        // 有 signature_delta
        assert!(events.iter().any(|e| e.event == "content_block_delta"
            && e.data["delta"]["type"] == "signature_delta"));
        // text_delta 内容
        assert!(events.iter().any(|e| e.event == "content_block_delta"
            && e.data["delta"]["type"] == "text_delta"
            && e.data["delta"]["text"] == "pineapple"));
        // message_delta(end_turn) + message_stop 收尾
        let md = events.iter().find(|e| e.event == "message_delta").unwrap();
        assert_eq!(md.data["delta"]["stop_reason"], "end_turn");
        assert_eq!(md.data["usage"]["input_tokens"], 8);
        assert!(names.last() == Some(&"message_stop"));
    }

    #[test]
    fn sse_converter_hides_thinking_when_disabled() {
        let mut conv = InteractionsSseConverter::new_with_extras("m", false);
        let mut events: Vec<AnthropicEvent> = Vec::new();
        for frame in real_event_sequence() {
            events.extend(conv.feed(&frame));
        }
        // emit_thoughts=false: 不应有 thinking 块, 但 text 块仍在
        assert!(!events.iter().any(|e| e.event == "content_block_start"
            && e.data["content_block"]["type"] == "thinking"));
        assert!(events.iter().any(|e| e.event == "content_block_start"
            && e.data["content_block"]["type"] == "text"));
    }

    #[test]
    fn non_streaming_collector_builds_anthropic_json() {
        let mut col = InteractionsNonStreamingCollector::new_with_extras("gemini-2.5-flash-lite", true);
        for frame in real_event_sequence() {
            col.feed(&frame);
        }
        let msg = col.finalize();
        assert_eq!(msg["type"], "message");
        assert_eq!(msg["role"], "assistant");
        assert_eq!(msg["stop_reason"], "end_turn");
        let content = msg["content"].as_array().unwrap();
        // thinking 块 (带 signature) + text 块
        assert_eq!(content[0]["type"], "thinking");
        assert!(!content[0]["signature"].as_str().unwrap().is_empty());
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "pineapple");
        assert_eq!(msg["usage"]["input_tokens"], 8);
    }

    #[test]
    fn sse_converter_function_call_sets_tool_use_stop_reason() {
        // 基于实测 function_call step 结构 (id/name/arguments) 构造流式序列。
        let frames = vec![
            json!({"interaction":{"model":"m"},"event_type":"interaction.created"}),
            json!({"index":0,"step":{"type":"function_call","id":"ca1","name":"get_weather","arguments":{"city":"Tokyo"}},"event_type":"step.start"}),
            json!({"index":0,"event_type":"step.stop"}),
            json!({"interaction":{"status":"requires_action","usage":{"total_input_tokens":50,"total_output_tokens":15}},"event_type":"interaction.completed"}),
        ];
        let mut conv = InteractionsSseConverter::new_with_extras("m", false);
        let mut events: Vec<AnthropicEvent> = Vec::new();
        for f in frames {
            events.extend(conv.feed(&f));
        }
        let start = events
            .iter()
            .find(|e| e.event == "content_block_start")
            .unwrap();
        assert_eq!(start.data["content_block"]["type"], "tool_use");
        assert_eq!(start.data["content_block"]["name"], "get_weather");
        assert!(events.iter().any(|e| e.event == "content_block_delta"
            && e.data["delta"]["type"] == "input_json_delta"));
        let md = events.iter().find(|e| e.event == "message_delta").unwrap();
        assert_eq!(md.data["delta"]["stop_reason"], "tool_use");
    }
}
