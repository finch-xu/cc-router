//! 协议翻译模块. 各子模块仅供特定 `auth_type` 订阅使用, 不影响 cc-router 默认的
//! Anthropic 透传管线.
//!
//! 子模块:
//! - [`openai_responses`] - Anthropic Messages ↔ OpenAI Responses (ChatGPT 反代). 用于 `auth_type=ChatgptOauth`.
//!   入口: `anthropic_to_responses`, `ResponsesSseConverter`, `collect_to_anthropic_json`.
//! - [`aws_event_stream`] - AWS Event Stream 二进制流解码器. 用于 Kiro/CodeWhisperer 响应解析.
//! - [`kiro_codewhisperer`] - Anthropic Messages ↔ AWS CodeWhisperer (Kiro IDE 后端). 用于 `auth_type=KiroOauth`.
//!   入口: `anthropic_to_codewhisperer`, `KiroSseConverter`, `NonStreamingCollector`.

pub mod openai_responses;
pub mod aws_event_stream;
pub mod kiro_codewhisperer;
