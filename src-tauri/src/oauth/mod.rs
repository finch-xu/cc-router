//! OAuth 凭据管理.
//!
//! 目前仅支持 ChatGPT Plus/Pro (`chatgpt`) 一种 OAuth 来源, 用于 OpenAI Codex provider.
//! 未来若要再接入 Google OAuth / GitHub Copilot 等, 在此目录新增子模块.

pub mod chatgpt;
pub mod chatgpt_models;
pub mod kiro;
