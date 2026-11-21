//! Network timing of one upstream attempt, for the request log (pure observability).
//!
//! What is measured, all relative to the attempt start:
//! - `prep`: attempt start → the request is handed to reqwest (auth, OAuth refresh, translation).
//! - `dns` / `connect` (TCP + TLS): only when this request opened a new connection.
//!   Most requests reuse a pooled connection and have neither; `conn_reused` says which.
//! - `headers`: attempt start → upstream response head received. For a streaming upstream
//!   this is network + upstream gateway (≈ one round trip); for a non-streaming one the
//!   upstream only answers after generating, so it includes the whole generation.
//!
//! How a measurement is tied to its request: [`send_timed`] runs the send inside a
//! task-local [`Probe`]; [`TimedResolver`] and [`TimedConnectLayer`] write into it. hyper-util
//! races "new connection" against "idle pooled connection"; a losing connect keeps running
//! in a spawned task and may still write into the probe. So a connect only counts when the
//! connection that answered (its local address, from the response's `HttpInfo`) is the one
//! the probe saw being opened.
//!
//! Same discipline as effort / tool / header logs: nothing here may fail a request. Every
//! value is optional and a missing one is just `None`.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use hyper_util::client::legacy::connect::{Connection, HttpInfo};
use serde::Serialize;

tokio::task_local! {
    static PROBE: Arc<Mutex<Probe>>;
}

#[derive(Debug, Default)]
struct Probe {
    /// Last finished lookup not yet claimed by a connect.
    dns: Option<(Duration, String)>,
    /// Last connect attempt (successful or not).
    connect: Option<ConnectRecord>,
}

#[derive(Debug, Clone)]
struct ConnectRecord {
    /// Whole connect including DNS.
    total: Duration,
    dns: Option<(Duration, String)>,
    local: Option<SocketAddr>,
    proxied: bool,
    ok: bool,
}

fn current_probe() -> Option<Arc<Mutex<Probe>>> {
    PROBE.try_with(Arc::clone).ok()
}

fn with_probe(probe: &Option<Arc<Mutex<Probe>>>, f: impl FnOnce(&mut Probe)) {
    if let Some(p) = probe {
        if let Ok(mut g) = p.lock() {
            f(&mut g);
        }
    }
}

// ---------------------------------------------------------------------------
// DNS

/// System resolver (getaddrinfo on the blocking pool, same as reqwest's default) that
/// records how long each lookup took.
pub struct TimedResolver;

impl reqwest::dns::Resolve for TimedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let probe = current_probe();
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let t = Instant::now();
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            let took = t.elapsed();
            with_probe(&probe, |p| p.dns = Some((took, host)));
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

// ---------------------------------------------------------------------------
// Connect (TCP + TLS)

/// Tower layer around reqwest's connector: times the whole connect and notes which local
/// address the new connection got.
#[derive(Clone, Copy, Default)]
pub struct TimedConnectLayer;

impl<S> tower::Layer<S> for TimedConnectLayer {
    type Service = TimedConnect<S>;
    fn layer(&self, inner: S) -> Self::Service {
        TimedConnect { inner }
    }
}

#[derive(Clone)]
pub struct TimedConnect<S> {
    inner: S,
}

impl<S, R> tower::Service<R> for TimedConnect<S>
where
    S: tower::Service<R>,
    S::Response: Connection + Send + 'static,
    S::Error: Send + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<S::Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: R) -> Self::Future {
        let probe = current_probe();
        let fut = self.inner.call(req);
        Box::pin(async move {
            let t = Instant::now();
            let res = fut.await;
            let total = t.elapsed();
            if probe.is_some() {
                let (local, proxied) = match &res {
                    Ok(conn) => {
                        let connected = conn.connected();
                        let mut ext = hyper::http::Extensions::new();
                        connected.get_extras(&mut ext);
                        (ext.get::<HttpInfo>().map(HttpInfo::local_addr), connected.is_proxied())
                    }
                    Err(_) => (None, false),
                };
                with_probe(&probe, |p| {
                    let dns = p.dns.take();
                    p.connect = Some(ConnectRecord { total, dns, local, proxied, ok: res.is_ok() });
                });
            }
            res
        })
    }
}

