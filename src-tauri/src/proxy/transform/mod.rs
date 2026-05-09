//! Anthropic Messages ↔ OpenAI Responses 协议翻译.
//!
//! 仅供 `auth_type=ChatgptOauth` 的订阅使用. 其他订阅走 cc-router 默认的
//! Anthropic 透传管线, 与本模块无关.
//!
//! 入口:
//! - [`openai_responses::anthropic_to_responses`] - 把 Anthropic Messages 请求体转成 OpenAI Responses
//! - [`openai_responses::ResponsesSseConverter`] - 流式状态机, 把 OpenAI Responses SSE 事件
//!   转成 Anthropic Messages SSE 事件 (chunk 进 / chunk 出)
//! - [`openai_responses::collect_to_anthropic_json`] - 非流式: 把所有 SSE 收完拼成 Anthropic 的 message 对象
//!   (因为 ChatGPT 后端强制 stream=true, 即便 Claude Code 要非流式也得这么转)

pub mod openai_responses;
