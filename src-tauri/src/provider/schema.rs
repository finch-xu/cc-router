use once_cell::sync::OnceCell;

use crate::error::{AppError, AppResult};

static COMPILED: OnceCell<jsonschema::Validator> = OnceCell::new();

/// 与 provider yaml 一样编进二进制 (见 `loader.rs`)。
const SCHEMA_JSON: &str = include_str!("../../providers/_schema.json");

pub fn compile() -> AppResult<()> {
    if COMPILED.get().is_some() {
        return Ok(());
    }
    let value: serde_json::Value = serde_json::from_str(SCHEMA_JSON)?;
    let schema = jsonschema::validator_for(&value)
        .map_err(|e| AppError::internal(format!("schema 编译失败: {e}")))?;
    let _ = COMPILED.set(schema);
    Ok(())
}

pub fn validate(value: &serde_json::Value) -> AppResult<()> {
    let schema = COMPILED
        .get()
        .ok_or_else(|| AppError::internal("schema 未初始化"))?;
    let messages: Vec<String> = schema.iter_errors(value).map(|e| e.to_string()).collect();
    if !messages.is_empty() {
        return Err(AppError::internal(format!(
            "YAML schema 校验失败: {}",
            messages.join("; ")
        )));
    }
    Ok(())
}
