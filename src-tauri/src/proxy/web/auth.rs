//! 网页界面鉴权: 会话表 / 登录限流 / cookie 组装解析 / login·logout·session 端点 /
//! `require_session` 与 `require_csrf_header` 两个中间件.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use axum::http::HeaderMap;

pub const SESSION_COOKIE: &str = "ccr_ui_session";
pub const CSRF_HEADER: &str = "x-ccr-ui";
/// 会话空闲过期时间 (30 天) 与 Max-Age 一致.
pub const SESSION_TTL: Duration = Duration::from_secs(30 * 24 * 3600);
pub const SESSION_MAX: usize = 32;
pub const LOGIN_MAX_FAILURES: u32 = 5;
pub const LOGIN_LOCKOUT: Duration = Duration::from_secs(60);

/// 内存会话表. 值为过期时刻; 每次 touch 成功即按 ttl 顺延 (空闲过期).
/// 不持久化: app 重启全体登出 (spec 非目标).
pub struct SessionStore {
    ttl: Duration,
    max: usize,
    map: HashMap<String, Instant>,
}

impl SessionStore {
    pub fn new(ttl: Duration, max: usize) -> Self {
        Self {
            ttl,
            max,
            map: HashMap::new(),
        }
    }

    pub fn new_default() -> Self {
        Self::new(SESSION_TTL, SESSION_MAX)
    }

    fn purge_expired(&mut self, now: Instant) {
        self.map.retain(|_, exp| *exp > now);
    }

    /// 新建会话并返回 id. 满员时淘汰最早过期的一条.
    pub fn create(&mut self, now: Instant) -> String {
        self.purge_expired(now);
        if self.map.len() >= self.max {
            if let Some(oldest) = self
                .map
                .iter()
                .min_by_key(|(_, exp)| **exp)
                .map(|(id, _)| id.clone())
            {
                self.map.remove(&oldest);
            }
        }
        let id = new_session_id();
        self.map.insert(id.clone(), now + self.ttl);
        id
    }

    /// 校验并顺延. 未知 / 已过期 → false (过期条目顺手删除).
    pub fn touch(&mut self, id: &str, now: Instant) -> bool {
        match self.map.get_mut(id) {
            Some(exp) if *exp > now => {
                *exp = now + self.ttl;
                true
            }
            Some(_) => {
                self.map.remove(id);
                false
            }
            None => false,
        }
    }

    pub fn remove(&mut self, id: &str) {
        self.map.remove(id);
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// 64 hex: 两个 uuid v4 (各 122 bit 随机) 拼接, 不引入 rand crate.
fn new_session_id() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

struct GuardEntry {
    failures: u32,
    locked_until: Option<Instant>,
}

/// 按来源 IP 的登录限流: 连续失败 `max_failures` 次锁 `lockout`; 成功清零.
pub struct LoginGuard {
    max_failures: u32,
    lockout: Duration,
    map: HashMap<IpAddr, GuardEntry>,
}

impl LoginGuard {
    pub fn new(max_failures: u32, lockout: Duration) -> Self {
        Self {
            max_failures,
            lockout,
            map: HashMap::new(),
        }
    }

    pub fn new_default() -> Self {
        Self::new(LOGIN_MAX_FAILURES, LOGIN_LOCKOUT)
    }

    /// Ok = 允许尝试; Err(剩余锁定时长) = 拒绝.
    pub fn check(&mut self, ip: IpAddr, now: Instant) -> Result<(), Duration> {
        let Some(entry) = self.map.get_mut(&ip) else {
            return Ok(());
        };
        match entry.locked_until {
            Some(until) if until > now => Err(until - now),
            Some(_) => {
                // 锁定到期: 计数归零
                self.map.remove(&ip);
                Ok(())
            }
            None => Ok(()),
        }
    }

    pub fn record_failure(&mut self, ip: IpAddr, now: Instant) {
        let entry = self.map.entry(ip).or_insert(GuardEntry {
            failures: 0,
            locked_until: None,
        });
        entry.failures += 1;
        if entry.failures >= self.max_failures {
            entry.locked_until = Some(now + self.lockout);
        }
    }

    pub fn record_success(&mut self, ip: IpAddr) {
        self.map.remove(&ip);
    }
}

/// 从 `Cookie` 头里取指定名字的值. 空值等同不存在.
pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get("cookie")?.to_str().ok()?;
    raw.split(';')
        .map(str::trim)
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub fn set_cookie_header(id: &str, secure: bool) -> String {
    let mut s = format!(
        "{SESSION_COOKIE}={id}; HttpOnly; SameSite=Strict; Path=/ui; Max-Age={}",
        SESSION_TTL.as_secs()
    );
    if secure {
        s.push_str("; Secure");
    }
    s
}

pub fn clear_cookie_header() -> String {
    format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Strict; Path=/ui; Max-Age=0")
}

/// 长度不等直接 false; 等长时按位 OR 累加, 不因首个差异位提前返回.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn session_create_touch_expire() {
        let mut s = SessionStore::new(Duration::from_secs(10), 32);
        let id = s.create(t0());
        assert_eq!(id.len(), 64, "两个 uuid simple 拼成 64 hex");
        assert!(s.touch(&id, t0() + Duration::from_secs(5)));
        // touch 刷新过期: 5s 时刷新, 到 14s 仍有效 (5+10 > 14)
        assert!(s.touch(&id, t0() + Duration::from_secs(14)));
        // 14s 刷新后到 25s 过期 (14+10 < 25)
        assert!(!s.touch(&id, t0() + Duration::from_secs(25)));
        assert_eq!(s.len(), 0, "过期条目在 touch 时被删除");
    }

