//! 代理 HTTP 服务。绑定 `127.0.0.1`，默认 23456，占用时 +1（§13.3）。

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::routing::post;
use axum::Router;
use tokio::net::TcpListener;
use tracing::{error, info};

use crate::error::{AppError, AppResult};
use crate::proxy::handler;
use crate::state::AppState;

const MAX_PORT_TRIES: u16 = 100;

pub async fn start(state: AppState) -> AppResult<()> {
    let (initial_port, listen_all) = {
        let g = state.settings.read().await;
        (g.proxy_port, g.listen_all)
    };
    let host = if listen_all {
        IpAddr::V4(Ipv4Addr::UNSPECIFIED)
    } else {
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
    };
    let (listener, port) = bind_with_fallback(host, initial_port).await?;
    *state.proxy_port.write().await = port;

    info!(%host, port, "proxy listening");

    let app = Router::new()
        .route("/v1/messages", post(handler::messages))
        .route("/health", axum::routing::get(handler::health))
        .with_state(state.clone());

    if let Err(e) = axum::serve(listener, app).await {
        error!(?e, "axum serve exited");
        return Err(AppError::internal(format!("axum: {e}")));
    }
    Ok(())
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
