//! rustls::ServerConfig 构造 + TlsStatus DTO.

use std::path::Path;
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};
use crate::tls::leaf::LeafMaterial;
use crate::tls::store;

#[derive(Debug, Serialize, Clone)]
pub struct TlsStatus {
    pub ca_exists: bool,
    /// CA 证书 SHA-256 指纹, hex 全小写, 用户用来对比导入信任库的是不是同一张.
    /// 不存在时为 None.
    pub ca_fingerprint_sha256: Option<String>,
    /// CA 公钥 PEM 绝对路径, 供前端「在文件管理器中显示」.
    pub ca_pem_path: Option<String>,
}

pub fn build_server_config(leaf: &LeafMaterial) -> AppResult<Arc<rustls::ServerConfig>> {
    // 安装一次性默认 crypto provider (ring). 多次调用幂等, 已设置时 install_default 返回 Err,
    // 我们安全忽略 — 这一调用只是确保 provider 被初始化, 不是状态切换.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cert_chain: Vec<CertificateDer<'static>> =
        rustls_pemfile::certs(&mut leaf.cert_pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::internal(format!("解析 leaf cert PEM: {e}")))?;
    if cert_chain.is_empty() {
        return Err(AppError::internal("leaf cert PEM 无证书"));
    }

    let key_der: PrivatePkcs8KeyDer<'static> =
        rustls_pemfile::pkcs8_private_keys(&mut leaf.key_pem.as_bytes())
            .next()
            .ok_or_else(|| AppError::internal("leaf key PEM 无 PKCS#8 私钥"))?
            .map_err(|e| AppError::internal(format!("解析 leaf key PEM: {e}")))?;

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, PrivateKeyDer::Pkcs8(key_der))
        .map_err(|e| AppError::internal(format!("rustls ServerConfig: {e}")))?;
    Ok(Arc::new(config))
}

pub async fn read_status(tls_dir: &Path) -> AppResult<TlsStatus> {
    let ca_path = tls_dir.join("ca.pem");
    let ca_exists = store::path_exists(&ca_path).await;
    let (ca_fingerprint_sha256, ca_pem_path) = if ca_exists {
        let pem = store::read_pem(&ca_path).await?;
        let fp = compute_cert_fingerprint(&pem)?;
        (Some(fp), Some(ca_path.to_string_lossy().to_string()))
    } else {
        (None, None)
    };
    Ok(TlsStatus {
        ca_exists,
        ca_fingerprint_sha256,
        ca_pem_path,
    })
}

fn compute_cert_fingerprint(pem: &str) -> AppResult<String> {
    let der_list: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::internal(format!("CA PEM 解析: {e}")))?;
    let first = der_list
        .first()
        .ok_or_else(|| AppError::internal("CA PEM 无证书"))?;
    let mut hasher = Sha256::new();
    hasher.update(first.as_ref());
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn ca_and_leaf_roundtrip_then_build_config() {
        let dir = tempdir().unwrap();
        let _cfg = crate::tls::load_or_init_server_config(dir.path(), &[])
            .await
            .unwrap();
        let tls_dir = dir.path().join("tls");
        let status = read_status(&tls_dir).await.unwrap();
        assert!(status.ca_exists);
        let fp = status.ca_fingerprint_sha256.unwrap();
        assert_eq!(fp.len(), 64, "sha256 hex 应为 64 字符");
    }

    #[tokio::test]
    async fn second_load_reuses_existing_ca() {
        let dir = tempdir().unwrap();
        crate::tls::load_or_init_server_config(dir.path(), &[]).await.unwrap();
        let ca_pem_1 = tokio::fs::read_to_string(dir.path().join("tls/ca.pem"))
            .await
            .unwrap();
        crate::tls::load_or_init_server_config(dir.path(), &[]).await.unwrap();
        let ca_pem_2 = tokio::fs::read_to_string(dir.path().join("tls/ca.pem"))
            .await
            .unwrap();
        assert_eq!(ca_pem_1, ca_pem_2);
    }

    #[tokio::test]
    async fn regenerate_leaf_keeps_ca() {
        let dir = tempdir().unwrap();
        crate::tls::load_or_init_server_config(dir.path(), &[]).await.unwrap();
        let ca_before = tokio::fs::read_to_string(dir.path().join("tls/ca.pem"))
            .await
            .unwrap();
        let leaf_before = tokio::fs::read_to_string(dir.path().join("tls/leaf.pem"))
            .await
            .unwrap();
        crate::tls::regenerate_leaf(dir.path(), &[]).await.unwrap();
        let ca_after = tokio::fs::read_to_string(dir.path().join("tls/ca.pem"))
            .await
            .unwrap();
        let leaf_after = tokio::fs::read_to_string(dir.path().join("tls/leaf.pem"))
            .await
            .unwrap();
        assert_eq!(ca_before, ca_after, "regenerate_leaf 不应动 CA");
        assert_ne!(leaf_before, leaf_after, "leaf 应被替换");
    }
}
