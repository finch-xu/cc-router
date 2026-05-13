//! 本地 TLS 证书管理 (HTTPS 模式).
//!
//! 设计:
//! - **root CA 长期持久化** (10 年), 用户导入一次系统信任库后即长期信任 cc-router 签发的证书.
//! - **leaf 证书短期自动续签** (1 年), 过期 <30 天或 SAN 不匹配时透明重签.
//! - SAN 覆盖 `localhost` / `127.0.0.1` / `::1` 三个 loopback 入口. listen_all=true 用 LAN IP
//!   访问会触发 SAN 不匹配 (设计如此, 未来 v2 加自定义 hostname 支持).
//! - 所有证书文件落 `<app_data_dir>/tls/{ca.pem,ca.key,leaf.pem,leaf.key}`,
//!   私钥 unix 下 0600. 不进 SQLite (与 settings.json / config.db 风格一致).
//!
//! 不在本模块范围:
//! - 装信任库的跨平台调用 (admin 提权; 本期由用户手动导入).
//! - mTLS 客户端证书.
//! - 任意 hostname/IP 重签 leaf.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{AppError, AppResult};

mod ca;
mod config;
mod leaf;
mod store;

pub use config::TlsStatus;

/// 启动时调一次. 缺则生成, 有则验证 + 必要时续签 leaf. 返回可直接喂给 axum-server 的 rustls ServerConfig.
pub async fn load_or_init_server_config(
    app_data_dir: &Path,
) -> AppResult<Arc<rustls::ServerConfig>> {
    let tls_dir = tls_dir(app_data_dir);
    tokio::fs::create_dir_all(&tls_dir)
        .await
        .map_err(AppError::Io)?;
    // CA: 不存在则生成.
    let ca_material = ca::ensure(&tls_dir).await?;
    // Leaf: 不存在 / 过期 / SAN 不匹配则用 CA 重签.
    let leaf_material = leaf::ensure(&tls_dir, &ca_material).await?;
    config::build_server_config(&leaf_material)
}

/// 仅确保 CA 存在 (不签 leaf, 不建 rustls ServerConfig).
/// 用于「导出 CA」「重新生成 leaf」等只需 CA 在场的场景.
pub async fn ensure_ca(app_data_dir: &Path) -> AppResult<()> {
    let tls_dir = tls_dir(app_data_dir);
    tokio::fs::create_dir_all(&tls_dir)
        .await
        .map_err(AppError::Io)?;
    ca::ensure(&tls_dir).await.map(|_| ())
}

/// CA 公钥 PEM 文件路径. 用于前端「在 Finder 中显示」.
pub fn ca_pem_path(app_data_dir: &Path) -> PathBuf {
    tls_dir(app_data_dir).join("ca.pem")
}

/// 拷贝 CA 公钥 PEM 到用户选择的目标路径.
pub async fn export_ca_pem(app_data_dir: &Path, dest: &Path) -> AppResult<()> {
    let src = ca_pem_path(app_data_dir);
    tokio::fs::copy(&src, dest)
        .await
        .map(|_| ())
        .map_err(AppError::Io)
}

/// 重新生成 leaf 证书 (CA 不动). 调试 / 手动续签入口.
pub async fn regenerate_leaf(app_data_dir: &Path) -> AppResult<()> {
    let tls_dir = tls_dir(app_data_dir);
    let ca_material = ca::load(&tls_dir).await?;
    leaf::force_regenerate(&tls_dir, &ca_material).await
}

/// 读取当前 TLS 状态 (用于前端 status 展示).
pub async fn read_status(app_data_dir: &Path) -> AppResult<TlsStatus> {
    let tls_dir = tls_dir(app_data_dir);
    config::read_status(&tls_dir).await
}

fn tls_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("tls")
}
