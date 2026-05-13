//! Leaf 证书 (server cert) 生成 / 加载.
//!
//! 内置 SAN: DNS=localhost + IP=127.0.0.1 + IP=::1.
//! 用户可通过 `Settings::tls_extra_sans` 追加 IP 或 hostname (改动后需显式重签).
//! 有效期 10 年, 与 CA 同寿命; 不做自动续签 (用户手动点「重新生成 leaf」按钮触发).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};

use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, KeyPair, KeyUsagePurpose,
    SanType,
};
use time::{Duration, OffsetDateTime};
use tracing::warn;

use crate::error::{AppError, AppResult};
use crate::tls::ca::CaMaterial;
use crate::tls::store;

const LEAF_VALIDITY_DAYS: i64 = 365 * 10;
const LEAF_COMMON_NAME: &str = "cc-router local";

pub struct LeafMaterial {
    pub cert_pem: String,
    pub key_pem: String,
}

pub async fn ensure(
    tls_dir: &Path,
    ca: &CaMaterial,
    extra_sans: &[String],
) -> AppResult<LeafMaterial> {
    let (cert_path, key_path) = paths(tls_dir);
    if store::path_exists(&cert_path).await && store::path_exists(&key_path).await {
        let cert_pem = store::read_pem(&cert_path).await?;
        let key_pem = store::read_pem(&key_path).await?;
        return Ok(LeafMaterial { cert_pem, key_pem });
    }
    generate_and_save(tls_dir, ca, extra_sans).await
}

pub async fn force_regenerate(
    tls_dir: &Path,
    ca: &CaMaterial,
    extra_sans: &[String],
) -> AppResult<()> {
    generate_and_save(tls_dir, ca, extra_sans).await.map(|_| ())
}

/// 把用户输入字符串列表解析为 SanType. 空/纯空白跳过; 能 parse 成 IpAddr = IP, 否则按 DNS;
/// rcgen DnsName 进一步校验失败的整条静默丢弃 + warn 日志.
pub(super) fn parse_extra_sans(entries: &[String]) -> Vec<SanType> {
    entries
        .iter()
        .filter_map(|raw| {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return None;
            }
            if let Ok(ip) = trimmed.parse::<IpAddr>() {
                return Some(SanType::IpAddress(ip));
            }
            match trimmed.try_into() {
                Ok(dns) => Some(SanType::DnsName(dns)),
                Err(e) => {
                    warn!(entry = %trimmed, error = %e, "丢弃无效 SAN 条目");
                    None
                }
            }
        })
        .collect()
}

async fn generate_and_save(
    tls_dir: &Path,
    ca: &CaMaterial,
    extra_sans: &[String],
) -> AppResult<LeafMaterial> {
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
    let mut sans = vec![
        SanType::DnsName(
            "localhost"
                .try_into()
                .map_err(|e| AppError::internal(format!("SAN dns: {e}")))?,
        ),
        SanType::IpAddress(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))),
        SanType::IpAddress(IpAddr::V6(Ipv6Addr::LOCALHOST)),
    ];
    sans.extend(parse_extra_sans(extra_sans));
    params.subject_alt_names = sans;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_recognizes_ipv4_ipv6_and_dns() {
        let parsed = parse_extra_sans(&[
            "192.168.1.5".into(),
            "fe80::1".into(),
            "my-laptop.local".into(),
        ]);
        assert_eq!(parsed.len(), 3);
        assert!(matches!(parsed[0], SanType::IpAddress(IpAddr::V4(_))));
        assert!(matches!(parsed[1], SanType::IpAddress(IpAddr::V6(_))));
        assert!(matches!(parsed[2], SanType::DnsName(_)));
    }

    #[test]
    fn parse_skips_empty_and_whitespace() {
        let parsed = parse_extra_sans(&["".into(), "   ".into(), "10.0.0.1".into()]);
        assert_eq!(parsed.len(), 1);
    }

    #[test]
    fn parse_drops_non_ascii_dns() {
        // rcgen DnsName 内部用 Ia5String, 非 ASCII 字符会被拒. 带 scheme / 含空格的串
        // 虽然不是合法 hostname 但仍是 ASCII, rcgen 不强校验, 这里会作为 DNS SAN 保留 —
        // 这是「rcgen 而非 cc-router 来兜底」的设计取舍, 用户填错的最坏后果是签出对应
        // 客户端用不到的证书条目, 无安全风险.
        let parsed = parse_extra_sans(&[
            "中文.test".into(),
            "192.168.1.5".into(), // 仍生效
        ]);
        assert_eq!(parsed.len(), 1, "非 ASCII 条目被丢, IP 保留");
    }
}
