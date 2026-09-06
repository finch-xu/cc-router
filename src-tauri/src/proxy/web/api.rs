//! POST /ui/api/cmd/{name}: 把 Tauri command 暴露成 HTTP.
//!
//! `#[tauri::command]` 不改变原函数, 这里拿 `AppHandle::state::<AppState>()` 造出
//! `State<'_, AppState>` 后直接调用原函数, 业务逻辑零复制.
//! 请求体 = 前端 `invoke(name, args)` 的 args 对象 (camelCase 键); 响应 = 返回值 JSON;
//! AppError 用现有 Serialize 实现输出 `{code, message}`, 与 Tauri 拒绝值同形.
//!
//! 加新 command 的规则: lib.rs::invoke_handler 加一行, 这里的 web_commands! 也加一行.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use tauri::Manager;

use crate::commands;
use crate::error::AppError;
use crate::state::AppState;

#[derive(Debug)]
pub enum DispatchError {
    Unknown,
    BadArgs(String),
    Serialize(String),
    Command(AppError),
    NotReady,
}

pub fn status_for(e: &AppError) -> StatusCode {
    match e {
        AppError::ProviderNotFound(_)
        | AppError::EndpointNotFound(_)
        | AppError::SubscriptionNotFound(_)
        | AppError::UnknownVirtualModel(_) => StatusCode::NOT_FOUND,
        AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
        AppError::BadGateway(_) => StatusCode::BAD_GATEWAY,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// 空 body 视为 `{}` (前端 invoke 无参数时); 非对象一律 400.
pub fn parse_body(body: &Bytes) -> Result<Value, String> {
    if body.is_empty() {
        return Ok(json!({}));
    }
    let v: Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    if !v.is_object() {
        return Err("args must be a JSON object".to_string());
    }
    Ok(v)
}

/// 登记表. 语法: `name(arg: Type, ...) => <返回 AppResult<T: Serialize> 的表达式>`.
/// 表达式里可用三个调用方给定的标识符: `st` (State<'_, AppState>), `app` (AppHandle),
/// `args` (按行声明的参数结构体). 用调用方传入的 ident 是为了绕过 macro_rules 卫生性.
macro_rules! web_commands {
    (
        ($st:ident, $app:ident, $args:ident)
        $( $name:ident ( $( $arg:ident : $ty:ty ),* $(,)? ) => $body:expr ),* $(,)?
    ) => {
        pub const REGISTERED: &[&str] = &[ $( stringify!($name) ),* ];

        #[allow(unused_variables, clippy::redundant_closure_call)]
        pub async fn dispatch(
            name: &str,
            raw: Value,
            state: &AppState,
        ) -> Result<Value, DispatchError> {
            match name {
                $(
                    stringify!($name) => {
                        #[derive(serde::Deserialize)]
                        #[serde(rename_all = "camelCase")]
                        #[allow(dead_code)]
                        struct Args { $( $arg: $ty, )* }
                        let $args: Args = serde_json::from_value(raw)
                            .map_err(|e| DispatchError::BadArgs(e.to_string()))?;
                        let handle = state.app_handle.clone();
                        let $st: tauri::State<'_, AppState> = match handle.try_state::<AppState>() {
                            Some(s) => s,
                            None => return Err(DispatchError::NotReady),
                        };
                        let $app: tauri::AppHandle = state.app_handle.clone();
                        let out = $body;
                        match out {
                            Ok(v) => serde_json::to_value(v)
                                .map_err(|e| DispatchError::Serialize(e.to_string())),
                            Err(e) => Err(DispatchError::Command(e)),
                        }
                    }
                )*
                _ => Err(DispatchError::Unknown),
            }
        }
    };
}

use crate::commands::events::EventFilters;
use crate::commands::oauth::{CreateChatGptOAuthSubscriptionInput, CreateKiroSubscriptionInput};
use crate::commands::receipts::ReceiptRange;
use crate::commands::requests::RequestLogFilters;
use crate::commands::statistics::{BreakdownBy, StatsRange};
use crate::commands::subscriptions::{CreateSubscriptionInput, SubscriptionPatch};
use crate::commands::virtual_models::UpdateVirtualModelInput;
use crate::settings::model::SettingsPatch;
use crate::subscription::model::KiroDisguise;
use crate::subscription::quota::TokenQuotas;

web_commands! {
    (st, app, args)
    // providers
    list_providers() => commands::providers::list_providers(st).await,
    // subscriptions
    list_subscriptions() => commands::subscriptions::list_subscriptions(st).await,
    get_subscription(id: String) => commands::subscriptions::get_subscription(st, args.id).await,
    create_subscription(input: CreateSubscriptionInput) => commands::subscriptions::create_subscription(st, args.input).await,
    update_subscription(id: String, patch: SubscriptionPatch) => commands::subscriptions::update_subscription(st, args.id, args.patch).await,
    update_subscription_key(id: String, new_key: String) => commands::subscriptions::update_subscription_key(st, args.id, args.new_key).await,
    delete_subscription(id: String) => commands::subscriptions::delete_subscription(st, args.id).await,
    set_subscription_enabled(id: String, enabled: bool) => commands::subscriptions::set_subscription_enabled(st, args.id, args.enabled).await,
    test_connection(id: String) => commands::subscriptions::test_connection(st, args.id).await,
    refresh_model_list(id: String) => commands::subscriptions::refresh_model_list(st, args.id).await,
    refresh_subscription_balance(id: String) => commands::subscriptions::refresh_subscription_balance(st, args.id).await,
    update_token_quotas(id: String, quotas: TokenQuotas) => commands::subscriptions::update_token_quotas(st, args.id, args.quotas).await,
    reset_total_quota_usage(id: String) => commands::subscriptions::reset_total_quota_usage(st, args.id).await,
    // ChatGPT OAuth
    start_chatgpt_device_flow() => commands::oauth::start_chatgpt_device_flow(st).await,
    poll_chatgpt_device_code(device_code: String) => commands::oauth::poll_chatgpt_device_code(st, args.device_code).await,
    create_chatgpt_oauth_subscription(input: CreateChatGptOAuthSubscriptionInput) => commands::oauth::create_chatgpt_oauth_subscription(st, args.input).await,
    forget_chatgpt_oauth_cache(subscription_id: String) => commands::oauth::forget_chatgpt_oauth_cache(st, args.subscription_id).await,
    get_chatgpt_oauth_usage(subscription_id: String) => commands::oauth::get_chatgpt_oauth_usage(st, args.subscription_id).await,
    // Kiro
    import_kiro_credentials_from_file(path: String) => commands::oauth::import_kiro_credentials_from_file(st, args.path).await,
    import_kiro_credentials_from_text(json: String) => commands::oauth::import_kiro_credentials_from_text(st, args.json).await,
    start_kiro_device_flow(region: Option<String>) => commands::oauth::start_kiro_device_flow(st, args.region).await,
    poll_kiro_device_code(device_code: String) => commands::oauth::poll_kiro_device_code(st, args.device_code).await,
    create_kiro_subscription(input: CreateKiroSubscriptionInput) => commands::oauth::create_kiro_subscription(st, args.input).await,
    forget_kiro_oauth_cache(subscription_id: String) => commands::oauth::forget_kiro_oauth_cache(st, args.subscription_id).await,
    update_kiro_disguise_fields(subscription_id: String, disguise: KiroDisguise) => commands::oauth::update_kiro_disguise_fields(st, args.subscription_id, args.disguise).await,
    // virtual models
    list_virtual_models() => commands::virtual_models::list_virtual_models(st).await,
    update_virtual_model(name: String, input: UpdateVirtualModelInput) => commands::virtual_models::update_virtual_model(st, args.name, args.input).await,
    // request logs
    list_requests(page: u32, page_size: u32, filters: Option<RequestLogFilters>) => commands::requests::list_requests(st, args.page, args.page_size, args.filters).await,
    export_requests_csv(path: String, filters: Option<RequestLogFilters>) => commands::requests::export_requests_csv(st, args.path, args.filters).await,
    export_requests_csv_text(filters: Option<RequestLogFilters>) => commands::requests::export_requests_csv_text(st, args.filters).await,
    list_supported_client_tools() => commands::requests::list_supported_client_tools().await,
    // statistics / receipts / events
    get_overall_stats(range: StatsRange) => commands::statistics::get_overall_stats(st, args.range).await,
    get_daily_series(range: StatsRange) => commands::statistics::get_daily_series(st, args.range).await,
    get_breakdown(range: StatsRange, by: BreakdownBy) => commands::statistics::get_breakdown(st, args.range, args.by).await,
    get_token_heatmap(days: u32) => commands::statistics::get_token_heatmap(st, args.days).await,
    get_receipt_summary(range: ReceiptRange) => commands::receipts::get_receipt_summary(st, args.range).await,
    list_events(page: u32, page_size: u32, filters: Option<EventFilters>) => commands::events::list_events(st, args.page, args.page_size, args.filters).await,
    // settings / proxy
    get_settings() => commands::settings::get_settings(st).await,
    update_settings(patch: SettingsPatch) => commands::settings::update_settings(st, args.patch).await,
    generate_new_token() => commands::settings::generate_new_token(st).await,
    proxy_status() => commands::proxy::proxy_status(st).await,
    env_snippet() => commands::proxy::env_snippet(st).await,
    list_lan_addresses() => commands::proxy::list_lan_addresses().await,
    // onboarding
    get_onboarding_state() => commands::onboarding::get_onboarding_state(st).await,
    complete_onboarding() => commands::onboarding::complete_onboarding(st).await,
    // app
    factory_reset() => commands::app::factory_reset(st, app).await,
    is_appimage_runtime() => Ok::<bool, AppError>(commands::app::is_appimage_runtime()),
    relaunch_app() => { commands::app::relaunch_app(app); Ok::<(), AppError>(()) },
    // debug
    open_debug_dump_dir() => commands::debug::open_debug_dump_dir(app).await,
    clear_debug_dumps() => commands::debug::clear_debug_dumps(app).await,
    // updater
    check_for_update() => commands::updater::check_for_update(app, st).await,
    download_install_update() => commands::updater::download_install_update(app, st).await,
    // TLS
    tls_get_status() => commands::tls::tls_get_status(st).await,
    tls_get_ca_pem_path() => commands::tls::tls_get_ca_pem_path(st).await,
    tls_export_ca_pem(dest: String) => commands::tls::tls_export_ca_pem(st, args.dest).await,
    tls_get_ca_pem_text() => commands::tls::tls_get_ca_pem_text(st).await,
    tls_regenerate_leaf() => commands::tls::tls_regenerate_leaf(st).await,
    // Claude Code / Codex integrations
    read_claude_code_settings() => commands::integrations::read_claude_code_settings(st).await,
    inspect_claude_code_settings() => commands::integrations::inspect_claude_code_settings(st).await,
    write_claude_code_settings(new_content: String) => commands::integrations::write_claude_code_settings(st, args.new_content).await,
    read_codex_config() => commands::integrations::read_codex_config(st).await,
    read_codex_auth() => commands::integrations::read_codex_auth(st).await,
    inspect_codex_config() => commands::integrations::inspect_codex_config(st).await,
    inspect_codex_auth() => commands::integrations::inspect_codex_auth(st).await,
    write_codex_config(new_content: String) => commands::integrations::write_codex_config(st, args.new_content).await,
    write_codex_auth(new_content: String) => commands::integrations::write_codex_auth(st, args.new_content).await,
}

/// POST /ui/api/cmd/{name}
pub async fn dispatch_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    body: Bytes,
) -> Response {
    let raw = match parse_body(&body) {
        Ok(v) => v,
        Err(msg) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"code": "bad_args", "message": msg})),
            )
                .into_response()
        }
    };
    match dispatch(&name, raw, &state).await {
        Ok(v) => (StatusCode::OK, Json(v)).into_response(),
        Err(DispatchError::Unknown) => (
            StatusCode::NOT_FOUND,
            Json(json!({"code": "unknown_command", "message": format!("unknown command: {name}")})),
        )
            .into_response(),
        Err(DispatchError::BadArgs(m)) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"code": "bad_args", "message": m})),
        )
            .into_response(),
        Err(DispatchError::Serialize(m)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"code": "internal", "message": m})),
        )
            .into_response(),
        Err(DispatchError::Command(e)) => (status_for(&e), Json(e)).into_response(),
        Err(DispatchError::NotReady) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"code": "not_ready", "message": "app state not initialized"})),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_has_no_duplicates_and_covers_known_names() {
        let mut names: Vec<&str> = REGISTERED.to_vec();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "web_commands! 有重复条目");
        for must in [
            "list_providers",
            "update_subscription_key",
            "factory_reset",
            "relaunch_app",
            "download_install_update",
            "export_requests_csv_text",
            "tls_get_ca_pem_text",
            "list_lan_addresses",
        ] {
            assert!(names.contains(&must), "{must} 未登记");
        }
    }

    #[test]
    fn status_mapping() {
        assert_eq!(status_for(&AppError::SubscriptionNotFound("x".into())), StatusCode::NOT_FOUND);
        assert_eq!(status_for(&AppError::ProviderNotFound("x".into())), StatusCode::NOT_FOUND);
        assert_eq!(status_for(&AppError::BadRequest("x".into())), StatusCode::BAD_REQUEST);
        assert_eq!(status_for(&AppError::BadGateway("x".into())), StatusCode::BAD_GATEWAY);
        assert_eq!(status_for(&AppError::Internal("x".into())), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn camel_case_args_deserialize() {
        // 与 update_subscription_key(id, new_key) 的 Args 同构; 锁 camelCase 对齐
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Args {
            id: String,
            new_key: String,
        }
        let a: Args = serde_json::from_value(json!({"id": "s1", "newKey": "k"})).unwrap();
        assert_eq!(a.id, "s1");
        assert_eq!(a.new_key, "k");
        assert!(serde_json::from_value::<Args>(json!({"id": "s1", "new_key": "k"})).is_err());
    }

    #[test]
    fn parse_body_empty_is_object() {
        assert_eq!(parse_body(&Bytes::new()).unwrap(), json!({}));
        assert_eq!(parse_body(&Bytes::from_static(b"{\"a\":1}")).unwrap(), json!({"a": 1}));
        assert!(parse_body(&Bytes::from_static(b"not json")).is_err());
        assert!(parse_body(&Bytes::from_static(b"[1]")).is_err(), "args 必须是对象");
    }
}
