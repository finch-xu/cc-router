use serde::Serialize;
use tauri::State;

use crate::error::AppResult;
use crate::settings::model::ProxyMode;
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct ProxyStatus {
    /// 兼容老前端字段: HTTP 端口 (HTTPS-only 模式下回退到 HTTPS 端口, 保留单值入口).
    pub port: u16,
    pub running: bool,
    pub mode: ProxyMode,
    /// HTTP listener 实际绑定端口, None=HTTP 未启用 (HTTPS-only 模式).
    pub http_port: Option<u16>,
    /// HTTPS listener 实际绑定端口, None=HTTPS 未启用 (HTTP-only 模式).
    pub https_port: Option<u16>,
    /// true: 监听 0.0.0.0; false: 仅 127.0.0.1.
    pub listen_all: bool,
}

#[tauri::command]
pub async fn proxy_status(state: State<'_, AppState>) -> AppResult<ProxyStatus> {
    let http_port = *state.http_bound_port.read().await;
    let https_port = *state.https_bound_port.read().await;
    let (mode, listen_all) = {
        let g = state.settings.read().await;
        (g.proxy_mode, g.listen_all)
    };
    let primary = http_port.or(https_port).unwrap_or(0);
    Ok(ProxyStatus {
        port: primary,
        running: primary != 0,
        mode,
        http_port,
        https_port,
        listen_all,
    })
}

#[tauri::command]
pub async fn env_snippet(state: State<'_, AppState>) -> AppResult<String> {
    let http_port = *state.http_bound_port.read().await;
    let https_port = *state.https_bound_port.read().await;
    let token = state.settings.read().await.auth_token.clone();
    // 优先用 HTTP 端口 (更通用); 仅 HTTPS 时给 https:// URL (用户需先导入 CA).
    let base_url = if let Some(p) = http_port {
        format!("http://127.0.0.1:{p}")
    } else if let Some(p) = https_port {
        format!("https://127.0.0.1:{p}")
    } else {
        "http://127.0.0.1:23456".to_string()
    };
    Ok(format!(
        "export ANTHROPIC_BASE_URL={base_url}\n\
         export ANTHROPIC_AUTH_TOKEN={token}\n\
         export API_TIMEOUT_MS=3000000\n\
         export ANTHROPIC_MODEL=model-opus\n\
         export ANTHROPIC_DEFAULT_OPUS_MODEL=model-opus\n\
         export ANTHROPIC_DEFAULT_SONNET_MODEL=model-sonnet\n\
         export ANTHROPIC_DEFAULT_HAIKU_MODEL=model-haiku\n\
         export CLAUDE_CODE_SUBAGENT_MODEL=model-opus\n\
         export CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1\n\
         export CLAUDE_CODE_DISABLE_NONSTREAMING_FALLBACK=1\n\
         export CLAUDE_CODE_ATTRIBUTION_HEADER=0\n\
         export CLAUDE_CODE_EFFORT_LEVEL=max"
    ))
}
