//! 网页管理界面 (/ui). 与代理 API 共用端口, 路径前缀分流.
//!
//! 子路由必须在 server.rs 的 auth_layer / cors_layer 之后 merge, 让 /ui 子树
//! 绕开代理 token 校验与 `Access-Control-Allow-Origin: *`——带凭据的管理 API
//! 绝不能配通配 CORS. 详见 docs/superpowers/specs/2026-09-06-web-ui-runtime-bridge-design.md
//!
//! 层次 (外→内): gate (web_ui_enabled 404, 或本机通行) → [api 子树: csrf 头 → 会话 (或本机通行)].

pub mod api;
pub mod assets;
pub mod auth;
pub mod events;
pub mod gate;
pub mod local_pass;

use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::{get, post};
use axum::Router;

use crate::state::AppState;

pub fn router(state: AppState) -> Router<AppState> {
    let api = Router::new()
        .route("/cmd/{name}", post(api::dispatch_handler))
        .route("/logout", post(auth::logout))
        .route("/events", get(events::sse_handler))
        .layer(from_fn_with_state(state.clone(), auth::require_session))
        .route("/login", post(auth::login))
        .route("/session", get(auth::session))
        .layer(from_fn(auth::require_csrf_header));

    Router::new()
        .route("/ui", get(assets::redirect_root))
        .route("/ui/", get(assets::serve_index))
        .route("/ui/{*path}", get(assets::serve_path))
        .nest("/ui/api", api)
        .layer(from_fn_with_state(state, gate::require_enabled))
}
