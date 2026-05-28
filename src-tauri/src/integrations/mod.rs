//! 与外部客户端工具 (Claude Code / Codex CLI 等) 的配置文件集成.
//!
//! 每个子模块独立处理一个工具的 settings 文件: 读 / 状态探测 / 智能写入.

use std::path::{Path, PathBuf};

use tokio::fs;

use crate::error::{AppError, AppResult};

pub mod claude_code;
pub mod codex;

/// 解析用户主目录: Unix 用 HOME, Windows 用 USERPROFILE.
/// 共享给所有 client-tool 集成 (CC / Codex / 未来的 Ollama 等), 避免每个 module 复制一份.
pub fn home_dir() -> AppResult<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .ok_or_else(|| AppError::internal("无法解析用户主目录: HOME/USERPROFILE 均未设置"))
}

/// 在 target 同目录下生成一个同名 + suffix 的兄弟路径 (用于 .tmp / .bak 等).
/// target 必须有 file_name, 否则 fallback 到 "file" + suffix.
pub fn sibling_with_suffix(target: &Path, suffix: &str) -> PathBuf {
    let mut p = target.to_path_buf();
    let file_name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    p.set_file_name(format!("{file_name}{suffix}"));
    p
}

/// Atomic 写入: 同目录 .tmp + rename. 跨平台 (NTFS/APFS/ext4) 均原子.
/// `tmp_suffix` 由调用者指定 (CC 用 `.cc-router.tmp`, Codex 也用同名), 同 target 不同 tmp 不冲突
/// (因为基于 target file_name 拼出来, 不同文件得到不同 tmp 名).
pub async fn atomic_write(target: &Path, bytes: &[u8], tmp_suffix: &str) -> AppResult<()> {
    let tmp = sibling_with_suffix(target, tmp_suffix);
    fs::write(&tmp, bytes).await?;
    fs::rename(&tmp, target).await?;
    Ok(())
}
