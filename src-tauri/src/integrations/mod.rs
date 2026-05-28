//! 与外部客户端工具 (Claude Code / Codex CLI 等) 的配置文件集成.
//!
//! 每个子模块独立处理一个工具的 settings 文件: 读 / 状态探测 / 智能写入.
//! Phase 1 仅含 Claude Code; Codex CLI 在 Phase 2 加入.

pub mod claude_code;
