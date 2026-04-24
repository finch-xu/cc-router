use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("sqlx 错误: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("YAML 解析错误: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("JSON 错误: {0}")]
    Json(#[from] serde_json::Error),
    #[error("reqwest 错误: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("未找到 provider: {0}")]
    ProviderNotFound(String),
    #[error("未找到 endpoint: {0}")]
    EndpointNotFound(String),
    #[error("未找到订阅: {0}")]
    SubscriptionNotFound(String),
    #[error("虚拟模型 '{0}' 未绑定任何订阅")]
    NoHealthySubscription(String),
    #[error("未知虚拟模型: {0}")]
    UnknownVirtualModel(String),
    #[error("请求内容错误: {0}")]
    BadRequest(String),
    #[error("内部错误: {0}")]
    Internal(String),
}

impl AppError {
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(msg.into())
    }
}

impl From<anyhow::Error> for AppError {
    fn from(err: anyhow::Error) -> Self {
        AppError::Internal(err.to_string())
    }
}

/// Tauri command 的错误序列化表示。
#[derive(Debug, Serialize)]
pub struct AppErrorDto {
    pub code: &'static str,
    pub message: String,
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let code = match self {
            AppError::Sqlx(_) => "sqlx",
            AppError::Io(_) => "io",
            AppError::Yaml(_) => "yaml",
            AppError::Json(_) => "json",
            AppError::Reqwest(_) => "network",
            AppError::ProviderNotFound(_) => "provider_not_found",
            AppError::EndpointNotFound(_) => "endpoint_not_found",
            AppError::SubscriptionNotFound(_) => "subscription_not_found",
            AppError::NoHealthySubscription(_) => "no_healthy_subscription",
            AppError::UnknownVirtualModel(_) => "unknown_virtual_model",
            AppError::BadRequest(_) => "bad_request",
            AppError::Internal(_) => "internal",
        };
        AppErrorDto {
            code,
            message: self.to_string(),
        }
        .serialize(serializer)
    }
}

pub type AppResult<T> = Result<T, AppError>;
