//! 协议翻译模块. 各子模块仅供特定 `auth_type` 订阅使用, 不影响 cc-router 默认的
//! Anthropic 透传管线.
//!
//! 子模块:
//! - [`responses_common`] - OpenAI Responses 协议翻译共享 helper (ResponsesTransformConfig +
//!   build_responses_body + SSE 状态机 + NonStreamingCollector). codex / openai 入口都用它.
//! - [`responses_inbound`] - 反向: OpenAI Responses → Anthropic Messages. 用于 cc-router 对外的
//!   `POST /v1/responses` 兼容入口 (handler::responses), 让外部 agent 通过 OpenAI Responses SDK
//!   透明访问 cc-router 聚合的多家订阅。
//! - [`openai_responses`] - Anthropic Messages ↔ OpenAI Responses (ChatGPT 反代). 用于 `auth_type=ChatgptOauth`.
//!   入口: `anthropic_to_responses`. 重新导出 common 的 SSE converter / collector / parser.
//! - [`openai`] - Anthropic Messages ↔ OpenAI 官方/兼容 `/v1/responses`. 用于 `auth_type=OpenaiResponsesApiKey`.
//!   入口: `anthropic_to_openai_responses`, `responses_json_to_anthropic`.
//! - [`aws_event_stream`] - AWS Event Stream 二进制流解码器. 用于 Kiro/CodeWhisperer 响应解析.
//! - [`kiro_codewhisperer`] - Anthropic Messages ↔ AWS CodeWhisperer (Kiro IDE 后端). 用于 `auth_type=KiroOauth`.
//!   入口: `anthropic_to_codewhisperer`, `KiroSseConverter`, `NonStreamingCollector`.
//! - [`gemini`] - Anthropic Messages ↔ Google Gemini generateContent. 用于 `auth_type=GeminiApiKey`.
//!   入口: `anthropic_to_gemini`, `GeminiSseConverter`, `NonStreamingCollector`.
//! - [`gemini_interactions`] - Anthropic Messages ↔ Google Gemini Interactions API (`/v1beta/interactions`).
//!   用于 `auth_type=GeminiInteractionsApiKey`. 入口: `anthropic_to_interactions`,
//!   `InteractionsSseConverter`, `InteractionsNonStreamingCollector`. 与 [`gemini`] 协议不同, 独立实现.
//! - [`openai_chat_completions`] - Anthropic Messages ↔ OpenAI Chat Completions (`/v1/chat/completions`).
//!   用于 `auth_type=OpenaiChatCompletionsApiKey`. 入口: `anthropic_to_openai_chat`,
//!   `chat_json_to_anthropic`, `ChatCompletionsSseConverter`. 覆盖 DeepSeek/Together/Groq/Ollama/各类 one-api 中转.
//! - [`chat_completions_inbound`] - 反向: OpenAI Chat Completions → Anthropic Messages. 用于 cc-router 对外的
//!   `POST /v1/chat/completions` 兼容入口 (handler::chat_completions), 让 Open WebUI / Cherry Studio 等
//!   只支持 Chat Completions 的工具接入。与 [`openai_chat_completions`] (出站) 方向相反, 代码不共享。

pub mod responses_common;
pub mod responses_inbound;
pub mod openai_responses;
pub mod openai;
pub mod aws_event_stream;
pub mod kiro_codewhisperer;
pub mod gemini;
pub mod gemini_interactions;
pub mod openai_chat_completions;
pub mod chat_completions_inbound;

use serde_json::Value;

/// Anthropic 内置工具: `tools[]` 里带 `type` 且不是 `custom` 的项 —— 服务端执行的
/// (`web_search_*` / `web_fetch_*` / `code_execution_*` …) 或要上游认得定义的 (`bash_*` / `text_editor_*` …)。
/// 翻译类上游没有对应物: 若当成参数为空的普通函数发上去, 模型「调用」它时客户端找不到这个工具。
/// 所以翻译层一律丢弃它们; 需要这类能力的请求应该用规则插件的 `tool_type` 条件改道到支持的订阅。
pub fn is_builtin_tool(t: &Value) -> bool {
    t.get("type").and_then(Value::as_str).is_some_and(|ty| ty != "custom")
}

/// 可以翻译成函数声明的工具 (丢掉 [`is_builtin_tool`]); 有丢弃时记一条 warn (只记 type)。
pub fn function_tools(body: &Value) -> Vec<&Value> {
    let Some(tools) = body.get("tools").and_then(Value::as_array) else { return Vec::new() };
    let (builtin, functions): (Vec<&Value>, Vec<&Value>) = tools.iter().partition(|t| is_builtin_tool(t));
    if !builtin.is_empty() {
        let types: Vec<&str> = builtin.iter().filter_map(|t| t.get("type").and_then(Value::as_str)).collect();
        tracing::warn!(?types, "翻译类上游不支持 Anthropic 内置工具, 已从请求中去掉");
    }
    functions
}

/// 还能翻译的 `tool_choice`。只在「丢了内置工具」时才可能改变原行为 (其余情况原样返回):
/// 丢完一个函数都不剩 (上游会拒「有 tool_choice 没 tools」), 或指定的工具正是被丢掉的那个 → None, 不发。
pub fn usable_tool_choice<'a>(body: &'a Value, functions: &[&Value]) -> Option<&'a Value> {
    let tc = body.get("tool_choice")?;
    let builtin: Vec<&Value> = body
        .get("tools")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter(|t| is_builtin_tool(t)).collect())
        .unwrap_or_default();
    if builtin.is_empty() {
        return Some(tc);
    }
    if functions.is_empty() {
        return None;
    }
    let forced = (tc.get("type").and_then(Value::as_str) == Some("tool")).then(|| tc.get("name").and_then(Value::as_str)).flatten();
    if forced.is_some_and(|n| builtin.iter().any(|t| t.get("name").and_then(Value::as_str) == Some(n))) {
        return None;
    }
    Some(tc)
}

#[cfg(test)]
mod builtin_tool_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builtin_tools_are_dropped_and_tool_choice_follows() {
        let body = json!({
            "tools": [
                {"name": "Bash", "input_schema": {}},
                {"type": "custom", "name": "Read", "input_schema": {}},
                {"type": "web_search_20250305", "name": "web_search"}
            ],
            "tool_choice": {"type": "tool", "name": "web_search"}
        });
        let f = function_tools(&body);
        assert_eq!(f.len(), 2);
        assert!(usable_tool_choice(&body, &f).is_none(), "指向被丢弃的内置工具");

        let only_search = json!({"tools": [{"type": "web_search_20250305", "name": "web_search"}], "tool_choice": {"type": "any"}});
        let f = function_tools(&only_search);
        assert!(f.is_empty());
        assert!(usable_tool_choice(&only_search, &f).is_none(), "一个函数都不剩就不发 tool_choice");

        let normal = json!({"tools": [{"name": "Bash"}], "tool_choice": {"type": "tool", "name": "Bash"}});
        let f = function_tools(&normal);
        assert_eq!(usable_tool_choice(&normal, &f), Some(&normal["tool_choice"]));

        let no_tools = json!({"tool_choice": "auto"});
        assert_eq!(usable_tool_choice(&no_tools, &[]), Some(&no_tools["tool_choice"]), "没有内置工具时行为不变");
    }
}
