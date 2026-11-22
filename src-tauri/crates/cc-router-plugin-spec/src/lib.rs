//! cc-router 插件与厂商描述符的数据模型和校验。
//!
//! 系统插件 (厂商 / 计价 / 规则 / 通知) 在代理里执行, 校验必须在 Rust 侧完成; 这个 crate 是
//! 唯一的规则来源, app (主 crate) 与社区仓库 CI (`ccr-plugin-check`) 共用。只依赖纯数据 crate。
//! 设计: docs/superpowers/specs/2026-11-22-system-plugins-design.md

pub mod balance_json;
pub mod check;
pub mod plugin;
pub mod provider;
pub mod provider_plugin;

use std::fmt;

/// 本 crate 的错误。主 crate 在边界处转成 `AppError` (yaml / json 错误保留原类型, 报错文字不变)。
#[derive(Debug)]
pub enum SpecError {
    Yaml(serde_yaml::Error),
    Json(serde_json::Error),
    Invalid(String),
}

impl fmt::Display for SpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Yaml(e) => write!(f, "{e}"),
            Self::Json(e) => write!(f, "{e}"),
            Self::Invalid(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for SpecError {}

impl From<serde_yaml::Error> for SpecError {
    fn from(e: serde_yaml::Error) -> Self {
        Self::Yaml(e)
    }
}

impl From<serde_json::Error> for SpecError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

pub type SpecResult<T> = Result<T, SpecError>;
