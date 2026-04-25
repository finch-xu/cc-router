//! cc-router 桌面 app 入口。
//!
//! 这里的 `run()` 是 Tauri 生命周期起点；桌面壳和代理服务、SQLite、Provider 加载
//! 全部在 `setup()` 中完成初始化。模块粒度见 plan §2。

pub mod commands;
pub mod db;
pub mod error;
pub mod observability;
pub mod provider;
pub mod proxy;
pub mod settings;
pub mod state;
pub mod subscription;
pub mod tray;
pub mod virtual_model;

use std::sync::Arc;

use tauri::Manager;
use tokio::sync::{mpsc, RwLock};
use tracing::{error, info};

use crate::state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            // 日志初始化
            let app_data_dir = app
                .path()
                .app_data_dir()
                .expect("无法解析 app_data_dir");
            observability::logger::init(&app_data_dir)?;
            info!(?app_data_dir, "cc-router starting");

            // 解析 provider YAML 资源目录
            let resource_dir = app
                .path()
                .resource_dir()
                .expect("无法解析 resource_dir");

            // 异步初始化：数据库 + providers + 订阅运行时 + 代理
            let handle = app.handle().clone();
            tauri::async_runtime::block_on(async move {
                match bootstrap(handle.clone(), app_data_dir, resource_dir).await {
                    Ok(state) => {
                        handle.manage(state);
                    }
                    Err(e) => {
                        error!(?e, "bootstrap failed");
                        std::process::exit(1);
                    }
                }
            });

            // 系统托盘
            if let Err(e) = tray::setup(app) {
                error!(?e, "tray setup failed");
            }

            Ok(())
        })
        .on_window_event(tray::on_window_event)
        .invoke_handler(tauri::generate_handler![
            commands::providers::list_providers,
            commands::subscriptions::list_subscriptions,
            commands::subscriptions::get_subscription,
            commands::subscriptions::create_subscription,
            commands::subscriptions::update_subscription,
            commands::subscriptions::update_subscription_key,
            commands::subscriptions::delete_subscription,
            commands::subscriptions::set_subscription_enabled,
            commands::subscriptions::test_connection,
            commands::subscriptions::refresh_model_list,
            commands::virtual_models::list_virtual_models,
            commands::virtual_models::update_virtual_model,
            commands::requests::list_requests,
            commands::settings::get_settings,
            commands::settings::update_settings,
            commands::settings::generate_new_token,
            commands::proxy::proxy_status,
            commands::proxy::env_snippet,
            commands::onboarding::get_onboarding_state,
            commands::onboarding::complete_onboarding,
            commands::app::factory_reset,
        ])
        .run(tauri::generate_context!())
        .expect("运行 cc-router 时发生错误");
}

async fn bootstrap(
    handle: tauri::AppHandle,
    app_data_dir: std::path::PathBuf,
    resource_dir: std::path::PathBuf,
) -> anyhow::Result<AppState> {
    // 1. DB
    let db_path = app_data_dir.join("config.db");
    let pool = db::init_pool(&db_path).await?;
    db::run_migrations(&pool, &resource_dir).await?;

    // 2. Provider YAML 加载
    let providers = provider::loader::load_all(&resource_dir)?;
    info!(provider_count = providers.len(), "providers loaded");

    // 3. Settings
    let mut settings = settings::load_or_default(&app_data_dir).await?;
    // 首次启动或老用户升级时,auth_token 为空 → 生成并立即 save。
    settings::ensure_auth_token(&app_data_dir, &mut settings).await?;

    // 4. 订阅运行时状态初始化
    let subscription_map = subscription::store::load_runtime(&pool).await?;

    // 5. 虚拟模型绑定
    let virtual_models = virtual_model::store::load_all(&pool).await?;

    // 6. 请求日志 channel
    let (log_tx, log_rx) = mpsc::channel(1024);
    let log_pool = pool.clone();
    let log_handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        observability::request_log::run_consumer(log_pool, log_rx, log_handle).await;
    });

    // 7. HTTP client 单例
    // timeout(600s) 是整个 request 生命周期上限(含流式 body),与 Anthropic server-side 上限对齐;
    // 主要在真 hang 时兜底,正常 SSE 客户端断开会通过 client_tx 失败立即释放(见 sse.rs)。
    let http_client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(600))
        .user_agent(concat!("cc-router/", env!("CARGO_PKG_VERSION")))
        .build()?;

    // probe_client: 仅 ping/测试连接用, 短超时, 与生产路径分离
    let probe_client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(concat!("cc-router-probe/", env!("CARGO_PKG_VERSION")))
        .build()?;

    let state = AppState {
        db: pool,
        providers: Arc::new(providers),
        subscriptions: Arc::new(RwLock::new(subscription_map)),
        virtual_models: Arc::new(RwLock::new(virtual_models)),
        settings: Arc::new(RwLock::new(settings)),
        proxy_port: Arc::new(RwLock::new(0)),
        request_log_tx: log_tx,
        http_client,
        probe_client,
        app_handle: handle.clone(),
    };

    // 8. 启动代理
    let proxy_state = state.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = proxy::server::start(proxy_state).await {
            error!(?e, "proxy server stopped");
        }
    });

    // 9. 启动订阅巡检器:每 10min 扫描被虚拟模型引用且异常的订阅, ping 通过则复活
    let recheck_state = state.clone();
    tauri::async_runtime::spawn(async move {
        subscription::recheck_worker::run(recheck_state).await;
    });

    Ok(state)
}
