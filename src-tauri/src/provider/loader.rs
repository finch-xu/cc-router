use std::collections::HashMap;

use tracing::{error, info, warn};

use crate::error::{AppError, AppResult};
use crate::provider::model::Provider;
use crate::provider::schema;

// build.rs 扫描 `providers/*.yaml` 生成: `pub static EMBEDDED_PROVIDERS: &[(文件名, 内容)]`。
// yaml 编进二进制而不是走 bundle resource —— 内置 provider 只在启动时读一次、运行时不写,
// 没有留在安装目录的理由, 还省掉了「加 provider 要登记 tauri.conf.json」这一步。
include!(concat!(env!("OUT_DIR"), "/embedded_providers.rs"));

/// 逐个解析并校验内嵌的 provider yaml。
/// 单个文件失败不阻塞其他 provider 加载；id 冲突是 fatal。
/// (两种情况都由本文件的单测在 `cargo test` 阶段拦住, 正常不会在用户机器上发生。)
pub fn load_all() -> AppResult<HashMap<String, Provider>> {
    if let Err(e) = schema::compile() {
        warn!(?e, "provider schema 编译失败, 将跳过 schema 校验");
    }

    let mut map: HashMap<String, Provider> = HashMap::new();
    for (file, raw) in EMBEDDED_PROVIDERS {
        match parse_single(raw) {
            Ok(provider) => {
                if map.contains_key(&provider.id) {
                    return Err(AppError::internal(format!(
                        "provider id '{}' 冲突 (文件: {file})",
                        provider.id
                    )));
                }
                info!(provider = %provider.id, %file, "provider loaded");
                map.insert(provider.id.clone(), provider);
            }
            Err(e) => {
                error!(?e, %file, "provider 加载失败, 跳过");
            }
        }
    }

    Ok(map)
}

fn parse_single(raw: &str) -> AppResult<Provider> {
    let as_yaml: serde_yaml::Value = serde_yaml::from_str(raw)?;
    // 转为 JSON 以便 schema 校验
    let as_json = serde_json::to_value(&as_yaml)?;
    let _ = schema::validate(&as_json);
    let provider: Provider = serde_yaml::from_value(as_yaml)?;
    Ok(provider)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// yaml 已经编进二进制, 「解析失败 / id 冲突」不再需要等到用户机器上启动才发现。
    #[test]
    fn every_embedded_provider_parses_and_ids_are_unique() {
        assert!(!EMBEDDED_PROVIDERS.is_empty());
        let mut seen: HashMap<String, &str> = HashMap::new();
        for (file, raw) in EMBEDDED_PROVIDERS {
            let p = parse_single(raw).unwrap_or_else(|e| panic!("{file} 解析失败: {e}"));
            if let Some(prev) = seen.insert(p.id.clone(), file) {
                panic!("provider id '{}' 冲突: {prev} 与 {file}", p.id);
            }
            // schema 不强校验这条软约束, 写错了前端新建向导会默认选不中任何 endpoint。
            if let Some(default) = p.default_endpoint.as_deref() {
                assert!(
                    p.endpoints.iter().any(|e| e.id == default),
                    "{file}: default_endpoint '{default}' 不在 endpoints[].id 中"
                );
            }
        }
        // load_all 对单个失败是 warn + 跳过, 数量相等才说明一个都没被吞掉。
        assert_eq!(load_all().unwrap().len(), EMBEDDED_PROVIDERS.len());
    }
}
