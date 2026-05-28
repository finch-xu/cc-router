//! ~/.codex/config.toml + ~/.codex/auth.json 读写与同步状态探测.
//!
//! 设计要点:
//! - Codex CLI / Desktop 共用 ~/.codex/ 目录, 同一份配置.
//! - 走 cc-router 时, config.toml 注入 `[model_providers.cc-router]` (wire_api = "responses")
//!   + 一个 `[profiles.cc-router]`, 让用户用 `codex -p cc-router` 走代理.
//! - auth.json 写入 `OPENAI_API_KEY = <cc-router token>`. 旧文件若含 ChatGPT OAuth
//!   (`tokens.access_token`) 或其他 API key, 首次写入触发 `auth.json.cc-router.bak` 备份.
//! - `inspect_*` 只判定「是否已应用 cc-router」, 不强校验用户额外加的 profile / 字段.
//! - `write_*` 前端把整文件文本落盘, Rust 只做合法性兜底 + 备份判定 + atomic write.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tokio::fs;

use super::{atomic_write, home_dir, sibling_with_suffix};
use crate::error::{AppError, AppResult};

pub const BACKUP_SUFFIX: &str = ".cc-router.bak";
pub const TMP_SUFFIX: &str = ".cc-router.tmp";

const CC_ROUTER_PROVIDER_NAME: &str = "cc-router";
const CC_ROUTER_WIRE_API: &str = "responses";

pub fn config_path() -> AppResult<PathBuf> {
    Ok(config_path_in(&home_dir()?))
}

pub fn auth_path() -> AppResult<PathBuf> {
    Ok(auth_path_in(&home_dir()?))
}

pub fn config_path_in(home: &Path) -> PathBuf {
    home.join(".codex").join("config.toml")
}

pub fn auth_path_in(home: &Path) -> PathBuf {
    home.join(".codex").join("auth.json")
}

#[derive(Debug, Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum CodexSyncStatus {
    InSync,
    NeedsApply,
    NeverApplied,
    FileMissing,
    ParseError,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ReadResult {
    pub path: String,
    /// None = 文件不存在.
    pub content: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ConfigInspectResult {
    pub path: String,
    pub status: CodexSyncStatus,
    /// 当前 config.toml 里 cc-router provider 的 base_url, 用于 UI 展示「指向了什么」.
    pub current_base_url: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct AuthInspectResult {
    pub path: String,
    pub status: CodexSyncStatus,
    /// 检测到 ChatGPT OAuth 凭据 (tokens.access_token 存在). 写入会触发备份.
    pub has_chatgpt_oauth: bool,
    /// 当前 OPENAI_API_KEY 是否与 cc-router 当前 token 匹配 (auth_required=false 时恒 true).
    pub current_token_matches: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct WriteOutcome {
    pub path: String,
    /// Some = 触发备份; None = 未触发 / 已有备份.
    pub backup_path: Option<String>,
    pub bytes_written: usize,
}

// ===== Read =====

pub async fn read_config() -> AppResult<ReadResult> {
    read_text(&config_path_in(&home_dir()?)).await
}

pub async fn read_auth() -> AppResult<ReadResult> {
    read_text(&auth_path_in(&home_dir()?)).await
}

async fn read_text(path: &Path) -> AppResult<ReadResult> {
    let display = path.to_string_lossy().into_owned();
    match fs::read_to_string(path).await {
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

// ===== Inspect =====

pub async fn inspect_config(expected_base_url: &str) -> AppResult<ConfigInspectResult> {
    inspect_config_in(&home_dir()?, expected_base_url).await
}

pub async fn inspect_auth(
    expected_token: &str,
    auth_required: bool,
) -> AppResult<AuthInspectResult> {
    inspect_auth_in(&home_dir()?, expected_token, auth_required).await
}

pub async fn inspect_config_in(
    home: &Path,
    expected_base_url: &str,
) -> AppResult<ConfigInspectResult> {
    let path = config_path_in(home);
    let display = path.to_string_lossy().into_owned();
    let raw = match fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ConfigInspectResult {
                path: display,
                status: CodexSyncStatus::FileMissing,
                current_base_url: None,
            });
        }
        Err(e) => return Err(AppError::from(e)),
    };

    let doc = match raw.parse::<toml_edit::DocumentMut>() {
        Ok(d) => d,
        Err(_) => {
            return Ok(ConfigInspectResult {
                path: display,
                status: CodexSyncStatus::ParseError,
                current_base_url: None,
            });
        }
    };

    let snap = ConfigSnapshot::extract(&doc);
    let expected_full = format!("{expected_base_url}/v1");
    let status = match (
        snap.cc_router_provider_present,
        snap.cc_router_base_url.as_deref(),
        snap.wire_api.as_deref(),
    ) {
        (false, _, _) => CodexSyncStatus::NeverApplied,
        (true, Some(url), Some(wire))
            if url == expected_full && wire == CC_ROUTER_WIRE_API =>
        {
            CodexSyncStatus::InSync
        }
        _ => CodexSyncStatus::NeedsApply,
    };

    Ok(ConfigInspectResult {
        path: display,
        status,
        current_base_url: snap.cc_router_base_url,
    })
}

pub async fn inspect_auth_in(
    home: &Path,
    expected_token: &str,
    auth_required: bool,
) -> AppResult<AuthInspectResult> {
    let path = auth_path_in(home);
    let display = path.to_string_lossy().into_owned();
    let raw = match fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(AuthInspectResult {
                path: display,
                status: CodexSyncStatus::FileMissing,
                has_chatgpt_oauth: false,
                current_token_matches: false,
            });
        }
        Err(e) => return Err(AppError::from(e)),
    };

    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => {
            return Ok(AuthInspectResult {
                path: display,
                status: CodexSyncStatus::ParseError,
                has_chatgpt_oauth: false,
                current_token_matches: false,
            });
        }
    };

    let snap = AuthSnapshot::extract(&value);
    let token_ok = if auth_required {
        snap.openai_api_key.as_deref() == Some(expected_token)
    } else {
        true
    };
    let status = match (snap.openai_api_key.is_some(), token_ok) {
        (false, _) => CodexSyncStatus::NeverApplied,
        (true, true) => CodexSyncStatus::InSync,
        (true, false) => CodexSyncStatus::NeedsApply,
    };

    Ok(AuthInspectResult {
        path: display,
        status,
        has_chatgpt_oauth: snap.has_chatgpt_oauth,
        current_token_matches: token_ok,
    })
}