    #[test]
    fn session_unknown_id_is_invalid() {
        let mut s = SessionStore::new(Duration::from_secs(10), 32);
        assert!(!s.touch("nope", t0()));
    }

    #[test]
    fn session_evicts_oldest_when_full() {
        let mut s = SessionStore::new(Duration::from_secs(100), 2);
        let a = s.create(t0());
        let b = s.create(t0() + Duration::from_secs(1));
        let _c = s.create(t0() + Duration::from_secs(2));
        assert_eq!(s.len(), 2);
        assert!(!s.touch(&a, t0() + Duration::from_secs(3)), "最早过期的 a 被淘汰");
        assert!(s.touch(&b, t0() + Duration::from_secs(3)));
    }

    #[test]
    fn session_remove_and_clear() {
        let mut s = SessionStore::new(Duration::from_secs(10), 32);
        let a = s.create(t0());
        let _b = s.create(t0());
        s.remove(&a);
        assert_eq!(s.len(), 1);
        s.clear();
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn login_guard_locks_after_five_failures() {
        let ip: IpAddr = "192.168.1.9".parse().unwrap();
        let base_time = t0();
        let mut g = LoginGuard::new(5, Duration::from_secs(60));
        for _ in 0..4 {
            g.record_failure(ip, base_time);
            assert!(g.check(ip, base_time).is_ok());
        }
        g.record_failure(ip, base_time);
        let remaining = g.check(ip, base_time + Duration::from_secs(10)).unwrap_err();
        assert_eq!(remaining, Duration::from_secs(50));
        assert!(g.check(ip, base_time + Duration::from_secs(61)).is_ok(), "锁定到期自动解锁");
        g.record_failure(ip, base_time + Duration::from_secs(61));
        assert!(g.check(ip, base_time + Duration::from_secs(61)).is_ok(), "解锁后计数从零开始");
    }

    #[test]
    fn login_guard_success_resets() {
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let mut g = LoginGuard::new(5, Duration::from_secs(60));
        for _ in 0..4 {
            g.record_failure(ip, t0());
        }
        g.record_success(ip);
        for _ in 0..4 {
            g.record_failure(ip, t0());
        }
        assert!(g.check(ip, t0()).is_ok());
    }

    #[test]
    fn cookie_value_parses_multiple_cookies() {
        let mut h = HeaderMap::new();
        h.insert("cookie", "a=1; ccr_ui_session=abc123; b=2".parse().unwrap());
        assert_eq!(cookie_value(&h, SESSION_COOKIE).as_deref(), Some("abc123"));
        assert_eq!(cookie_value(&h, "b").as_deref(), Some("2"));
        assert!(cookie_value(&h, "zzz").is_none());
    }

    #[test]
    fn cookie_value_missing_header_or_empty_value() {
        let h = HeaderMap::new();
        assert!(cookie_value(&h, SESSION_COOKIE).is_none());
        let mut h = HeaderMap::new();
        h.insert("cookie", "ccr_ui_session=".parse().unwrap());
        assert!(cookie_value(&h, SESSION_COOKIE).is_none(), "空值视为无 cookie");
    }

    #[test]
    fn set_cookie_header_shape() {
        let v = set_cookie_header("abc", false);
        assert_eq!(v, "ccr_ui_session=abc; HttpOnly; SameSite=Strict; Path=/ui; Max-Age=2592000");
        let v = set_cookie_header("abc", true);
        assert!(v.ends_with("; Secure"));
        assert_eq!(
            clear_cookie_header(),
            "ccr_ui_session=; HttpOnly; SameSite=Strict; Path=/ui; Max-Age=0"
        );
    }

    #[test]
    fn constant_time_eq_basic() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }
}
