use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use tracing::{error, info};

use crate::proxy::pipeline;
use crate::state::AppState;

pub async fn health() -> &'static str {
    "ok"
}

/// POST /v1/messages
/// 入口：把 Claude Code 的请求解析成 UnifiedRequest 后交给 pipeline。
pub async fn messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(error_body("invalid_request_error", &format!("JSON 解析失败: {e}"))),
            )
                .into_response();
        }
    };

    let model = match parsed.get("model").and_then(|v| v.as_str()) {
        Some(m) => m.to_string(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(error_body("invalid_request_error", "缺少 model 字段")),
            )
                .into_response();
        }
    };

    let is_streaming = parsed
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    info!(%model, is_streaming, "proxy received request");

    match pipeline::dispatch(&state, &model, parsed, headers, is_streaming).await {
        Ok(resp) => resp,
        Err(e) => {
            error!(?e, "pipeline dispatch failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(error_body("api_error", &e.to_string())),
            )
                .into_response()
        }
    }
}

pub fn error_body(kind: &str, message: &str) -> serde_json::Value {
    json!({
        "type": "error",
        "error": {
            "type": kind,
            "message": message,
        }
    })
}
