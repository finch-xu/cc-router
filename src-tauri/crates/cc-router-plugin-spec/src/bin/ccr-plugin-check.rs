//! 社区仓库 CI 用的插件检查: `ccr-plugin-check <插件目录>`。
//!
//! 输出一份 JSON (`CheckResult`, 或主题 / 小票的 `{"delegated":"ts"}`), 有问题时退出码 1。
//! 主题与小票由社区 CI 现有的 TS 校验器负责, 这里只认出类型后让路。

use std::path::Path;
use std::process::ExitCode;

use cc_router_plugin_spec::check::{CheckResult, Issue, IssueCode};
use cc_router_plugin_spec::plugin;

fn main() -> ExitCode {
    let Some(dir) = std::env::args().nth(1) else {
        eprintln!("usage: ccr-plugin-check <plugin-dir>");
        return ExitCode::from(2);
    };
    let out = match plugin::check_dir(Path::new(&dir)) {
        plugin::DirCheck::Delegated => serde_json::json!({ "delegated": "ts" }),
        plugin::DirCheck::Checked(result) => serde_json::to_value(&result).expect("CheckResult serializes"),
        plugin::DirCheck::Unreadable(msg) => serde_json::to_value(CheckResult::failed(vec![
            Issue::new(plugin::MANIFEST_FILE, IssueCode::Missing).detail(msg),
        ]))
        .expect("CheckResult serializes"),
    };
    println!("{}", serde_json::to_string_pretty(&out).expect("json"));
    if out.get("ok") == Some(&serde_json::Value::Bool(false)) {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
