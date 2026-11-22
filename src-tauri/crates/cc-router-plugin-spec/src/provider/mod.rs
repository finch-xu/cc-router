//! 厂商描述符 (`src-tauri/providers/*.yaml`, 以及将来的厂商插件) 的模型、模板与校验。
//! 从主 crate 的 `provider` 模块搬来; 主 crate 重新导出, `crate::provider::model::…` 路径不变。

pub mod model;
pub mod schema;
pub mod semantics;
pub mod url_template;

pub use model::{Provider, ProviderEndpoint};

use crate::SpecResult;

/// 解析并校验一份内置厂商 yaml。schema 校验结果不阻塞 (与搬家前一致, 以 serde + 语义校验为准)。
pub fn parse_builtin(raw: &str) -> SpecResult<Provider> {
    let as_yaml: serde_yaml::Value = serde_yaml::from_str(raw)?;
    // 转为 JSON 以便 schema 校验
    let as_json = serde_json::to_value(&as_yaml)?;
    let _ = schema::validate(&as_json);
    let provider: Provider = serde_yaml::from_value(as_yaml)?;
    semantics::validate(&provider)?;
    Ok(provider)
}
