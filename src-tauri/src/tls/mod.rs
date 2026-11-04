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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{AppError, AppResult};

mod ca;
mod config;
mod leaf;
mod store;

pub use config::TlsStatus;

/// 启动时调一次. 缺则生成, 有则直接复用. 返回可直接喂给 axum-server 的 rustls ServerConfig.
///
/// `extra_sans` 仅影响新生成的 leaf 证书; 如果磁盘上已有 leaf, 这里不会因 SAN 变化而重签
/// (避免每次启动覆盖用户已固定的证书). 改动后需显式调 `regenerate_leaf`.
///
/// `enable_h2` 控制是否在 TLS ALPN 中通告 h2; true 时 client 可协商 HTTP/2, false 时
/// 强制 HTTP/1.1. 切换后点「重启代理服务」生效 (h2 取重启时的生效配置).
pub async fn load_or_init_server_config(
    app_data_dir: &Path,
    extra_sans: &[String],
    enable_h2: bool,
) -> AppResult<Arc<rustls::ServerConfig>> {
    let tls_dir = tls_dir(app_data_dir);
    tokio::fs::create_dir_all(&tls_dir)
        .await
        .map_err(AppError::Io)?;
    let ca_material = ca::ensure(&tls_dir).await?;
    let leaf_material = leaf::ensure(&tls_dir, &ca_material, extra_sans).await?;
    config::build_server_config(&leaf_material, enable_h2)
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

/// CA 证书导出包: 同一份 PEM 字节落两个后缀 (Windows 双击 .crt 即装, 其余平台用 .pem)。
pub fn ca_zip_bytes(pem: &[u8]) -> AppResult<Vec<u8>> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    for name in ["cc-router-ca.pem", "cc-router-ca.crt"] {
        w.start_file(name, opts).map_err(|e| AppError::internal(e.to_string()))?;
        w.write_all(pem).map_err(AppError::Io)?;
    }
    let cursor = w.finish().map_err(|e| AppError::internal(e.to_string()))?;
    Ok(cursor.into_inner())
}

/// 重新生成 leaf 证书 (CA 不动). 用 `extra_sans` 注入用户配置的额外 SAN.
pub async fn regenerate_leaf(app_data_dir: &Path, extra_sans: &[String]) -> AppResult<()> {
    let tls_dir = tls_dir(app_data_dir);
    let ca_material = ca::load(&tls_dir).await?;
    leaf::force_regenerate(&tls_dir, &ca_material, extra_sans).await
}

/// 读取当前 TLS 状态 (用于前端 status 展示).
pub async fn read_status(app_data_dir: &Path) -> AppResult<TlsStatus> {
    let tls_dir = tls_dir(app_data_dir);
    config::read_status(&tls_dir).await
}

fn tls_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("tls")
}

#[cfg(test)]
mod zip_tests {
    use super::*;

    #[test]
    fn ca_zip_contains_pem_and_crt() {
        let pem = b"-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n";
        let bytes = ca_zip_bytes(pem).unwrap();
        let mut ar = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(ar.len(), 2);
        let mut names: Vec<String> = (0..ar.len()).map(|i| ar.by_index(i).unwrap().name().unwrap().to_string()).collect();
        names.sort();
        assert_eq!(names, vec!["cc-router-ca.crt", "cc-router-ca.pem"]);
        for n in ["cc-router-ca.crt", "cc-router-ca.pem"] {
            let mut f = ar.by_name(n).unwrap();
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut f, &mut buf).unwrap();
            assert_eq!(buf, pem);
        }
    }
}
