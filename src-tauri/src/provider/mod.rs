pub mod loader;
pub mod registry;

// 模型 / 模板 / schema 在 cc-router-plugin-spec (厂商插件与社区 CI 共用同一份), 这里重新导出,
// `crate::provider::model::…` 等路径保持不变。
pub use cc_router_plugin_spec::provider::{model, schema, url_template};

pub use model::{Provider, ProviderEndpoint};

/// `BalanceHttpMethod` 转 reqwest 的方法 (spec crate 不依赖 reqwest)。
pub trait BalanceMethodExt {
    fn as_reqwest(self) -> reqwest::Method;
}

impl BalanceMethodExt for model::BalanceHttpMethod {
    fn as_reqwest(self) -> reqwest::Method {
        match self {
            Self::Get => reqwest::Method::GET,
            Self::Post => reqwest::Method::POST,
        }
    }
}
