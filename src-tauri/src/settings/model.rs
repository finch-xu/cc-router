use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_port")]
    pub proxy_port: u16,
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
}

fn default_port() -> u16 {
    23456
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

impl Default for Settings {
    fn default() -> Self {
        Self {
            proxy_port: default_port(),
            listen_all: false,
            autostart: false,
            log_retention_days: default_retention(),
            db_size_limit_mb: default_db_limit(),
            auth_enabled: default_auth_enabled(),
            auth_token: String::new(),
            cors_enabled: default_cors_enabled(),
            cors_allow_origin: default_cors_origin(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SettingsPatch {
    pub proxy_port: Option<u16>,
    pub listen_all: Option<bool>,
    pub autostart: Option<bool>,
    pub log_retention_days: Option<u32>,
    pub db_size_limit_mb: Option<u32>,
    pub auth_enabled: Option<bool>,
    pub cors_enabled: Option<bool>,
    pub cors_allow_origin: Option<String>,
}

impl Settings {
    pub fn apply_patch(&mut self, patch: SettingsPatch) {
        if let Some(p) = patch.proxy_port {
            self.proxy_port = p;
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
    }
}
