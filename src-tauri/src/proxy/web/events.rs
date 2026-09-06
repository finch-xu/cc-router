//! Tauri emit → broadcast → SSE 桥. 本文件在 Task 6 补全 handler.

/// 一条转投给网页端的事件. payload 是 `app.emit` 时序列化后的 JSON 文本.
#[derive(Debug, Clone)]
pub struct UiEvent {
    pub name: String,
    pub payload: String,
}

/// broadcast 容量. 网页端消费慢时旧事件被覆盖 (RecvError::Lagged), SSE 侧跳过继续.
pub const UI_EVENT_CAPACITY: usize = 256;
