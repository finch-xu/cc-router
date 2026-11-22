use once_cell::sync::OnceCell;

use crate::{SpecError, SpecResult};

static COMPILED: OnceCell<jsonschema::Validator> = OnceCell::new();

/// 与 provider yaml 一样编进二进制。文件留在 `src-tauri/providers/` (作者与 new-provider skill 按这个路径找它)。
const SCHEMA_JSON: &str = include_str!("../../../../providers/_schema.json");

pub fn compile() -> SpecResult<()> {
    if COMPILED.get().is_some() {
        return Ok(());
    }
    let value: serde_json::Value = serde_json::from_str(SCHEMA_JSON)?;
    let schema = jsonschema::validator_for(&value)
        .map_err(|e| SpecError::Invalid(format!("schema 编译失败: {e}")))?;
    let _ = COMPILED.set(schema);
    Ok(())
}

pub fn validate(value: &serde_json::Value) -> SpecResult<()> {
    let schema = COMPILED
        .get()
        .ok_or_else(|| SpecError::Invalid("schema 未初始化".into()))?;
    let messages: Vec<String> = schema.iter_errors(value).map(|e| e.to_string()).collect();
    if !messages.is_empty() {
        return Err(SpecError::Invalid(format!(
            "YAML schema 校验失败: {}",
            messages.join("; ")
        )));
    }
    Ok(())
}
