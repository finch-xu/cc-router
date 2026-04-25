//! 代理服务器的两个轴向中间件:
//!
//! - `cors_layer`: 给响应附加 CORS 头,默认允许 `*`;OPTIONS 预检直接返 204。
//! - `auth_layer`: 校验请求携带的 token(从 `x-api-key` 或 `Authorization: Bearer` 提取),
//!   匹配 `settings.auth_token` 才放行;`/health` 与 OPTIONS 直通。
//!
//! 两个中间件都是「每请求即时读 settings」,改 settings 无需重启 app。
//! 关于 layer 顺序:在 server.rs 里 cors 挂在外层 → auth 在内层。这样 401 错误响应也带 CORS 头,
//! 浏览器 fetch 才能拿到错误 body,而不是因 CORS 失败被吞掉。

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::proxy::handler;
use crate::state::AppState;

pub async fn cors_layer(State(state): State<AppState>, req: Request<Body>, next: Next) -> Response {
    let s = state.settings.read().await.clone();
    let method = req.method().clone();

    if method == Method::OPTIONS {
        let mut resp = (StatusCode::NO_CONTENT, ()).into_response();
        if s.cors_enabled {
            apply_cors_headers(resp.headers_mut(), &s.cors_allow_origin);
        }
        return resp;
    }

    let mut resp = next.run(req).await;
    if s.cors_enabled {
        apply_cors_headers(resp.headers_mut(), &s.cors_allow_origin);
    }
    resp
}

pub async fn auth_layer(State(state): State<AppState>, req: Request<Body>, next: Next) -> Response {
    if req.method() == Method::OPTIONS || req.uri().path() == "/health" {
        return next.run(req).await;
    }

    let s = state.settings.read().await.clone();
    if !s.auth_enabled {
        return next.run(req).await;
    }

    let provided = extract_token(req.headers());
    if provided.as_deref() == Some(s.auth_token.as_str()) {
        next.run(req).await
    } else {
        handler::error_response(
            StatusCode::UNAUTHORIZED,
            "authentication_error",
            "missing or invalid cc-router token. set ANTHROPIC_API_KEY=<token> or Authorization: Bearer <token>",
        )
    }
}

/// 从两类 header 中尝试提取 token,任一存在即返回。
/// 优先 `x-api-key`(CC 默认 ANTHROPIC_API_KEY env 走这里),
/// 其次 `Authorization: Bearer <token>`(CC 的 ANTHROPIC_AUTH_TOKEN env 走这里)。
fn extract_token(headers: &HeaderMap) -> Option<String> {
    if let Some(v) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        if !v.is_empty() {
            return Some(v.to_string());
        }
    }
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ").map(String::from))
        .filter(|s| !s.is_empty())
}

fn apply_cors_headers(h: &mut HeaderMap, origin: &str) {
    if let Ok(v) = origin.parse() {
        h.insert("Access-Control-Allow-Origin", v);
    }
    h.insert(
        "Access-Control-Allow-Methods",
        "GET, POST, OPTIONS".parse().unwrap(),
    );
    h.insert("Access-Control-Allow-Headers", "*".parse().unwrap());
    h.insert("Access-Control-Max-Age", "86400".parse().unwrap());
    // 暴露 anthropic-version / 错误体的 content-type 等,方便浏览器调试
    h.insert(
        "Access-Control-Expose-Headers",
        "*".parse().unwrap(),
    );
}
