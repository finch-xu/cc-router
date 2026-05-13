//! 代理 HTTP / HTTPS 服务. 监听 127.0.0.1 默认或 0.0.0.0 (listen_all=true),
//! 端口按 ProxyMode 同时启 HTTP / HTTPS / 二者 (双端口). 占用时 +1 最多 100 次.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::routing::post;
use axum::Router;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tracing::{error, info};

use crate::error::{AppError, AppResult};
use crate::proxy::{handler, middleware as cc_middleware};
use crate::state::AppState;

const MAX_PORT_TRIES: u16 = 100;

pub async fn start(state: AppState) -> AppResult<()> {
    let (mode, http_port_pref, https_port_pref, listen_all) = {
        let g = state.settings.read().await;
        (g.proxy_mode, g.proxy_port, g.https_port, g.listen_all)
    };
    let host: IpAddr = if listen_all {
        IpAddr::V4(Ipv4Addr::UNSPECIFIED)
    } else {
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
    };

    let router = build_router(state.clone());

    let mut tasks: Vec<JoinHandle<AppResult<()>>> = Vec::new();

    if mode.includes_http() {
        let (listener, port) = bind_with_fallback(host, http_port_pref).await?;
        *state.http_bound_port.write().await = Some(port);
        info!(%host, port, mode = ?mode, "proxy HTTP listening");
        let r = router.clone();
        tasks.push(tokio::spawn(async move {
            axum::serve(listener, r)
                .await
                .map_err(|e| AppError::internal(format!("axum http: {e}")))
        }));
    }

    if mode.includes_https() {
        let cfg = state
            .tls_config
            .clone()
            .ok_or_else(|| AppError::internal("HTTPS 模式但 TLS config 未初始化"))?;
        // 端口冲突 (HTTP 已抢走 https_port_pref) 由 bind_with_fallback 内置 +1 探测兜底.
        let (listener, port) = bind_with_fallback(host, https_port_pref).await?;
        let std_listener = listener.into_std().map_err(AppError::Io)?;
        std_listener
            .set_nonblocking(true)
            .map_err(AppError::Io)?;
        *state.https_bound_port.write().await = Some(port);
        info!(%host, port, mode = ?mode, "proxy HTTPS listening");
        let r = router.clone();
        tasks.push(tokio::spawn(async move {
            axum_server::from_tcp_rustls(
                std_listener,
                axum_server::tls_rustls::RustlsConfig::from_config(cfg),
            )
            .serve(r.into_make_service())
            .await
            .map_err(|e| AppError::internal(format!("axum-server tls: {e}")))
        }));
    }

    if tasks.is_empty() {
        return Err(AppError::internal("proxy_mode 没有任何 listener 被启用"));
    }

    // 任一 listener 退出整体退出 (panic 拖垮 app 是接受的设计, 见 CLAUDE.md).
    for handle in tasks {
        match handle.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                error!(?e, "proxy listener stopped with error");
                return Err(e);
            }
            Err(join_err) => {
                error!(?join_err, "proxy listener task panicked");
                return Err(AppError::internal(format!("join: {join_err}")));
            }
        }
    }
    Ok(())
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/v1/messages", post(handler::messages))
        .route("/health", axum::routing::get(handler::health))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            cc_middleware::auth_layer,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            cc_middleware::cors_layer,
        ))
        .with_state(state)
}

async fn bind_with_fallback(host: IpAddr, start_port: u16) -> AppResult<(TcpListener, u16)> {
    let mut port = start_port;
    for _ in 0..MAX_PORT_TRIES {
        let addr = SocketAddr::new(host, port);
        match TcpListener::bind(addr).await {
            Ok(listener) => return Ok((listener, port)),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                port = port.saturating_add(1);
                continue;
            }
            Err(e) => return Err(AppError::Io(e)),
        }
    }
    Err(AppError::internal(format!(
        "无法绑定端口 {start_port}..{}",
        start_port.saturating_add(MAX_PORT_TRIES)
    )))
}
