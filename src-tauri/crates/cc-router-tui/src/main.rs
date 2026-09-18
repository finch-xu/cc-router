//! cc-router-tui 入口。P2a 只有连接自检; 界面在后续阶段接到 `run_ui` 的位置上。

use std::path::PathBuf;
use std::process::ExitCode;

use cc_router_tui::client::discovery::{default_data_dir, Platform};
use cc_router_tui::client::dto::{ProxyStatus, Settings, Subscription, SubscriptionState};
use cc_router_tui::client::{Client, ClientError};
use serde_json::json;

const HELP: &str = "\
cc-router-tui — cc-router 的终端界面

用法: cc-router-tui [选项]

选项:
  --check            连接正在运行的 cc-router 并打印状态, 然后退出
  --data-dir <路径>  指定 cc-router 的数据目录 (默认按系统规则查找)
  -V, --version      打印版本
  -h, --help         打印本帮助
";

struct Args {
    data_dir: Option<PathBuf>,
}

enum Parsed {
    Run(Args),
    Exit(ExitCode),
}

fn parse_args(mut argv: impl Iterator<Item = String>) -> Parsed {
    let mut data_dir = None;
    while let Some(a) = argv.next() {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return Parsed::Exit(ExitCode::SUCCESS);
            }
            "-V" | "--version" => {
                println!("cc-router-tui {}", env!("CARGO_PKG_VERSION"));
                return Parsed::Exit(ExitCode::SUCCESS);
            }
            "--check" => {} // P2a 的唯一模式, 接受该参数是为了让以后的脚本不用改
            "--data-dir" => match argv.next() {
                Some(p) => data_dir = Some(PathBuf::from(p)),
                None => {
                    eprintln!("--data-dir 需要一个路径\n\n{HELP}");
                    return Parsed::Exit(ExitCode::from(2));
                }
            },
            other => {
                eprintln!("未知参数: {other}\n\n{HELP}");
                return Parsed::Exit(ExitCode::from(2));
            }
        }
    }
    Parsed::Run(Args { data_dir })
}

/// 给人看的一句话 + 下一步该做什么。
fn explain(err: &ClientError) -> String {
    match err {
        ClientError::Discovery(e) => format!("{e}\n请先启动 cc-router 桌面 app。"),
        ClientError::NotRunning => "cc-router 未在运行。请先启动桌面 app。".into(),
        ClientError::Disabled => "终端界面未启用。请在桌面 app 的 设置 → 安全与访问 → 终端界面 打开开关。".into(),
        other => other.to_string(),
    }
}

async fn check(args: Args) -> Result<(), ClientError> {
    let data_dir = match args.data_dir {
        Some(d) => d,
        None => default_data_dir(Platform::current(), |k| std::env::var(k).ok())?,
    };
    let client = Client::connect(&data_dir)?;
    let status: ProxyStatus = client.call("proxy_status", json!({})).await?;
    let settings: Settings = client.call("get_settings", json!({})).await?;
    let subs: Vec<Subscription> = client.call("list_subscriptions", json!({})).await?;
    let _events = client.events().await?; // 只验证事件流能建立
    let rt = client.runtime().await;

    let dispatchable = subs.iter().filter(|s| s.state == SubscriptionState::Healthy).count();
    println!("已连接 cc-router {} (pid {})", rt.app_version, rt.pid);
    println!("  地址     {}", status.base_url);
    println!("  模式     {}{}", status.mode, if status.listen_all { " · 监听 0.0.0.0" } else { "" });
    println!("  订阅     {} 个, {} 个可调度", subs.len(), dispatchable);
    println!("  语言     {}", settings.preferred_language);
    println!("  事件流   正常");
    if rt.app_version != env!("CARGO_PKG_VERSION") {
        println!("\n注意: TUI 版本 {} 与 app 版本 {} 不一致。", env!("CARGO_PKG_VERSION"), rt.app_version);
    }
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Parsed::Run(a) => a,
        Parsed::Exit(code) => return code,
    };
    match check(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", explain(&e));
            ExitCode::FAILURE
        }
    }
}