// ---------------------------------------------------------------------------
// Per-request result

/// What the response itself says about its connection.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct ResponseConn {
    pub headers_at: Option<Instant>,
    pub local: Option<SocketAddr>,
    pub remote: Option<SocketAddr>,
}

pub(crate) fn response_conn(resp: &reqwest::Response, at: Instant) -> ResponseConn {
    let info = resp.extensions().get::<HttpInfo>();
    ResponseConn {
        headers_at: Some(at),
        local: info.map(HttpInfo::local_addr),
        remote: info.map(HttpInfo::remote_addr),
    }
}

/// Raw timing of one send (instants). Converted to log fields with [`NetTiming::fields`].
#[derive(Debug, Clone)]
pub struct NetTiming {
    send_at: Instant,
    headers_at: Option<Instant>,
    dns: Option<Duration>,
    /// TCP + TLS (connect minus DNS). For a failed connect: time until it failed.
    connect: Option<Duration>,
    reused: Option<bool>,
    via_proxy: Option<bool>,
    remote_addr: Option<String>,
    upstream_stream: bool,
}

/// Runs `fut` (one upstream send) with a fresh probe and assembles its timing.
pub(crate) async fn measure<T, F>(url: &str, upstream_stream: bool, fut: F) -> (T, NetTiming)
where
    F: Future<Output = (T, ResponseConn)>,
{
    let probe = Arc::new(Mutex::new(Probe::default()));
    let send_at = Instant::now();
    let (out, conn) = PROBE.scope(Arc::clone(&probe), fut).await;
    let record = probe.lock().ok().and_then(|mut p| p.connect.take());
    let host = reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_owned));
    (out, assemble(send_at, upstream_stream, conn, record, host.as_deref()))
}

fn assemble(
    send_at: Instant,
    upstream_stream: bool,
    conn: ResponseConn,
    record: Option<ConnectRecord>,
    host: Option<&str>,
) -> NetTiming {
    // Did this response come over the connection the probe saw being opened?
    let fresh = match (&record, conn.local) {
        (Some(r), Some(local)) => r.ok && r.local == Some(local),
        // No response (send failed): a connect seen in this request is what failed or
        // what the failed request went over.
        (Some(_), None) => conn.headers_at.is_none(),
        (None, _) => false,
    };
    let reused = match (&record, conn.local) {
        _ if fresh => Some(false),
        (_, Some(_)) => Some(true),
        // Failed before any response and no connect seen: unknown.
        _ => None,
    };
    let (dns, connect, via_proxy) = match record.filter(|_| fresh) {
        Some(r) => {
            let dns = r.dns.as_ref().map(|(d, _)| *d);
            let connect = r.total.saturating_sub(dns.unwrap_or_default());
            let resolved_other = match (&r.dns, host) {
                (Some((_, resolved)), Some(h)) => Some(!resolved.eq_ignore_ascii_case(h)),
                _ => None,
            };
            let via_proxy = if r.proxied { Some(true) } else { resolved_other };
            (dns, Some(connect), via_proxy)
        }
        None => (None, None, None),
    };
    NetTiming {
        send_at,
        headers_at: conn.headers_at,
        dns,
        connect,
        reused,
        via_proxy,
        remote_addr: conn.remote.map(|a| a.to_string()),
        upstream_stream,
    }
}

/// Log columns of one attempt (`requests.net_*`). Every field optional: old rows, sends
/// that never happened and anything not measurable are `None`.
///
/// Deliberately no `Default`: every `RequestLogEntry` construction site must say where
/// its timing comes from (normally `ctx.net_log()`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NetTimingFields {
    pub prep_ms: Option<u64>,
    pub dns_ms: Option<u64>,
    pub connect_ms: Option<u64>,
    pub headers_ms: Option<u64>,
    pub conn_reused: Option<bool>,
    pub via_proxy: Option<bool>,
    pub remote_addr: Option<String>,
    pub upstream_stream: Option<bool>,
}

