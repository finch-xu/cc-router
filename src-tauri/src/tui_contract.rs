//! 主 crate ↔ cc-router-tui 的契约。TUI 不依赖主 crate, 它的视图结构体是手写的;
//! 这里把**真实** DTO 序列化后塞进那些结构体, 后端改字段名 / 改枚举值 / 改头名字时当场失败。
//! 新增 TUI 用到的 DTO 时, 在这里加一条。

use std::path::Path;

use cc_router_tui::client::{discovery, dto, http};
use serde::{de::DeserializeOwned, Serialize};

use crate::commands::proxy::ProxyStatus;
use crate::runtime_file::RuntimeFile;
use crate::settings::model::{ProxyMode, Settings};
use crate::subscription::model::{SubscriptionDto, SubscriptionRow, SubscriptionRuntime, SubscriptionState};

fn through_json<T: Serialize, V: DeserializeOwned>(value: &T) -> V {
    let json = serde_json::to_value(value).unwrap();
    serde_json::from_value(json.clone()).unwrap_or_else(|e| panic!("TUI 视图结构体读不了后端的 JSON: {e}\n{json:#}"))
}

#[test]
fn identifier_matches_tauri_conf() {
    let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    assert_eq!(conf["identifier"], discovery::IDENTIFIER);
}

#[test]
fn header_names_match() {
    assert_eq!(crate::proxy::web::local_pass::LOCAL_HEADER, http::LOCAL_HEADER);
    assert_eq!(crate::proxy::web::auth::CSRF_HEADER, http::CSRF_HEADER);
    assert_eq!(crate::runtime_file::FILE_NAME, discovery::RUNTIME_FILE);
}

#[test]
fn runtime_file_is_readable_by_the_tui() {
    let written = RuntimeFile::new(Path::new("/data"), None, Some(23457), "s3cret");
    let read: discovery::RuntimeInfo = through_json(&written);
    assert_eq!(read.pid, written.pid);
    assert_eq!(read.app_version, written.app_version);
    assert_eq!(read.https_port, Some(23457));
    assert_eq!(read.ca_pem_path, written.ca_pem_path);
    assert_eq!(read.local_secret, "s3cret");
    assert_eq!(read.base_url().as_deref(), Some("https://127.0.0.1:23457"));
}

#[test]
fn proxy_status_matches() {
    let real = ProxyStatus {
        port: 23456,
        running: true,
        mode: ProxyMode::Both,
        http_port: Some(23456),
        https_port: Some(23457),
        listen_all: true,
        base_url: "http://127.0.0.1:23456".into(),
    };
    let view: dto::ProxyStatus = through_json(&real);
    assert_eq!(
        view,
        dto::ProxyStatus {
            running: true,
            mode: "both".into(),
            http_port: Some(23456),
            https_port: Some(23457),
            listen_all: true,
            base_url: "http://127.0.0.1:23456".into(),
        }
    );
}

#[test]
fn settings_match() {
    let mut real = Settings::default();
    real.preferred_language = "ja".into();
    real.tui_enabled = true;
    let view: dto::Settings = through_json(&real);
    assert_eq!(view, dto::Settings { preferred_language: "ja".into(), tui_enabled: true });
}

#[test]
fn subscription_matches() {
    let mut row = SubscriptionRow::test_fixture("zhipu", "default");
    row.display_name = "智谱主号".into();
    row.provider_display_name = "智谱".into();
    let id = row.id.to_string();
    let mut rt = SubscriptionRuntime::from_row(row);
    rt.cooldown_until = Some(chrono::DateTime::from_timestamp_millis(1_700_000_000_000).unwrap());
    rt.last_error_message = Some("上游 429".into());
    let real = SubscriptionDto::from_runtime(&rt, Vec::new());
    let view: dto::Subscription = through_json(&real);
    assert_eq!(view.id, id);
    assert_eq!(view.display_name, "智谱主号");
    assert_eq!(view.provider_display_name, "智谱");
    assert!(view.enabled);
    assert_eq!(view.state, dto::SubscriptionState::Healthy);
    assert_eq!(view.cooldown_until, Some(1_700_000_000_000));
    assert_eq!(view.last_error_message.as_deref(), Some("上游 429"));
}

/// 后端的每一个状态, TUI 都必须认得 —— 落到 `Unknown` 说明 TUI 的枚举漏了一个。
#[test]
fn every_backend_state_is_known_to_the_tui() {
    use SubscriptionState::*;
    for state in [Healthy, RateLimited, QuotaExhausted, TransientError, AuthFailed, Disabled] {
        let view: dto::SubscriptionState = through_json(&state);
        assert_ne!(view, dto::SubscriptionState::Unknown, "{state:?}");
    }
}

/// TUI 调用的每个 command 名都必须还在 web_commands! 表里 —— 改名会在这里炸, 而不是在用户终端里变成 unknown_command。
#[test]
fn commands_are_registered() {
    use crate::proxy::web::api::REGISTERED;
    for name in cc_router_tui::client::commands::ALL {
        assert!(REGISTERED.contains(name), "TUI 调用的 command `{name}` 不在 web_commands! 表里");
    }
}
