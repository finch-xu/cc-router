//! `/ui` 子树总闸. `web_ui_enabled` 关闭时整个子树返回纯 404, 对外看起来像不存在;
//! 例外是 cc-router-tui 的「本机通行」(见 local_pass.rs), 它只对 /ui/api/ 生效, 不看 web_ui_enabled.
//! 每请求读 settings, 与 proxy/middleware.rs 同一模式, 开关不需要重启.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::proxy::web::local_pass;
use crate::state::AppState;

pub fn gate_status(web_ui_enabled: bool, local_pass: bool) -> Option<StatusCode> {
    if web_ui_enabled || local_pass {
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
    let (web_ui_enabled, tui_enabled) = {
        let s = state.settings.read().await;
        (s.web_ui_enabled, s.tui_enabled)
    };
    // 本层挂在外层 Router 上, 看到的是完整路径 (nest 还没剥前缀)。
    let is_api_path = req.uri().path().starts_with(local_pass::API_PREFIX);
    let local = local_pass::from_request(&req, &state.local_secret, tui_enabled, is_api_path);
    match gate_status(web_ui_enabled, local) {
        Some(status) => status.into_response(),
        None => next.run(req).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_ui_switch_alone_behaves_as_before() {
        assert_eq!(gate_status(false, false), Some(StatusCode::NOT_FOUND));
        assert_eq!(gate_status(true, false), None);
    }

    /// TUI 不要求打开网页界面: 本机通行单独就能过总闸。
    #[test]
    fn local_pass_opens_the_gate_without_web_ui() {
        assert_eq!(gate_status(false, true), None);
        assert_eq!(gate_status(true, true), None);
    }
}
