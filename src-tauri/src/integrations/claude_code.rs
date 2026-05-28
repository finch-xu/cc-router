//! ~/.claude/settings.json 读写与同步状态探测.
//!
//! 设计要点:
//! - `read` 仅返回原文件文本, 不解析 — UI 用 CodeMirror 自己 parse, 避免双方语法判定漂移.
//! - `inspect` 解析后探测 5 核心字段 (ANTHROPIC_BASE_URL / AUTH_TOKEN / 3 个 DEFAULT_*_MODEL)
//!   是否与 cc-router 当前值一致, 给 UI 状态徽章用. cc-router 关闭鉴权时 token 不参与判定.
//! - `write` 把前端传入的整文件文本原样落盘 — merge 算法在前端做 (前端能看到结果, 用户透明).
//!   Rust 端只负责: JSON 合法性兜底校验 + 备份判定 + atomic write.
//!
//! 备份触发条件: 旧文件 `env.ANTHROPIC_BASE_URL` 存在且不等于 cc-router 地址,
//! 且 `settings.json.cc-router.bak` 不存在 → 首次切换时备份一次.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tokio::fs;

use super::{atomic_write, home_dir, sibling_with_suffix};
use crate::error::{AppError, AppResult};

pub const BACKUP_SUFFIX: &str = ".cc-router.bak";
pub const TMP_SUFFIX: &str = ".cc-router.tmp";

/// 解析 `~/.claude/settings.json` 路径. 测试用 [`settings_path_in`] 注入 home.
pub fn settings_path() -> AppResult<PathBuf> {
    Ok(settings_path_in(&home_dir()?))
}

