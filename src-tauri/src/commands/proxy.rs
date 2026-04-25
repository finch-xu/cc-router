use serde::Serialize;
use tauri::State;

use crate::error::AppResult;
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct ProxyStatus {
    pub port: u16,
    pub running: bool,
}

#[tauri::command]
pub async fn proxy_status(state: State<'_, AppState>) -> AppResult<ProxyStatus> {
    let port = *state.proxy_port.read().await;
    Ok(ProxyStatus {
        port,
        running: port != 0,
    })
}

#[tauri::command]
pub async fn env_snippet(state: State<'_, AppState>) -> AppResult<String> {
    let port = *state.proxy_port.read().await;
    let token = state.settings.read().await.auth_token.clone();
    Ok(format!(
        "export ANTHROPIC_BASE_URL=http://127.0.0.1:{port}\n\
         export ANTHROPIC_AUTH_TOKEN={token}\n\
         export API_TIMEOUT_MS=3000000\n\
         export CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1\n\
         export ANTHROPIC_MODEL=model-opus\n\
         export ANTHROPIC_DEFAULT_OPUS_MODEL=model-opus\n\
         export ANTHROPIC_DEFAULT_SONNET_MODEL=model-sonnet\n\
         export ANTHROPIC_DEFAULT_HAIKU_MODEL=model-haiku"
    ))
}