fn ms(d: Duration) -> u64 {
    d.as_millis().min(u64::MAX as u128) as u64
}

impl NetTiming {
    pub fn fields(&self, attempt_start: Instant) -> NetTimingFields {
        NetTimingFields {
            prep_ms: Some(ms(self.send_at.saturating_duration_since(attempt_start))),
            dns_ms: self.dns.map(ms),
            connect_ms: self.connect.map(ms),
            headers_ms: self.headers_at.map(|t| ms(t.saturating_duration_since(attempt_start))),
            conn_reused: self.reused,
            via_proxy: self.via_proxy,
            remote_addr: self.remote_addr.clone(),
            upstream_stream: Some(self.upstream_stream),
        }
    }
}

impl NetTimingFields {
    /// No send happened (or it was not measured).
    pub fn none() -> Self {
        Self {
            prep_ms: None,
            dns_ms: None,
            connect_ms: None,
            headers_ms: None,
            conn_reused: None,
            via_proxy: None,
            remote_addr: None,
            upstream_stream: None,
        }
    }

    /// Network share of the attempt, shown in list rows.
    ///
    /// Streaming upstream: everything up to the response head (local prep, DNS, connect,
    /// round trip, upstream gateway). Non-streaming: the head only arrives after
    /// generation, so only local prep + DNS + connect count.
    pub fn network_ms(&self) -> Option<u64> {
        if self.upstream_stream == Some(true) {
            if let Some(h) = self.headers_ms {
                return Some(h);
            }
        }
        let prep = self.prep_ms?;
        Some(prep + self.dns_ms.unwrap_or(0) + self.connect_ms.unwrap_or(0))
    }

