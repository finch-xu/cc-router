//! 外部客户端工具配置写入 commands.
//!
//! Claude Code (~/.claude/settings.json) + Codex CLI/Desktop (~/.codex/{config.toml, auth.json}).

use tauri::State;

use crate::error::AppResult;
use crate::integrations::claude_code::{self, InspectResult, ReadResult, WriteOutcome};
use crate::integrations::codex::{
    self, AuthInspectResult, ConfigInspectResult, ReadResult as CodexReadResult,
    WriteOutcome as CodexWriteOutcome,
};
use crate::state::AppState;

#[tauri::command]
pub async fn read_claude_code_settings(_state: State<'_, AppState>) -> AppResult<ReadResult> {
    claude_code::read().await
}

#[tauri::command]
pub async fn inspect_claude_code_settings(
    state: State<'_, AppState>,
) -> AppResult<InspectResult> {
    let base_url = state.local_base_url().await;
    let (token, auth_required) = {
        let g = state.settings.read().await;
        (g.auth_token.clone(), g.auth_enabled)
    };
    // auth_enabled=false 时 cc-router 不查 token, settings.json 里的 token 无所谓 — 不参与同步判定.
    claude_code::inspect(&base_url, token.as_str(), auth_required).await
}

#[tauri::command]
pub async fn write_claude_code_settings(
    state: State<'_, AppState>,
    new_content: String,
) -> AppResult<WriteOutcome> {
    let base_url = state.local_base_url().await;
    claude_code::write(&new_content, &base_url).await
}

// ===== Codex =====

#[tauri::command]
pub async fn read_codex_config(_state: State<'_, AppState>) -> AppResult<CodexReadResult> {
    codex::read_config().await
}

#[tauri::command]
pub async fn read_codex_auth(_state: State<'_, AppState>) -> AppResult<CodexReadResult> {
    codex::read_auth().await
}

#[tauri::command]
pub async fn inspect_codex_config(
    state: State<'_, AppState>,
) -> AppResult<ConfigInspectResult> {
    let base_url = state.local_base_url().await;
    codex::inspect_config(&base_url).await
}

#[tauri::command]
pub async fn inspect_codex_auth(state: State<'_, AppState>) -> AppResult<AuthInspectResult> {
    let (token, auth_required) = {
        let g = state.settings.read().await;
        (g.auth_token.clone(), g.auth_enabled)
    };
    codex::inspect_auth(token.as_str(), auth_required).await
}

#[tauri::command]
pub async fn write_codex_config(
    state: State<'_, AppState>,
    new_content: String,
) -> AppResult<CodexWriteOutcome> {
    let base_url = state.local_base_url().await;
    codex::write_config(&new_content, &base_url).await
}

#[tauri::command]
pub async fn write_codex_auth(
    _state: State<'_, AppState>,
    new_content: String,
) -> AppResult<CodexWriteOutcome> {
    codex::write_auth(&new_content).await
}
