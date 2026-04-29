//! 更新源(manifest URL)集中管理。
//!
//! cc-router 提供两个内置更新源:
//! - 国际(international): GitHub Releases 直连,海外用户用
//! - 中国大陆(china): 阿里云 OSS 镜像,大陆用户用
//!
//! 实际选择由 `Settings::update_source` 字段决定;`None` 时回退 `tauri.conf.json` 默认值。

pub const INTERNATIONAL_MANIFEST_URL: &str =
    "https://github.com/finch-xu/cc-router/releases/latest/download/latest.json";

// 阿里云 OSS bucket=cc-router (oss-cn-shanghai),bucket 直访 URL,公共读 ACL。
// 用 bucket 域名而非 access point 域名,因为 access point 不支持匿名访问。
// CI 在 release 时把 binary/sig 双发到 GitHub Release + OSS,manifest 重写 URL 字段为 OSS 前缀。
pub const CHINA_MANIFEST_URL: &str =
    "https://cc-router.oss-cn-shanghai.aliyuncs.com/latest.json";

/// 把 `Settings::update_source` 映射成具体 manifest URL。
/// 返回 `None` 表示走 `tauri.conf.json::plugins.updater.endpoints` 默认值。
pub fn manifest_url_for(source: Option<&str>) -> Option<&'static str> {
    match source {
        Some("china") => Some(CHINA_MANIFEST_URL),
        Some("international") => Some(INTERNATIONAL_MANIFEST_URL),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known_sources() {
        assert_eq!(manifest_url_for(Some("china")), Some(CHINA_MANIFEST_URL));
        assert_eq!(
            manifest_url_for(Some("international")),
            Some(INTERNATIONAL_MANIFEST_URL)
        );
    }

    #[test]
    fn unknown_or_none_returns_none() {
        assert!(manifest_url_for(None).is_none());
        assert!(manifest_url_for(Some("")).is_none());
        assert!(manifest_url_for(Some("foo")).is_none());
    }
}
