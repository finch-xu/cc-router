//! 代理的 router 与 app 侧钩子. listener 的绑定 / 关停 / 重启在 `listeners.rs` 与
//! `controller.rs`; `AppHooks` 把它们接到 AppState、runtime.json、托盘与界面事件上.

use std::sync::Arc;

use axum::routing::post;
use axum::Router;
use tauri::Emitter;
use tracing::{info, warn};

use crate::error::AppResult;
use crate::proxy::controller::ProxyHooks;
use crate::proxy::listeners::{BoundPorts, ProxyConfig};
use crate::proxy::{handler, middleware as cc_middleware};
use crate::state::AppState;

#[derive(Clone)]
pub struct AppHooks(pub AppState);

impl ProxyHooks for AppHooks {
    async fn desired_config(&self) -> ProxyConfig {
        ProxyConfig::from_settings(&*self.0.settings.read().await)
    }

    async fn tls_config(&self, enable_h2: bool) -> AppResult<Arc<rustls::ServerConfig>> {
        let dir = crate::db::paths::app_data_dir(&self.0.app_handle)?;
        let sans = self.0.settings.read().await.tls_extra_sans.clone();
        crate::tls::load_or_init_server_config(&dir, &sans, enable_h2).await
    }

    fn build_router(&self, cfg: &ProxyConfig) -> Router {
        build_router(self.0.clone(), cfg.body_limit_bytes())
    }

    async fn on_bound(&self, ports: Option<BoundPorts>) {
        let p = ports.unwrap_or_default();
        *self.0.http_bound_port.write().await = p.http;
        *self.0.https_bound_port.write().await = p.https;
        // 停止时不删 runtime.json: TUI 连不上会重读, 读到旧端口照样连不上, 按「未运行」处理.
        if ports.is_some() {
            write_runtime_file(&self.0).await;
        }
        crate::tray::refresh(&self.0.app_handle);
        let _ = self.0.app_handle.emit("proxy_restarted", ());
    }
}

fn build_router(state: AppState, body_limit: usize) -> Router {
    Router::new()
        .route("/v1/messages", post(handler::messages))
        .route("/v1/messages/count_tokens", post(handler::count_tokens))
        // Claude Code's best-effort connection warm-up probe.
        .route("/api/hello", axum::routing::head(|| async { axum::http::StatusCode::OK }))
        .route("/v1/responses", post(handler::responses))
        .route("/v1/chat/completions", post(handler::chat_completions))
        .route("/v1/systemone", post(handler::systemone))
        .route("/v1/models", axum::routing::get(handler::models))
        .route("/health", axum::routing::get(handler::health))
        // 显式兜底 404: 不加这行, 下面 merge 网页路由时 axum 会用网页子路由的
        // (缺省) fallback 覆盖主路由的缺省 fallback, 未注册的代理路径就绕开了
        // cors_layer —— 未知路径的 OPTIONS 预检会收到裸 404 而不是带 CORS 头的 204.
        .fallback(|| async { axum::http::StatusCode::NOT_FOUND })
        // axum 对 Bytes extractor 默认 2 MiB 上限, Codex 多图 base64 请求会 413
        // (issue #41); 上限来自生效配置快照, 改动点「重启代理服务」生效
        .layer(axum::extract::DefaultBodyLimit::max(body_limit))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            cc_middleware::auth_layer,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            cc_middleware::cors_layer,
        ))
        // 网页界面必须在三个 layer 之后 merge: axum 的 layer 只作用于它之前注册的路由,
        // /ui 子树由此绕开代理 token 校验与通配 CORS (安全前提, 见 proxy/web/mod.rs)。
        // merge 时若两边都设了 fallback 会 panic, 若只有一边设了则以那边为准 —— 这里主路由
        // 已在上面显式设置 (且已过三个 layer), 因此合并后仍是主路由的兜底, 继续经过 CORS。
        .merge(crate::proxy::web::router(state.clone()))
        .with_state(state)
}

async fn write_runtime_file(state: &AppState) {
    let app_data_dir = match crate::db::paths::app_data_dir(&state.app_handle) {
        Ok(d) => d,
        Err(e) => {
            warn!(?e, "无法解析 app_data_dir, 跳过 runtime.json");
            return;
        }
    };
    // 与托盘 `tray::TrayLocale::from_pref` 读的是同一个 API: 原始标签直接下发给 TUI, 映射规则
    // (zh*/ja*/其余) 由 TUI 侧的 `Lang::resolve` 做, 这里不解析。偏好语言只在这里取一次:
    // `update_settings` 改语言时不重写 runtime.json (写入点只有代理绑定成功这一处: 启动 / 重启 / 回滚)。
    let preferred_language = state.settings.read().await.preferred_language.clone();
    let file = crate::runtime_file::RuntimeFile::new(
        &app_data_dir,
        *state.http_bound_port.read().await,
        *state.https_bound_port.read().await,
        &state.local_secret,
        tauri_plugin_os::locale(),
    )
    .with_preferred_language(Some(preferred_language));
    // 同步小文件写入, 只在绑定成功时发生, 不值得 spawn_blocking。
    match crate::runtime_file::write(&app_data_dir, &file) {
        Ok(()) => info!(http = ?file.http_port, https = ?file.https_port, "runtime.json written"),
        Err(e) => warn!(?e, "runtime.json 写入失败, cc-router-tui 将无法连接"),
    }
}
