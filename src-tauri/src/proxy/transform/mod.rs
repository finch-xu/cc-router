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

pub mod responses_common;
pub mod responses_inbound;
pub mod openai_responses;
pub mod openai;
pub mod aws_event_stream;
pub mod kiro_codewhisperer;
pub mod gemini;
pub mod gemini_interactions;
pub mod openai_chat_completions;
