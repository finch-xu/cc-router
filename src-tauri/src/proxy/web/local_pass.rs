//! 「本机通行」: cc-router-tui 凭 runtime.json 里的密钥直达 /ui/api, 不走 cookie 登录,
//! 也不要求 web_ui_enabled。四个条件缺一不可, 见 spec §3.3。
//!
//! 密钥不匹配时**不**返回可区分的状态码 —— 调用方按「没带这个头」继续走原有逻辑,
//! 对外探测不出这条通道是否存在。

use std::net::{IpAddr, SocketAddr};

use axum::body::Body;
use axum::extract::{ConnectInfo, Request};

use crate::proxy::web::auth::constant_time_eq;

pub const LOCAL_HEADER: &str = "x-ccr-local";
/// 只有管理 API 认这个头。静态资源 (/ui/, /ui/{*path}) 不认: 开了 TUI 不等于能打开网页。
pub const API_PREFIX: &str = "/ui/api/";

pub fn is_local_pass(
    tui_enabled: bool,
    presented: Option<&[u8]>,
    expected: &str,
    peer: Option<IpAddr>,
    is_api_path: bool,
) -> bool {
    if !tui_enabled || !is_api_path || expected.is_empty() {
        return false;
    }
    // to_canonical: 把 ::ffff:127.0.0.1 还原成 127.0.0.1 再判回环
    let Some(peer) = peer.map(|ip| ip.to_canonical()) else {
        return false;
    };
    if !peer.is_loopback() {
        return false;
    }
    presented.is_some_and(|p| constant_time_eq(p, expected.as_bytes()))
}

/// 从请求里取头和对端地址。`ConnectInfo` 由 server.rs 的
/// `into_make_service_with_connect_info::<SocketAddr>()` 注入; 取不到就拒绝。
pub fn from_request(
    req: &Request<Body>,
    expected: &str,
    tui_enabled: bool,
    is_api_path: bool,
) -> bool {
    let presented = req.headers().get(LOCAL_HEADER).map(|v| v.as_bytes());
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip());
    is_local_pass(tui_enabled, presented, expected, peer, is_api_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    const SECRET: &str = "s3cret-s3cret-s3cret-s3cret-s3cret-s3cret-x";
    fn lo() -> Option<IpAddr> {
        Some(IpAddr::V4(Ipv4Addr::LOCALHOST))
    }
    fn lan() -> Option<IpAddr> {
        Some(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)))
    }
    fn ok() -> Option<&'static [u8]> {
        Some(SECRET.as_bytes())
    }

    #[test]
    fn all_four_conditions_pass() {
        assert!(is_local_pass(true, ok(), SECRET, lo(), true));
        assert!(is_local_pass(true, ok(), SECRET, Some(IpAddr::V6(Ipv6Addr::LOCALHOST)), true));
    }

    #[test]
    fn each_condition_is_individually_required() {
        assert!(!is_local_pass(false, ok(), SECRET, lo(), true), "tui_enabled 关");
        assert!(!is_local_pass(true, None, SECRET, lo(), true), "没带头");
        assert!(!is_local_pass(true, Some(b"wrong"), SECRET, lo(), true), "密钥错");
        assert!(!is_local_pass(true, ok(), SECRET, lan(), true), "非回环");
        assert!(!is_local_pass(true, ok(), SECRET, None, true), "拿不到对端地址 → 拒绝 (fail closed)");
        assert!(!is_local_pass(true, ok(), SECRET, lo(), false), "静态资源路径");
    }

    /// 防御: 万一 AppState 被构造成空密钥, 空头不能因为「相等」而通过。
    #[test]
    fn empty_expected_secret_never_passes() {
        assert!(!is_local_pass(true, Some(b""), "", lo(), true));
    }

    /// listener 目前只绑 IPv4, 但双栈 socket 上回环会显示成 ::ffff:127.0.0.1。
    #[test]
    fn ipv4_mapped_loopback_counts_as_loopback() {
        let mapped = IpAddr::V6(Ipv4Addr::LOCALHOST.to_ipv6_mapped());
        assert!(is_local_pass(true, ok(), SECRET, Some(mapped), true));
        let mapped_lan = IpAddr::V6(Ipv4Addr::new(10, 0, 0, 5).to_ipv6_mapped());
        assert!(!is_local_pass(true, ok(), SECRET, Some(mapped_lan), true));
    }

    #[test]
    fn from_request_reads_header_and_connect_info() {
        let build = |secret: Option<&str>, peer: Option<SocketAddr>| {
            let mut b = Request::builder().uri("/ui/api/cmd/list_subscriptions");
            if let Some(s) = secret {
                b = b.header(LOCAL_HEADER, s);
            }
            let mut req = b.body(Body::empty()).unwrap();
            if let Some(p) = peer {
                req.extensions_mut().insert(ConnectInfo(p));
            }
            req
        };
        let lo: SocketAddr = "127.0.0.1:51000".parse().unwrap();
        let lan: SocketAddr = "192.168.1.20:51000".parse().unwrap();
        assert!(from_request(&build(Some(SECRET), Some(lo)), SECRET, true, true));
        assert!(!from_request(&build(Some(SECRET), Some(lan)), SECRET, true, true));
        assert!(!from_request(&build(Some(SECRET), None), SECRET, true, true));
        assert!(!from_request(&build(None, Some(lo)), SECRET, true, true));
    }
}
