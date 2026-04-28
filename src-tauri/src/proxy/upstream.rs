use axum::http::StatusCode;
use bytes::Bytes;
use futures::stream::BoxStream;
use reqwest::header::HeaderMap as ReqHeaderMap;
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum UpstreamError {
    #[error("reqwest: {0}")]
    Reqwest(#[from] reqwest::Error),
}

pub enum UpstreamResponse {
    NonStreaming {
        status: StatusCode,
        headers: ReqHeaderMap,
        body: Value,
        /// 原始响应文本。仅在非 2xx 时填充, 用于错误路径诊断展示;
        /// 成功路径不保留, 避免每个响应多 hold 一份 KB 级 String。
        body_text: Option<String>,
    },
    Streaming {
        status: StatusCode,
        headers: ReqHeaderMap,
        stream: BoxStream<'static, Result<Bytes, reqwest::Error>>,
    },
}

pub async fn send(
    client: &reqwest::Client,
    url: &str,
    body: Vec<u8>,
    headers: ReqHeaderMap,
    is_streaming: bool,
) -> Result<UpstreamResponse, UpstreamError> {
    let resp = client
        .post(url)
        .headers(headers)
        .body(body)
        .send()
        .await?;

    let status = StatusCode::from_u16(resp.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let resp_headers = resp.headers().clone();

    if is_streaming && status.is_success() {
        Ok(UpstreamResponse::Streaming {
            status,
            headers: resp_headers,
            stream: Box::pin(resp.bytes_stream()),
        })
    } else {
        // 非流式：完整读取
        let text = resp.text().await?;
        let body = serde_json::from_str::<Value>(&text)
            .unwrap_or_else(|_| serde_json::json!({ "raw": text.clone() }));
        let body_text = if status.is_success() { None } else { Some(text) };
        Ok(UpstreamResponse::NonStreaming {
            status,
            headers: resp_headers,
            body,
            body_text,
        })
    }
}
