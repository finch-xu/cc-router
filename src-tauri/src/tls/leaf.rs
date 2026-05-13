//! Leaf 证书 (server cert) 生成 / 加载.
//!
//! SAN 固定: DNS=localhost + IP=127.0.0.1 + IP=::1.
//! 有效期 10 年, 与 CA 同寿命; 不做自动续签检测 (用户手动点「重新生成 leaf」按钮触发).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};

use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, KeyPair, KeyUsagePurpose,
    SanType,
};
use time::{Duration, OffsetDateTime};

use crate::error::{AppError, AppResult};
use crate::tls::ca::CaMaterial;
use crate::tls::store;

const LEAF_VALIDITY_DAYS: i64 = 365 * 10;
const LEAF_COMMON_NAME: &str = "cc-router local";

pub struct LeafMaterial {
    pub cert_pem: String,
    pub key_pem: String,
}

pub async fn ensure(tls_dir: &Path, ca: &CaMaterial) -> AppResult<LeafMaterial> {
    let (cert_path, key_path) = paths(tls_dir);
    if store::path_exists(&cert_path).await && store::path_exists(&key_path).await {
        let cert_pem = store::read_pem(&cert_path).await?;
        let key_pem = store::read_pem(&key_path).await?;
        return Ok(LeafMaterial { cert_pem, key_pem });
    }
    generate_and_save(tls_dir, ca).await
}

pub async fn force_regenerate(tls_dir: &Path, ca: &CaMaterial) -> AppResult<()> {
    generate_and_save(tls_dir, ca).await.map(|_| ())
}

async fn generate_and_save(tls_dir: &Path, ca: &CaMaterial) -> AppResult<LeafMaterial> {
    let mut params = CertificateParams::new(Vec::new())
        .map_err(|e| AppError::internal(format!("rcgen leaf params: {e}")))?;
    let now = OffsetDateTime::now_utc();
    params.not_before = now
        .checked_sub(Duration::days(1))
        .ok_or_else(|| AppError::internal("time underflow"))?;
    params.not_after = now
        .checked_add(Duration::days(LEAF_VALIDITY_DAYS))
        .ok_or_else(|| AppError::internal("time overflow"))?;
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::CommonName, LEAF_COMMON_NAME);
    params.subject_alt_names = vec![
        SanType::DnsName(
            "localhost"
                .try_into()
                .map_err(|e| AppError::internal(format!("SAN dns: {e}")))?,
        ),
        SanType::IpAddress(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))),
        SanType::IpAddress(IpAddr::V6(Ipv6Addr::LOCALHOST)),
    ];
    params.use_authority_key_identifier_extension = true;
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];

    let key_pair = KeyPair::generate()
        .map_err(|e| AppError::internal(format!("leaf keypair: {e}")))?;
    let cert = params
        .signed_by(&key_pair, &ca.cert, &ca.key_pair)
        .map_err(|e| AppError::internal(format!("leaf sign: {e}")))?;
    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();

    let (cert_path, key_path) = paths(tls_dir);
    store::write_cert_pem(&cert_path, &cert_pem).await?;
    store::write_key_pem(&key_path, &key_pem).await?;

    Ok(LeafMaterial { cert_pem, key_pem })
}

fn paths(tls_dir: &Path) -> (PathBuf, PathBuf) {
    (tls_dir.join("leaf.pem"), tls_dir.join("leaf.key"))
}