// ===== Write =====

pub async fn write_config(
    new_content: &str,
    cc_router_base_url: &str,
) -> AppResult<WriteOutcome> {
    write_config_in(&home_dir()?, new_content, cc_router_base_url).await
}

pub async fn write_auth(new_content: &str) -> AppResult<WriteOutcome> {
    write_auth_in(&home_dir()?, new_content).await
}

pub async fn write_config_in(
    home: &Path,
    new_content: &str,
    cc_router_base_url: &str,
) -> AppResult<WriteOutcome> {
    new_content
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| AppError::BadRequest(format!("config.toml 不是合法 TOML: {e}")))?;

    let path = config_path_in(home);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }

    let backup_path = maybe_backup_config(&path, cc_router_base_url).await?;
    atomic_write(&path, new_content.as_bytes(), TMP_SUFFIX).await?;

    Ok(WriteOutcome {
        path: path.to_string_lossy().into_owned(),
        backup_path: backup_path.map(|p| p.to_string_lossy().into_owned()),
        bytes_written: new_content.len(),
    })
}

pub async fn write_auth_in(home: &Path, new_content: &str) -> AppResult<WriteOutcome> {
    let new_value: serde_json::Value = serde_json::from_str(new_content)
        .map_err(|e| AppError::BadRequest(format!("auth.json 不是合法 JSON: {e}")))?;
    if !new_value.is_object() {
        return Err(AppError::BadRequest(
            "auth.json 顶层必须是 JSON Object".into(),
        ));
    }

    let path = auth_path_in(home);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }

    let backup_path = maybe_backup_auth(&path).await?;
    atomic_write(&path, new_content.as_bytes(), TMP_SUFFIX).await?;

    Ok(WriteOutcome {
        path: path.to_string_lossy().into_owned(),
        backup_path: backup_path.map(|p| p.to_string_lossy().into_owned()),
        bytes_written: new_content.len(),
    })
}

/// config.toml 备份判定:
/// 1. 已经是 cc-router 稳态 (provider 存在 + base_url 匹配) → 不备份
/// 2. 其他情况 (含非 cc-router provider / 旧 cc-router URL / 解析失败) → 备份
/// 3. .cc-router.bak 已存在 → 不重复 (保护最初的原始版本)
async fn maybe_backup_config(
    target: &Path,
    cc_router_base_url: &str,
) -> AppResult<Option<PathBuf>> {
    let old_raw = match fs::read_to_string(target).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(AppError::from(e)),
    };

    let needs_backup = match old_raw.parse::<toml_edit::DocumentMut>() {
        Ok(doc) => {
            let snap = ConfigSnapshot::extract(&doc);
            let expected_full = format!("{cc_router_base_url}/v1");
            !(snap.cc_router_provider_present
                && snap.cc_router_base_url.as_deref() == Some(&expected_full))
        }
        Err(_) => true,
    };
    if !needs_backup {
        return Ok(None);
    }
    create_backup_if_absent(target).await
}