    /// Read from a `requests` row; missing columns are `None`.
    pub(crate) fn from_row(r: &sqlx::sqlite::SqliteRow) -> Self {
        use sqlx::Row;
        let u = |c: &str| r.try_get::<Option<i64>, _>(c).ok().flatten().map(|v| v.max(0) as u64);
        let b = |c: &str| r.try_get::<Option<i64>, _>(c).ok().flatten().map(|v| v != 0);
        Self {
            prep_ms: u("net_prep_ms"),
            dns_ms: u("net_dns_ms"),
            connect_ms: u("net_connect_ms"),
            headers_ms: u("net_headers_ms"),
            conn_reused: b("net_conn_reused"),
            via_proxy: b("net_via_proxy"),
            remote_addr: r.try_get::<Option<String>, _>("net_remote_addr").ok().flatten(),
            upstream_stream: b("net_upstream_stream"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    fn record(local: u16, dns: Option<(u64, &str)>, total: u64) -> ConnectRecord {
        ConnectRecord {
            total: Duration::from_millis(total),
            dns: dns.map(|(d, h)| (Duration::from_millis(d), h.to_owned())),
            local: Some(addr(local)),
            proxied: false,
            ok: true,
        }
    }

    fn resp(local: u16, at: Instant) -> ResponseConn {
        ResponseConn { headers_at: Some(at), local: Some(addr(local)), remote: Some(addr(443)) }
    }

    #[test]
    fn fresh_connection_reports_dns_and_connect() {
        let t0 = Instant::now();
        let send = t0 + Duration::from_millis(10);
        let t = assemble(send, true, resp(5000, send + Duration::from_millis(400)), Some(record(5000, Some((30, "api.x.com")), 230)), Some("api.x.com"));
        let f = t.fields(t0);
        assert_eq!(f.prep_ms, Some(10));
        assert_eq!(f.dns_ms, Some(30));
        assert_eq!(f.connect_ms, Some(200));
        assert_eq!(f.headers_ms, Some(410));
        assert_eq!(f.conn_reused, Some(false));
        assert_eq!(f.via_proxy, Some(false));
        assert_eq!(f.remote_addr.as_deref(), Some("127.0.0.1:443"));
        assert_eq!(f.network_ms(), Some(410));
    }

    #[test]
    fn pooled_connection_is_reused_even_if_a_losing_connect_wrote_the_probe() {
        let t0 = Instant::now();
        let t = assemble(t0, true, resp(6000, t0), Some(record(5000, Some((30, "h")), 230)), Some("h"));
        let f = t.fields(t0);
        assert_eq!(f.conn_reused, Some(true));
        assert_eq!((f.dns_ms, f.connect_ms, f.via_proxy), (None, None, None));
    }

    #[test]
    fn reused_without_any_connect() {
        let t0 = Instant::now();
        let f = assemble(t0, false, resp(6000, t0), None, Some("h")).fields(t0);
        assert_eq!(f.conn_reused, Some(true));
        assert_eq!(f.upstream_stream, Some(false));
    }

    #[test]
    fn failed_connect_keeps_time_until_failure() {
        let t0 = Instant::now();
        let mut r = record(0, Some((20, "h")), 10_020);
        r.ok = false;
        r.local = None;
        let f = assemble(t0, true, ResponseConn::default(), Some(r), Some("h")).fields(t0);
        assert_eq!(f.conn_reused, Some(false));
        assert_eq!(f.dns_ms, Some(20));
        assert_eq!(f.connect_ms, Some(10_000));
        assert_eq!(f.headers_ms, None);
    }

    #[test]
    fn failed_before_connect_is_unknown() {
        let t0 = Instant::now();
        let f = assemble(t0, true, ResponseConn::default(), None, Some("h")).fields(t0);
        assert_eq!(f.conn_reused, None);
        assert_eq!(f.prep_ms, Some(0));
    }

    #[test]
    fn resolving_another_host_means_proxy() {
        let t0 = Instant::now();
        let f = assemble(t0, true, resp(1, t0), Some(record(1, Some((5, "proxy.lan")), 9)), Some("api.x.com")).fields(t0);
        assert_eq!(f.via_proxy, Some(true));
    }

    #[test]
    fn network_ms_for_non_streaming_excludes_waiting() {
        let f = NetTimingFields {
            prep_ms: Some(5),
            dns_ms: Some(10),
            connect_ms: Some(100),
            headers_ms: Some(9000),
            conn_reused: Some(false),
            via_proxy: None,
            remote_addr: None,
            upstream_stream: Some(false),
        };
        assert_eq!(f.network_ms(), Some(115));
        assert_eq!(NetTimingFields::none().network_ms(), None);
    }

    /// End to end over a real socket: first request opens a connection, the second reuses it.
    #[tokio::test]
    async fn real_client_new_then_reused() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            for _ in 0..2 {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                sock.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}")
                    .await
                    .unwrap();
            }
        });
        let client = crate::proxy::upstream::build_client(Duration::from_secs(5)).unwrap();
        // "localhost" goes through the resolver; an IP literal would skip DNS.
        let url = format!("http://localhost:{port}/v1/messages");
        let start = Instant::now();
        let (r1, t1) = crate::proxy::upstream::send_timed(&client, &url, b"{}".to_vec(), Default::default(), false).await;
        assert!(r1.is_ok());
        let f1 = t1.fields(start);
        assert_eq!(f1.conn_reused, Some(false), "{f1:?}");
        assert!(f1.dns_ms.is_some() && f1.connect_ms.is_some(), "{f1:?}");
        assert_eq!(f1.via_proxy, Some(false));
        assert!(f1.remote_addr.is_some());

        let (r2, t2) = crate::proxy::upstream::send_timed(&client, &url, b"{}".to_vec(), Default::default(), false).await;
        assert!(r2.is_ok());
        let f2 = t2.fields(start);
        assert_eq!(f2.conn_reused, Some(true), "{f2:?}");
        assert_eq!((f2.dns_ms, f2.connect_ms), (None, None));
    }
}
