//! TLS 证书文件 IO. 私钥写盘 unix 下 0600.

use std::path::Path;

use tokio::fs;

use crate::error::{AppError, AppResult};

pub async fn read_pem(path: &Path) -> AppResult<String> {
    fs::read_to_string(path).await.map_err(AppError::Io)
}

pub async fn write_cert_pem(path: &Path, pem: &str) -> AppResult<()> {
    fs::write(path, pem).await.map_err(AppError::Io)
}

/// 私钥写盘: unix 下严格 0600, windows 走默认 ACL.
pub async fn write_key_pem(path: &Path, pem: &str) -> AppResult<()> {
    fs::write(path, pem).await.map_err(AppError::Io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = fs::metadata(path).await.map_err(AppError::Io)?.permissions();
        perm.set_mode(0o600);
        fs::set_permissions(path, perm).await.map_err(AppError::Io)?;
    }
    Ok(())
}

pub async fn path_exists(path: &Path) -> bool {
    fs::try_exists(path).await.unwrap_or(false)
}
