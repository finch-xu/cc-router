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
}
