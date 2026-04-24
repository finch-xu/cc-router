pub mod model;

use std::path::{Path, PathBuf};

use tokio::fs;
use tracing::warn;

use crate::error::AppResult;
use crate::settings::model::Settings;

fn settings_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("settings.json")
}

pub async fn load_or_default(app_data_dir: &Path) -> AppResult<Settings> {
    let path = settings_path(app_data_dir);
    if !path.exists() {
        let default = Settings::default();
        save(app_data_dir, &default).await?;
        return Ok(default);
    }
    match fs::read_to_string(&path).await {
        Ok(raw) => match serde_json::from_str::<Settings>(&raw) {
            Ok(s) => Ok(s),
            Err(e) => {
                warn!(?e, "settings.json 解析失败, 使用默认值");
                Ok(Settings::default())
            }
        },
        Err(e) => {
            warn!(?e, "settings.json 读取失败, 使用默认值");
            Ok(Settings::default())
        }
    }
}

pub async fn save(app_data_dir: &Path, settings: &Settings) -> AppResult<()> {
    if !app_data_dir.exists() {
        fs::create_dir_all(app_data_dir).await?;
    }
    let path = settings_path(app_data_dir);
    let raw = serde_json::to_string_pretty(settings)?;
    fs::write(path, raw).await?;
    Ok(())
}