/// auth.json 备份判定:
/// 1. 含 ChatGPT OAuth (tokens.access_token) → 备份
/// 2. OPENAI_API_KEY 非空 → 备份 (可能是用户原 sk- 或上一轮 cc-router token)
/// 3. .cc-router.bak 已存在 → 不重复
async fn maybe_backup_auth(target: &Path) -> AppResult<Option<PathBuf>> {
    let old_raw = match fs::read_to_string(target).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(AppError::from(e)),
    };

    let needs_backup = match serde_json::from_str::<serde_json::Value>(&old_raw) {
        Ok(v) => {
            let snap = AuthSnapshot::extract(&v);
            snap.has_chatgpt_oauth
                || snap
                    .openai_api_key
                    .as_deref()
                    .map(|k| !k.is_empty())
                    .unwrap_or(false)
        }
        Err(_) => true,
    };
    if !needs_backup {
        return Ok(None);
    }
    create_backup_if_absent(target).await
}

async fn create_backup_if_absent(target: &Path) -> AppResult<Option<PathBuf>> {
    let backup = sibling_with_suffix(target, BACKUP_SUFFIX);
    if fs::try_exists(&backup).await.unwrap_or(false) {
        return Ok(None);
    }
    fs::copy(target, &backup).await?;
    Ok(Some(backup))
}

// ===== Snapshot helpers =====

struct ConfigSnapshot {
    cc_router_provider_present: bool,
    cc_router_base_url: Option<String>,
    wire_api: Option<String>,
}

