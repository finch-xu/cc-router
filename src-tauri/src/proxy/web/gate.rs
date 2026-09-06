//! `web_ui_enabled` 总开关. 关闭时整个 /ui 子树返回纯 404, 对外看起来像不存在.
//! 每请求读 settings, 与 proxy/middleware.rs 同一模式, 开关不需要重启.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::state::AppState;

pub fn gate_status(enabled: bool) -> Option<StatusCode> {
    if enabled {
        None
    } else {
        Some(StatusCode::NOT_FOUND)
    }
}

pub async fn require_enabled(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let enabled = state.settings.read().await.web_ui_enabled;
    match gate_status(enabled) {
        Some(status) => status.into_response(),
        None => next.run(req).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_is_404_enabled_passes() {
        assert_eq!(gate_status(false), Some(StatusCode::NOT_FOUND));
        assert_eq!(gate_status(true), None);
    }
}