pub fn settings_path_in(home: &Path) -> PathBuf {
    home.join(".claude").join("settings.json")
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ReadResult {
    pub path: String,
    /// None = 文件不存在 (前端把编辑器初始化为空对象 `{}`).
    pub content: Option<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum SyncStatus {
    /// 5 个核心字段全部存在且 BASE_URL/TOKEN 与 cc-router 当前值一致.
    InSync,
    /// 5 核心字段都在但 BASE_URL/TOKEN 至少一项不一致 (典型: token 被重新生成).
    NeedsApply,
    /// 5 核心字段一个都没.
    NeverApplied,
    /// settings.json 不存在.
    FileMissing,
    /// 文件存在但 JSON 解析失败.
    ParseError,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct InspectResult {
    pub path: String,
    pub status: SyncStatus,
    pub current_base_url: Option<String>,
    pub current_token_matches: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct WriteOutcome {
    pub path: String,
    /// Some = 触发了备份; None = 未触发或已有备份.
    pub backup_path: Option<String>,
    pub bytes_written: usize,
}

pub async fn read() -> AppResult<ReadResult> {
    read_in(&home_dir()?).await
}

pub async fn inspect(
    expected_base_url: &str,
    expected_token: &str,
    auth_required: bool,
) -> AppResult<InspectResult> {
    inspect_in(&home_dir()?, expected_base_url, expected_token, auth_required).await
}

pub async fn write(new_content: &str, cc_router_base_url: &str) -> AppResult<WriteOutcome> {
    write_in(&home_dir()?, new_content, cc_router_base_url).await
}

pub async fn read_in(home: &Path) -> AppResult<ReadResult> {
    let path = settings_path_in(home);
    let display = path.to_string_lossy().into_owned();
    match fs::read_to_string(&path).await {
        Ok(content) => Ok(ReadResult {
            path: display,
            content: Some(content),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ReadResult {
            path: display,
            content: None,
        }),
        Err(e) => Err(AppError::from(e)),
    }
}

/// 从 settings.json 内容里抽出 env 段的 `ANTHROPIC_BASE_URL` / `ANTHROPIC_AUTH_TOKEN`.
/// 给 inspect 与 maybe_backup 共用, 避免两处对 env 路径的解析逻辑漂移.
struct AnthropicEnvSnapshot {
    base_url: Option<String>,
    token: Option<String>,
    /// 5 个核心 ANTHROPIC_* 字段中存在的数量, 用于推断 SyncStatus.
    core_present_count: u8,
}

const CORE_ENV_KEYS: [&str; 5] = [
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
];

fn parse_snapshot(raw: &str) -> Option<AnthropicEnvSnapshot> {
    let root: serde_json::Value = serde_json::from_str(raw).ok()?;
    let env = root.get("env").and_then(|v| v.as_object());
    let str_field = |k: &str| {
        env.and_then(|e| e.get(k))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    };
    let core_present_count = CORE_ENV_KEYS
        .iter()
        .filter(|k| env.and_then(|e| e.get(**k)).is_some())
        .count() as u8;
    Some(AnthropicEnvSnapshot {
        base_url: str_field("ANTHROPIC_BASE_URL"),
        token: str_field("ANTHROPIC_AUTH_TOKEN"),
        core_present_count,
    })
}

pub async fn inspect_in(
    home: &Path,
    expected_base_url: &str,
    expected_token: &str,
    auth_required: bool,
) -> AppResult<InspectResult> {
    let path = settings_path_in(home);
    let display = path.to_string_lossy().into_owned();
    let raw = match fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(InspectResult {
                path: display,
                status: SyncStatus::FileMissing,
                current_base_url: None,
                current_token_matches: false,
            });
        }
        Err(e) => return Err(AppError::from(e)),
    };

    let Some(snap) = parse_snapshot(&raw) else {
        return Ok(InspectResult {
            path: display,
            status: SyncStatus::ParseError,
            current_base_url: None,
            current_token_matches: false,
        });
    };

    let token_ok = if auth_required {
        snap.token.as_deref() == Some(expected_token)
    } else {
        // 关闭鉴权时不限制客户端 token 内容.
        true
    };
    let base_url_ok = snap.base_url.as_deref() == Some(expected_base_url);

    let status = match (snap.core_present_count, base_url_ok && token_ok) {
        (0, _) => SyncStatus::NeverApplied,
        (5, true) => SyncStatus::InSync,
        _ => SyncStatus::NeedsApply,
    };

    Ok(InspectResult {
        path: display,
        status,
        current_base_url: snap.base_url,
        current_token_matches: token_ok,
    })
}

pub async fn write_in(
    home: &Path,
    new_content: &str,
    cc_router_base_url: &str,
) -> AppResult<WriteOutcome> {
    let new_value: serde_json::Value = serde_json::from_str(new_content)
        .map_err(|e| AppError::BadRequest(format!("settings.json 不是合法 JSON: {e}")))?;
    if !new_value.is_object() {
        return Err(AppError::BadRequest(
            "settings.json 顶层必须是 JSON Object".into(),
        ));
    }

    let path = settings_path_in(home);
    let display = path.to_string_lossy().into_owned();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }

    let backup_path = maybe_backup(&path, cc_router_base_url).await?;

    let bytes = new_content.as_bytes();
    atomic_write(&path, bytes, TMP_SUFFIX).await?;

    Ok(WriteOutcome {
        path: display,
        backup_path: backup_path.map(|p| p.to_string_lossy().into_owned()),
        bytes_written: bytes.len(),
    })
}

async fn maybe_backup(target: &Path, cc_router_base_url: &str) -> AppResult<Option<PathBuf>> {
    let old_raw = match fs::read_to_string(target).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(AppError::from(e)),
    };
    let Some(snap) = parse_snapshot(&old_raw) else {
        return Ok(None);
    };
    let needs_backup = matches!(snap.base_url.as_deref(), Some(url) if url != cc_router_base_url);
    if !needs_backup {
        return Ok(None);
    }

    let backup = sibling_with_suffix(target, BACKUP_SUFFIX);
    if fs::try_exists(&backup).await.unwrap_or(false) {
        return Ok(None);
    }
    fs::copy(target, &backup).await?;
    Ok(Some(backup))
}
