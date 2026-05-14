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
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                &format!("JSON 解析失败: {e}"),
            );
        }
    };

    let model = match parsed.get("model").and_then(|v| v.as_str()) {
        Some(m) => m.to_string(),
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "缺少 model 字段",
            );
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
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "api_error", &e.to_string())
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

pub fn error_response(status: StatusCode, kind: &str, message: &str) -> Response {
    (status, Json(error_body(kind, message))).into_response()
}

/// GET /v1/models
/// 返回 cc-router 对外暴露的固定模型清单, 无鉴权 (与 /health 同级在 auth_layer 直通).
/// schema 对齐 Anthropic 官方 /v1/models 响应。
pub async fn models() -> Response {
    const MODEL_IDS: &[&str] = &[
        "model-opus",
        "model-sonnet",
        "model-haiku",
        "claude-opus-4-7",
        "claude-sonnet-4-6",
        "claude-haiku-4-5",
        "anthropic/claude-opus-4-7",
        "anthropic/claude-sonnet-4-6",
        "anthropic/claude-haiku-4-5",
        "anthropic/model-opus",
        "anthropic/model-sonnet",
        "anthropic/model-haiku",
    ];
    const CREATED_AT: &str = "2026-01-01T00:00:00Z";

    let data: Vec<serde_json::Value> = MODEL_IDS
        .iter()
        .map(|id| {
            json!({
                "type": "model",
                "id": id,
                "display_name": id,
                "created_at": CREATED_AT,
            })
        })
        .collect();

    Json(json!({
        "data": data,
        "has_more": false,
        "first_id": MODEL_IDS.first(),
        "last_id": MODEL_IDS.last(),
    }))
    .into_response()
}
