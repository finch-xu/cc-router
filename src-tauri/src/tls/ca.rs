//! Root CA 生成 / 加载.
//!
//! 一次生成永久持久化: 用户导入系统信任库一次后, leaf 证书自动签发不需要重新导入 CA.
//! 私钥用 rcgen 默认 ECDSA P-256 (PKCS#8 PEM), 与 rustls/ring 兼容.

use std::path::{Path, PathBuf};

use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair,
    KeyUsagePurpose,
};
use time::{Duration, OffsetDateTime};

use crate::error::{AppError, AppResult};
use crate::tls::store;

const CA_VALIDITY_YEARS: i64 = 10;
const CA_COMMON_NAME: &str = "cc-router local CA";
const CA_ORG: &str = "cc-router";

pub struct CaMaterial {
    pub cert: Certificate,
    pub key_pair: KeyPair,
}

pub async fn ensure(tls_dir: &Path) -> AppResult<CaMaterial> {
    let (cert_path, key_path) = paths(tls_dir);
    if store::path_exists(&cert_path).await && store::path_exists(&key_path).await {
        return load(tls_dir).await;
    }
    generate_and_save(tls_dir).await
}

/// 仅加载 (假设文件存在). 用于「重新生成 leaf」路径.
pub async fn load(tls_dir: &Path) -> AppResult<CaMaterial> {
    let (cert_path, key_path) = paths(tls_dir);
    let cert_pem = store::read_pem(&cert_path).await?;
    let key_pem = store::read_pem(&key_path).await?;

    let key_pair = KeyPair::from_pem(&key_pem)
        .map_err(|e| AppError::internal(format!("CA 私钥解析失败: {e}")))?;
    // 用 x509-parser feature 提供的入口从 PEM 重建 params, 再 self_signed 还原 Certificate.
    // 注意: self_signed 重签时间窗口与原证书可能略差(纳秒级), 但 SubjectPublicKeyInfo /
    // 颁发者 DN 完全一致, 不影响下游 leaf 续签链路.
    let params = CertificateParams::from_ca_cert_pem(&cert_pem)
        .map_err(|e| AppError::internal(format!("CA 证书解析失败: {e}")))?;
    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| AppError::internal(format!("CA self-sign 重建: {e}")))?;
    Ok(CaMaterial { cert, key_pair })
}

async fn generate_and_save(tls_dir: &Path) -> AppResult<CaMaterial> {
    let mut params = CertificateParams::new(Vec::new())
        .map_err(|e| AppError::internal(format!("rcgen CA params: {e}")))?;
    let now = OffsetDateTime::now_utc();
    params.not_before = now
        .checked_sub(Duration::days(1))
        .ok_or_else(|| AppError::internal("time underflow"))?;
    params.not_after = now
        .checked_add(Duration::days(CA_VALIDITY_YEARS * 365))
        .ok_or_else(|| AppError::internal("time overflow"))?;
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::CommonName, CA_COMMON_NAME);
    params
        .distinguished_name
        .push(DnType::OrganizationName, CA_ORG);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];

    let key_pair = KeyPair::generate()
        .map_err(|e| AppError::internal(format!("CA keypair generate: {e}")))?;
    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| AppError::internal(format!("CA self-sign: {e}")))?;
    let key_pem = key_pair.serialize_pem();

    let (cert_path, key_path) = paths(tls_dir);
    store::write_cert_pem(&cert_path, &cert.pem()).await?;
    store::write_key_pem(&key_path, &key_pem).await?;

    Ok(CaMaterial { cert, key_pair })
}

fn paths(tls_dir: &Path) -> (PathBuf, PathBuf) {
    (tls_dir.join("ca.pem"), tls_dir.join("ca.key"))
}
