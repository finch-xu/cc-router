use std::collections::HashMap;
use std::path::Path;

use tracing::{error, info, warn};

use crate::error::{AppError, AppResult};
use crate::provider::model::Provider;
use crate::provider::schema;

/// 扫描 `<resource_dir>/providers/*.yaml`，逐个解析并校验。
/// 单个文件失败不阻塞其他 provider 加载；id 冲突是 fatal。
pub fn load_all(resource_dir: &Path) -> AppResult<HashMap<String, Provider>> {
    let providers_dir = resource_dir.join("providers");
    let schema_path = providers_dir.join("_schema.json");

    if !providers_dir.exists() {
        return Err(AppError::internal(format!(
            "providers 目录不存在: {}",
            providers_dir.display()
        )));
    }

    if schema_path.exists() {
        if let Err(e) = schema::compile(&schema_path) {
            warn!(?e, "provider schema 编译失败, 将跳过 schema 校验");
        }
    } else {
        warn!("_schema.json 缺失, 跳过 schema 校验");
    }

    let mut map: HashMap<String, Provider> = HashMap::new();
    let entries = std::fs::read_dir(&providers_dir)?;
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                warn!(?e, "读取 provider 目录项失败, 跳过");
                continue;
            }
        };
        let path = entry.path();
        let is_yaml = path
            .extension()
            .map(|ext| ext == "yaml" || ext == "yml")
            .unwrap_or(false);
        if !is_yaml {
            continue;
        }

        match load_single(&path) {
            Ok(provider) => {
                if map.contains_key(&provider.id) {
                    return Err(AppError::internal(format!(
                        "provider id '{}' 冲突 (文件: {})",
                        provider.id,
                        path.display()
                    )));
                }
                info!(provider = %provider.id, file = %path.display(), "provider loaded");
                map.insert(provider.id.clone(), provider);
            }
            Err(e) => {
                error!(?e, file = %path.display(), "provider 加载失败, 跳过");
            }
        }
    }

    Ok(map)
}

fn load_single(path: &Path) -> AppResult<Provider> {
    let raw = std::fs::read_to_string(path)?;
    let as_yaml: serde_yaml::Value = serde_yaml::from_str(&raw)?;
    // 转为 JSON 以便 schema 校验
    let as_json = serde_json::to_value(&as_yaml)?;
    let _ = schema::validate(&as_json);
    let provider: Provider = serde_yaml::from_value(as_yaml)?;
    Ok(provider)
}
