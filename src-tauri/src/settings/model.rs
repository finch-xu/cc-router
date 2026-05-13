use serde::{Deserialize, Serialize};

/// 代理监听协议组合。
/// - `Http`: 仅 HTTP (默认, 与历史行为一致)
/// - `Https`: 仅 HTTPS (用 cc-router 自签 CA 签发的 leaf 证书)
/// - `Both`: HTTP 和 HTTPS 双端口同时监听, 共享同一份 AppState
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    Http,
    Https,
    Both,
}

impl Default for ProxyMode {
    fn default() -> Self {
        Self::Http
    }
}

impl ProxyMode {
    pub fn includes_http(self) -> bool {
        matches!(self, Self::Http | Self::Both)
    }
    pub fn includes_https(self) -> bool {
        matches!(self, Self::Https | Self::Both)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// HTTP 端口 (仅在 proxy_mode 包含 Http 时使用). 字段名保留 `proxy_port`
    /// 兼容老 settings.json; HTTPS 单独由 `https_port` 控制.
    #[serde(default = "default_port")]
    pub proxy_port: u16,
    /// 代理监听协议组合, 默认仅 HTTP. 切换需要重启 app.
    #[serde(default)]
    pub proxy_mode: ProxyMode,
    /// HTTPS 端口 (仅在 proxy_mode 包含 Https 时使用). 默认 23457.
    #[serde(default = "default_https_port")]
    pub https_port: u16,
    /// 用户配置的额外 SAN (Subject Alternative Name) 列表. 每条字符串按 IpAddr::from_str 尝试解析:
    /// 成功 = IP SAN; 失败 = DnsName SAN (rcgen 进一步校验, 不合规的静默丢弃).
    /// 内置 localhost / 127.0.0.1 / ::1 永远在 SAN 列表里, 此 vec 是追加项.
    /// 改动后需点「重新生成 leaf」按钮 + 重启 app 才生效.
    #[serde(default)]
    pub tls_extra_sans: Vec<String>,
    /// true: 监听 0.0.0.0（局域网可访问）；false: 监听 127.0.0.1（仅本机）。
    #[serde(default)]
    pub listen_all: bool,
    #[serde(default)]
    pub autostart: bool,
    #[serde(default = "default_retention")]
    pub log_retention_days: u32,
    #[serde(default = "default_db_limit")]
    pub db_size_limit_mb: u32,
    /// true: 代理校验 token;false: 完全放行
    #[serde(default = "default_auth_enabled")]
    pub auth_enabled: bool,
    /// 鉴权 token 明文。空字符串表示未生成,bootstrap 时会自动生成并 save。
    /// 客户端通过 `x-api-key` 或 `Authorization: Bearer <token>` 携带。
    #[serde(default)]
    pub auth_token: String,
    /// true: 代理为响应附加 CORS 头;false: 不附加(浏览器跨域调用会被拦截)
    #[serde(default = "default_cors_enabled")]
    pub cors_enabled: bool,
    /// CORS Access-Control-Allow-Origin 值,默认 `*` 放行所有来源。
    #[serde(default = "default_cors_origin")]
    pub cors_allow_origin: String,
    /// 前端 UI 语言偏好。"system" / "zh" / "en"。默认 system,
    /// system 模式下前端用 navigator.language 决定 zh 或 en。
    #[serde(default = "default_preferred_language")]
    pub preferred_language: String,
    /// 更新源选择: None=未设置(走 tauri.conf.json 默认 GitHub),
    /// Some("international")=国际(GitHub) / Some("china")=中国大陆(阿里云 OSS)。
    /// 首次启动后前端按 navigator.language 自动写入,之后用户 Settings 切换覆盖。
    #[serde(default)]
    pub update_source: Option<String>,
    /// 调试模式: 开启后每次出站 attempt 把客户端请求体 / cc-router 出站请求体 /
    /// 上游响应体三段写入 `<app_data_dir>/debug-dumps/` 下 .txt 文件,
    /// 用于排查协议适配类问题. 默认关闭(file IO 与磁盘占用代价).
    #[serde(default)]
    pub debug_mode: bool,
}

fn default_port() -> u16 {
    23456
}
fn default_https_port() -> u16 {
    23457
}
fn default_retention() -> u32 {
    30
}
fn default_db_limit() -> u32 {
    500
}
fn default_auth_enabled() -> bool {
    true
}
fn default_cors_enabled() -> bool {
    true
}
fn default_cors_origin() -> String {
    "*".to_string()
}
fn default_preferred_language() -> String {
    "system".to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            proxy_port: default_port(),
            proxy_mode: ProxyMode::default(),
            https_port: default_https_port(),
            tls_extra_sans: Vec::new(),
            listen_all: false,
            autostart: false,
            log_retention_days: default_retention(),
            db_size_limit_mb: default_db_limit(),
            auth_enabled: default_auth_enabled(),
            auth_token: String::new(),
            cors_enabled: default_cors_enabled(),
            cors_allow_origin: default_cors_origin(),
            preferred_language: default_preferred_language(),
            update_source: None,
            debug_mode: false,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SettingsPatch {
    pub proxy_port: Option<u16>,
    pub proxy_mode: Option<ProxyMode>,
    pub https_port: Option<u16>,
    pub tls_extra_sans: Option<Vec<String>>,
    pub listen_all: Option<bool>,
    pub autostart: Option<bool>,
    pub log_retention_days: Option<u32>,
    pub db_size_limit_mb: Option<u32>,
    pub auth_enabled: Option<bool>,
    pub cors_enabled: Option<bool>,
    pub cors_allow_origin: Option<String>,
    pub preferred_language: Option<String>,
    pub update_source: Option<String>,
    pub debug_mode: Option<bool>,
}

impl Settings {
    pub fn apply_patch(&mut self, patch: SettingsPatch) {
        if let Some(p) = patch.proxy_port {
            self.proxy_port = p;
        }
        if let Some(p) = patch.proxy_mode {
            self.proxy_mode = p;
        }
        if let Some(p) = patch.https_port {
            self.https_port = p;
        }
        if let Some(p) = patch.tls_extra_sans {
            self.tls_extra_sans = p;
        }
        if let Some(p) = patch.listen_all {
            self.listen_all = p;
        }
        if let Some(p) = patch.autostart {
            self.autostart = p;
        }
        if let Some(p) = patch.log_retention_days {
            self.log_retention_days = p;
        }
        if let Some(p) = patch.db_size_limit_mb {
            self.db_size_limit_mb = p;
        }
        if let Some(p) = patch.auth_enabled {
            self.auth_enabled = p;
        }
        if let Some(p) = patch.cors_enabled {
            self.cors_enabled = p;
        }
        if let Some(p) = patch.cors_allow_origin {
            self.cors_allow_origin = p;
        }
        if let Some(p) = patch.preferred_language {
            self.preferred_language = p;
        }
        if let Some(p) = patch.update_source {
            self.update_source = Some(p);
        }
        if let Some(p) = patch.debug_mode {
            self.debug_mode = p;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_update_source_is_none() {
        assert!(Settings::default().update_source.is_none());
    }

    #[test]
    fn legacy_settings_json_without_update_source_deserializes_to_none() {
        // 老版本 settings.json 里没有 update_source 字段,#[serde(default)] 应该填 None
        let raw = r#"{
            "proxy_port": 23456,
            "listen_all": false,
            "autostart": false,
            "log_retention_days": 30,
            "db_size_limit_mb": 500,
            "auth_enabled": true,
            "auth_token": "abc",
            "cors_enabled": true,
            "cors_allow_origin": "*",
            "preferred_language": "system"
        }"#;
        let s: Settings = serde_json::from_str(raw).unwrap();
        assert!(s.update_source.is_none());
    }

    #[test]
    fn apply_patch_sets_update_source() {
        let mut s = Settings::default();
        s.apply_patch(SettingsPatch {
            update_source: Some("china".into()),
            ..Default::default()
        });
        assert_eq!(s.update_source.as_deref(), Some("china"));
    }

    #[test]
    fn apply_patch_none_does_not_clear_update_source() {
        let mut s = Settings::default();
        s.update_source = Some("china".into());
        s.apply_patch(SettingsPatch::default()); // 全 None patch
        assert_eq!(s.update_source.as_deref(), Some("china"));
    }

    #[test]
    fn default_proxy_mode_is_http() {
        let s = Settings::default();
        assert_eq!(s.proxy_mode, ProxyMode::Http);
        assert_eq!(s.https_port, 23457);
    }

    #[test]
    fn legacy_settings_json_without_proxy_mode_deserializes_to_http() {
        // 老版本 settings.json 没 proxy_mode / https_port 字段
        let raw = r#"{
            "proxy_port": 23456,
            "listen_all": false,
            "autostart": false,
            "log_retention_days": 30,
            "db_size_limit_mb": 500,
            "auth_enabled": true,
            "auth_token": "abc",
            "cors_enabled": true,
            "cors_allow_origin": "*",
            "preferred_language": "system"
        }"#;
        let s: Settings = serde_json::from_str(raw).unwrap();
        assert_eq!(s.proxy_mode, ProxyMode::Http);
        assert_eq!(s.https_port, 23457);
    }

    #[test]
    fn proxy_mode_both_string_deserializes() {
        let raw = r#"{"proxy_port":23456,"proxy_mode":"both","https_port":23457,"listen_all":false,"autostart":false,"log_retention_days":30,"db_size_limit_mb":500,"auth_enabled":true,"auth_token":"","cors_enabled":true,"cors_allow_origin":"*","preferred_language":"system"}"#;
        let s: Settings = serde_json::from_str(raw).unwrap();
        assert_eq!(s.proxy_mode, ProxyMode::Both);
        assert!(s.proxy_mode.includes_http());
        assert!(s.proxy_mode.includes_https());
    }

    #[test]
    fn apply_patch_sets_proxy_mode_and_https_port() {
        let mut s = Settings::default();
        s.apply_patch(SettingsPatch {
            proxy_mode: Some(ProxyMode::Https),
            https_port: Some(24000),
            ..Default::default()
        });
        assert_eq!(s.proxy_mode, ProxyMode::Https);
        assert_eq!(s.https_port, 24000);
        // 未传的字段保持默认
        assert_eq!(s.proxy_port, 23456);
    }

    #[test]
    fn default_tls_extra_sans_is_empty() {
        assert!(Settings::default().tls_extra_sans.is_empty());
    }

    #[test]
    fn legacy_settings_json_without_tls_extra_sans_deserializes_to_empty() {
        let raw = r#"{
            "proxy_port": 23456, "proxy_mode": "http", "https_port": 23457,
            "listen_all": false, "autostart": false,
            "log_retention_days": 30, "db_size_limit_mb": 500,
            "auth_enabled": true, "auth_token": "",
            "cors_enabled": true, "cors_allow_origin": "*",
            "preferred_language": "system"
        }"#;
        let s: Settings = serde_json::from_str(raw).unwrap();
        assert!(s.tls_extra_sans.is_empty());
    }

    #[test]
    fn apply_patch_sets_tls_extra_sans() {
        let mut s = Settings::default();
        s.apply_patch(SettingsPatch {
            tls_extra_sans: Some(vec!["192.168.1.5".to_string(), "my-laptop.local".to_string()]),
            ..Default::default()
        });
        assert_eq!(
            s.tls_extra_sans,
            vec!["192.168.1.5".to_string(), "my-laptop.local".to_string()]
        );
    }
}
