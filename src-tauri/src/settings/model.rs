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

impl Default for Settings {
    fn default() -> Self {
        Self {
            proxy_port: default_port(),
            listen_all: false,
            autostart: false,
            log_retention_days: default_retention(),
            db_size_limit_mb: default_db_limit(),
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
    }
}
