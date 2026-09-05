use std::path::Path;

use once_cell::sync::OnceCell;

use crate::error::{AppError, AppResult};

static COMPILED: OnceCell<jsonschema::Validator> = OnceCell::new();

pub fn compile(schema_path: &Path) -> AppResult<()> {
    if COMPILED.get().is_some() {
        return Ok(());
    }
    let raw = std::fs::read_to_string(schema_path)?;
    let value: serde_json::Value = serde_json::from_str(&raw)?;
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
