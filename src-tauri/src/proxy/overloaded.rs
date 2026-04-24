//! §5.3 全部订阅不可用时的 503 响应。

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::virtual_model::VirtualModelName;

pub fn response(vm: VirtualModelName, summary: &[String]) -> Response {
    response_with_summary(vm, summary)
}

pub fn response_with_summary(vm: VirtualModelName, summary: &[String]) -> Response {
    let detail = if summary.is_empty() {
        "未绑定任何订阅".to_string()
    } else {
        summary.join("\n")
    };
    let msg = format!(
        "All subscriptions for {} are unavailable.\nDetails:\n{}",
        vm.as_str(),
        detail
    );
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "type": "error",
            "error": {
                "type": "overloaded_error",
                "message": msg,
            }
        })),
    )
        .into_response()
}
