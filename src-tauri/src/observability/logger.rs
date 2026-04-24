use std::path::Path;
use std::sync::OnceLock;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{filter::EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

static GUARD: OnceLock<WorkerGuard> = OnceLock::new();

pub fn init(app_data_dir: &Path) -> anyhow::Result<()> {
    let log_dir = app_data_dir.join("logs");
    std::fs::create_dir_all(&log_dir)?;
    let file_appender = tracing_appender::rolling::daily(&log_dir, "app.log");
    let (nb, guard) = tracing_appender::non_blocking(file_appender);
    let _ = GUARD.set(guard);

    let env = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,cc_router_lib=debug,cc_router=debug"));

    let stdout = fmt::layer().with_target(true).compact();
    let file = fmt::layer()
        .with_writer(nb)
        .with_ansi(false)
        .with_target(true)
        .compact();

    tracing_subscriber::registry()
        .with(env)
        .with(stdout)
        .with(file)
        .try_init()
        .ok();

    Ok(())
}