impl ConfigSnapshot {
    fn extract(doc: &toml_edit::DocumentMut) -> Self {
        let provider = doc
            .get("model_providers")
            .and_then(|v| v.as_table_like())
            .and_then(|t| t.get(CC_ROUTER_PROVIDER_NAME))
            .and_then(|v| v.as_table_like());

        let base_url = provider
            .and_then(|t| t.get("base_url"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let wire_api = provider
            .and_then(|t| t.get("wire_api"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Self {
            cc_router_provider_present: provider.is_some(),
            cc_router_base_url: base_url,
            wire_api,
        }
    }
}

struct AuthSnapshot {
    openai_api_key: Option<String>,
    has_chatgpt_oauth: bool,
}

impl AuthSnapshot {
    fn extract(value: &serde_json::Value) -> Self {
        let obj = value.as_object();
        let openai_api_key = obj
            .and_then(|o| o.get("OPENAI_API_KEY"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let has_chatgpt_oauth = obj
            .and_then(|o| o.get("tokens"))
            .and_then(|v| v.as_object())
            .and_then(|o| o.get("access_token"))
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        Self {
            openai_api_key,
            has_chatgpt_oauth,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const CC_ROUTER_URL: &str = "http://127.0.0.1:23456";

    fn recommended_config() -> String {
        format!(
            r#"# cc-router 推荐配置
[model_providers.cc-router]
name = "cc-router"
base_url = "{CC_ROUTER_URL}/v1"
wire_api = "responses"
env_key = "OPENAI_API_KEY"

[profiles.cc-router]
model_provider = "cc-router"
model = "model-sonnet"
"#
        )
    }

    fn recommended_auth(token: &str) -> String {
        format!("{{\n  \"OPENAI_API_KEY\": \"{token}\"\n}}\n")
    }

    #[tokio::test]
    async fn path_helpers_compose_codex_dir() {
        let dir = tempdir().unwrap();
        let cfg = config_path_in(dir.path());
        let auth = auth_path_in(dir.path());
        // Path::ends_with 按 component 比较, 跨平台 `\` 和 `/` 都会被解析为 separator,
        // 所以单一字符串字面量在 Windows 上同样匹配 — 不需要 OR 分支.
        assert!(cfg.ends_with(".codex/config.toml"));
        assert!(auth.ends_with(".codex/auth.json"));
    }

    #[tokio::test]
    async fn inspect_config_file_missing() {
        let dir = tempdir().unwrap();
        let r = inspect_config_in(dir.path(), CC_ROUTER_URL).await.unwrap();
        assert_eq!(r.status, CodexSyncStatus::FileMissing);
    }

    #[tokio::test]
    async fn inspect_config_never_applied() {
        let dir = tempdir().unwrap();
        let path = config_path_in(dir.path());
        fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        fs::write(&path, "# only a comment\n").await.unwrap();
        let r = inspect_config_in(dir.path(), CC_ROUTER_URL).await.unwrap();
        assert_eq!(r.status, CodexSyncStatus::NeverApplied);
    }

    #[tokio::test]
    async fn inspect_config_in_sync_then_needs_apply_on_url_change() {
        let dir = tempdir().unwrap();
        let path = config_path_in(dir.path());
        fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        fs::write(&path, recommended_config()).await.unwrap();

        let r = inspect_config_in(dir.path(), CC_ROUTER_URL).await.unwrap();
        assert_eq!(r.status, CodexSyncStatus::InSync);
        assert_eq!(
            r.current_base_url.as_deref(),
            Some("http://127.0.0.1:23456/v1")
        );

        // 模拟用户改了端口, cc-router 现在跑在 33333.
        let r2 = inspect_config_in(dir.path(), "http://127.0.0.1:33333")
            .await
            .unwrap();
        assert_eq!(r2.status, CodexSyncStatus::NeedsApply);
    }

    #[tokio::test]
    async fn inspect_config_parse_error() {
        let dir = tempdir().unwrap();
        let path = config_path_in(dir.path());
        fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        fs::write(&path, "this is = not [valid toml").await.unwrap();
        let r = inspect_config_in(dir.path(), CC_ROUTER_URL).await.unwrap();
        assert_eq!(r.status, CodexSyncStatus::ParseError);
    }

    #[tokio::test]
    async fn inspect_auth_detects_chatgpt_oauth_without_api_key() {
        let dir = tempdir().unwrap();
        let path = auth_path_in(dir.path());
        fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        let oauth = r#"{ "tokens": { "access_token": "AT_xxx", "refresh_token": "RT_yyy" } }"#;
        fs::write(&path, oauth).await.unwrap();
        let r = inspect_auth_in(dir.path(), "expected-token", true)
            .await
            .unwrap();
        assert!(r.has_chatgpt_oauth);
        assert_eq!(r.status, CodexSyncStatus::NeverApplied);
    }

    #[tokio::test]
    async fn inspect_auth_token_match_logic() {
        let dir = tempdir().unwrap();
        let path = auth_path_in(dir.path());
        fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        fs::write(&path, recommended_auth("expected-token"))
            .await
            .unwrap();

        let r = inspect_auth_in(dir.path(), "expected-token", true)
            .await
            .unwrap();
        assert_eq!(r.status, CodexSyncStatus::InSync);

        let r2 = inspect_auth_in(dir.path(), "different-token", true)
            .await
            .unwrap();
        assert_eq!(r2.status, CodexSyncStatus::NeedsApply);

        // auth_required=false 时即使 token 不一致也算 in_sync — cc-router 关鉴权.
        let r3 = inspect_auth_in(dir.path(), "different-token", false)
            .await
            .unwrap();
        assert_eq!(r3.status, CodexSyncStatus::InSync);
    }

    #[tokio::test]
    async fn write_config_backups_user_file_once() {
        let dir = tempdir().unwrap();
        let path = config_path_in(dir.path());
        fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        fs::write(
            &path,
            "[model_providers.foo]\nbase_url = \"https://api.foo.com/v1\"\n",
        )
        .await
        .unwrap();

        let r1 = write_config_in(dir.path(), &recommended_config(), CC_ROUTER_URL)
            .await
            .unwrap();
        assert!(r1.backup_path.is_some(), "首次写入应触发备份");
        let backup = PathBuf::from(r1.backup_path.unwrap());
        assert!(fs::try_exists(&backup).await.unwrap());

        let r2 = write_config_in(dir.path(), &recommended_config(), CC_ROUTER_URL)
            .await
            .unwrap();
        assert!(
            r2.backup_path.is_none(),
            "已经是 cc-router 稳态, 不应再备份"
        );
    }

    #[tokio::test]
    async fn write_auth_backups_oauth_then_only_once() {
        let dir = tempdir().unwrap();
        let path = auth_path_in(dir.path());
        fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        fs::write(&path, r#"{ "tokens": { "access_token": "AT_xxx" } }"#)
            .await
            .unwrap();

        let r1 = write_auth_in(dir.path(), &recommended_auth("t1"))
            .await
            .unwrap();
        assert!(r1.backup_path.is_some(), "OAuth 旧文件必须备份");

        // 二次写入: 旧文件已是 API key 结构, 仍判定 needs_backup, 但 .bak 已存在 → None.
        let r2 = write_auth_in(dir.path(), &recommended_auth("t2"))
            .await
            .unwrap();
        assert!(r2.backup_path.is_none());
    }

    #[tokio::test]
    async fn write_config_rejects_invalid_toml() {
        let dir = tempdir().unwrap();
        let err = write_config_in(dir.path(), "this is = not [valid", CC_ROUTER_URL).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn write_auth_rejects_non_object_json() {
        let dir = tempdir().unwrap();
        let err = write_auth_in(dir.path(), "\"just a string\"").await;
        assert!(err.is_err());
    }
}
