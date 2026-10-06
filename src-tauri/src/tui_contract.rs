//! 主 crate ↔ cc-router-tui 的契约。TUI 不依赖主 crate, 它的视图结构体是手写的;
//! 这里把**真实** DTO 序列化后塞进那些结构体, 后端改字段名 / 改枚举值 / 改头名字时当场失败。
//! 新增 TUI 用到的 DTO 时, 在这里加一条。

use std::collections::HashMap;
use std::path::Path;

use cc_router_tui::client::{discovery, dto, http};
use cc_router_tui::i18n::Lang;
use cc_router_tui::secret::Secret;
use serde::{de::DeserializeOwned, Serialize};

use crate::commands::providers::ProviderInfo;
use crate::commands::proxy::ProxyStatus;
use crate::commands::requests::{ListRequestsResult, RequestLogDto, RequestLogFilters};
use crate::commands::statistics::{DailySeriesPointDto, OverallStatsDto, StatsRange};
use crate::commands::subscriptions::{
    CreateSource, CreateSubscriptionInput, CustomProtocol, ProbeCustomModelsInput, ProbeCustomModelsResult, RefreshBalanceResult,
    RefreshModelListResult, SubscriptionPatch, TestConnectionResult, ALLOWED_SLOT_EFFORTS,
};
use crate::commands::virtual_models::{UpdateVirtualModelInput, VirtualModelDto};
use crate::observability::request_log::RequestStatus;
use crate::provider::model::{AuthHeaderFormat, AuthType};
use crate::provider::Provider;
use crate::runtime_file::RuntimeFile;
use crate::settings::model::{ProxyMode, Settings};
use crate::subscription::model::{
    BalanceEntry, BalanceSeverity, BalanceSnapshot, ModelCache, ModelInfo, SubscriptionDto, SubscriptionRow, SubscriptionRuntime,
    SubscriptionState,
};
use crate::subscription::quota::{TokenQuotas, ALL_PERIODS};
use crate::virtual_model::model::RoutingMode;

fn through_json<T: Serialize, V: DeserializeOwned>(value: &T) -> V {
    let json = serde_json::to_value(value).unwrap();
    serde_json::from_value(json.clone()).unwrap_or_else(|e| panic!("TUI 视图结构体读不了后端的 JSON: {e}\n{json:#}"))
}

/// TUI `CustomProtocol` → 后端 `CustomProtocol` 的期望映射, 写成对 TUI 枚举的穷尽 `match`——
/// TUI 加协议变体时这里会 `E0004`, 逼着当场决定后端对应哪个变体, 而不是遗漏在某个手写列表里。
/// `create_subscription_input_matches` 与 `custom_protocol_wire_names_round_trip`
/// 共用这一份映射。
fn expected_backend_protocol(p: dto::CustomProtocol) -> CustomProtocol {
    match p {
        dto::CustomProtocol::Anthropic => CustomProtocol::Anthropic,
        dto::CustomProtocol::Gemini => CustomProtocol::Gemini,
        dto::CustomProtocol::OpenaiResponses => CustomProtocol::OpenaiResponses,
        dto::CustomProtocol::OpenaiChatCompletions => CustomProtocol::OpenaiChatCompletions,
        dto::CustomProtocol::GeminiInteractions => CustomProtocol::GeminiInteractions,
    }
}

/// 后端 `AuthType` → 是否属于「TUI 该置灰」的 OAuth 类, 写成对后端枚举的穷尽 `match`——后端加
/// 新变体时这里会 `E0004`, 逼着当场决定它算不算 OAuth, 而不是悄悄漏在 `OAUTH_AUTH_TYPES` 之外
/// 让 TUI 把它当普通 api_key 厂商展示。
fn is_oauth_auth_type(auth: AuthType) -> bool {
    match auth {
        AuthType::ChatgptOauth | AuthType::KiroOauth => true,
        AuthType::ApiKey
        | AuthType::GeminiApiKey
        | AuthType::OpenaiResponsesApiKey
        | AuthType::OpenaiChatCompletionsApiKey
        | AuthType::GeminiInteractionsApiKey => false,
    }
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
    let written = RuntimeFile::new(Path::new("/data"), None, Some(23457), "s3cret", None);
    let read: discovery::RuntimeInfo = through_json(&written);
    assert_eq!(read.pid, written.pid);
    assert_eq!(read.app_version, written.app_version);
    assert_eq!(read.https_port, Some(23457));
    assert_eq!(read.ca_pem_path, written.ca_pem_path);
    assert_eq!(read.local_secret, "s3cret");
    assert_eq!(read.base_url().as_deref(), Some("https://127.0.0.1:23457"));
    assert_eq!(read.system_locale, None);
}

/// `system_locale` 的 Some / None 两种形状都能从主 crate 序列化结果里被 TUI `RuntimeInfo`
/// 读回来——`Some` 时原样保留, `None` 时字段被省略 (`skip_serializing_if`) 但 TUI 的
/// `#[serde(default)]` 照样把缺失字段读成 `None`, 不是解析失败。
#[test]
fn runtime_file_system_locale_reaches_the_tui() {
    let with_locale = RuntimeFile::new(Path::new("/data"), Some(23456), None, "s3cret", Some("zh-Hans-CN".into()));
    let read: discovery::RuntimeInfo = through_json(&with_locale);
    assert_eq!(read.system_locale.as_deref(), Some("zh-Hans-CN"));

    let without_locale = RuntimeFile::new(Path::new("/data"), Some(23456), None, "s3cret", None);
    let read: discovery::RuntimeInfo = through_json(&without_locale);
    assert_eq!(read.system_locale, None);
}

/// `preferred_language` 同样两种形状都能到达 TUI: 旧版桌面端写的文件没有这个字段 (TUI 读成
/// `None`, 连接前语言退回 `system_locale`)。
#[test]
fn runtime_file_preferred_language_reaches_the_tui() {
    let with_pref = RuntimeFile::new(Path::new("/data"), Some(23456), None, "s3cret", Some("zh-Hans-CN".into()))
        .with_preferred_language(Some("ja".into()));
    let read: discovery::RuntimeInfo = through_json(&with_pref);
    assert_eq!(read.preferred_language.as_deref(), Some("ja"));
    assert_eq!(read.system_locale.as_deref(), Some("zh-Hans-CN"));

    let without_pref = RuntimeFile::new(Path::new("/data"), Some(23456), None, "s3cret", None);
    let read: discovery::RuntimeInfo = through_json(&without_pref);
    assert_eq!(read.preferred_language, None);
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
        restart_pending: false,
        applied: None,
        last_error: None,
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
    real.auth_enabled = true;
    let view: dto::Settings = through_json(&real);
    assert_eq!(view, dto::Settings { preferred_language: "ja".into(), tui_enabled: true, auth_enabled: true });
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
    let registry = HashMap::from([("zhipu".to_string(), zhipu_provider())]);
    let real = SubscriptionDto::from_runtime(&rt, Vec::new(), &registry);
    let view: dto::Subscription = through_json(&real);
    assert_eq!(view.id, id);
    assert_eq!(view.display_name, "智谱主号");
    assert_eq!(view.provider_display_name, "智谱");
    assert_eq!(view.provider_name(Lang::En), "Zhipu GLM", "provider_names 被改名会静默落回中文快照");
    assert_eq!(view.provider_name(Lang::Ja), "Zhipu GLM（日）");
    // 注册表里查不到 (自定义订阅 / yaml 已删) 就用快照。
    let orphan: dto::Subscription = through_json(&SubscriptionDto::from_runtime(&rt, Vec::new(), &HashMap::new()));
    assert_eq!(orphan.provider_name(Lang::En), "智谱");
    assert!(view.enabled);
    assert_eq!(view.state, dto::SubscriptionState::Healthy);
    assert_eq!(view.cooldown_until, Some(1_700_000_000_000));
    assert_eq!(view.last_error_message.as_deref(), Some("上游 429"));
    // 冷却时间在过去 + 健康 + 启用 + 没设限额 → 可调度
    assert!(view.is_dispatchable);
    assert_eq!(view.quota_usage.len(), 4, "后端固定给 4 个周期");
    assert!(view.tightest_quota().is_none(), "没设上限的周期不参与");
    assert_eq!(view.provider_id, "zhipu");
    assert_eq!(view.base_url, "");
    assert_eq!(view.auth_type, "api_key");
    assert_eq!(view.model_slots.sonnet, "b");
    assert!(view.referenced_by.is_empty());
}

/// 订阅详情页要用的字段: 槽位级 effort 覆盖、兜底槽、referenced_by、model_cache、balance_cache。
#[test]
fn subscription_detail_fields_match() {
    let mut row = SubscriptionRow::test_fixture("zhipu", "default");
    row.slot_efforts.opus = Some("high".into());
    row.model_slots.fallback = "x".into();
    let mut rt = SubscriptionRuntime::from_row(row);
    rt.model_cache = Some(ModelCache {
        fetched_at: chrono::Utc::now(),
        models: vec![
            ModelInfo { id: "glm-4.6".into(), display_name: Some("GLM-4.6".into()) },
            ModelInfo { id: "glm-4.7".into(), display_name: None },
        ],
    });
    rt.balance_cache = Some(BalanceSnapshot {
        is_available: Some(true),
        entries: vec![BalanceEntry {
            label: "余额".into(),
            value_text: "9.99".into(),
            unit: "CNY".into(),
            hint: None,
            severity: BalanceSeverity::Low,
            detail: Some(crate::subscription::model::BalanceDetail::TopupGranted {
                topped_up: "9.99".into(),
                granted: "0.00".into(),
            }),
        }],
        fetched_at: chrono::Utc::now(),
    });
    let real = SubscriptionDto::from_runtime(&rt, vec!["model-sonnet".into()], &HashMap::new());
    let view: dto::Subscription = through_json(&real);

    assert_eq!(view.slot_efforts.opus.as_deref(), Some("high"));
    assert_eq!(view.model_slots.fallback, "x");
    assert_eq!(view.referenced_by, vec!["model-sonnet".to_string()]);
    let model_cache = view.model_cache.expect("model_cache 应该有值");
    assert_eq!(model_cache.models.len(), 2);
    let balance_cache = view.balance_cache.expect("balance_cache 应该有值");
    assert_eq!(balance_cache.snapshot.entries.len(), 1);
    assert_eq!(balance_cache.snapshot.entries[0].severity, dto::BalanceSeverity::Low);
}

/// 后端的每一个余额告警级别, TUI 都必须认得 —— 落到 `Unknown` 说明 TUI 的枚举漏了一个。
#[test]
fn every_backend_balance_severity_is_known_to_the_tui() {
    for severity in [BalanceSeverity::Normal, BalanceSeverity::Low, BalanceSeverity::Critical] {
        let view: dto::BalanceSeverity = through_json(&severity);
        assert_ne!(view, dto::BalanceSeverity::Unknown, "{severity:?}");
    }
}

/// `auth_type` 在 TUI 侧按字符串收 (只用于显示) —— 后端 7 个变体都必须序列化成普通字符串,
/// 不能是带 tag 的对象, 否则 `dto::Subscription::auth_type: String` 解析失败。
#[test]
fn every_backend_auth_type_is_a_plain_string_for_the_tui() {
    use AuthType::*;
    for auth in [
        ApiKey,
        ChatgptOauth,
        KiroOauth,
        GeminiApiKey,
        OpenaiResponsesApiKey,
        OpenaiChatCompletionsApiKey,
        GeminiInteractionsApiKey,
    ] {
        let value = serde_json::to_value(auth).unwrap();
        assert!(value.is_string(), "{auth:?} 没有序列化成字符串: {value:?}");
    }
}

#[test]
fn test_connection_result_matches() {
    let real = TestConnectionResult {
        ok: true,
        message: "连接成功".into(),
        http_status: Some(200),
        model_used: Some("glm-4.6".into()),
        state_reset: true,
        // 桌面端专用的结构化字段; TUI 视图不认它, 反序列化必须照样成功。
        note: Some(crate::subscription::ping::ProbeNote::Ok),
    };
    let view: dto::TestConnectionResult = through_json(&real);
    assert_eq!(
        view,
        dto::TestConnectionResult {
            ok: true,
            message: "连接成功".into(),
            http_status: Some(200),
            model_used: Some("glm-4.6".into()),
            state_reset: true,
        }
    );

    // 网络错误时两个 Option 字段都缺省。
    let real_absent = TestConnectionResult { ok: false, message: "网络错误".into(), http_status: None, model_used: None, state_reset: false, note: None };
    let view_absent: dto::TestConnectionResult = through_json(&real_absent);
    assert_eq!(
        view_absent,
        dto::TestConnectionResult { ok: false, message: "网络错误".into(), http_status: None, model_used: None, state_reset: false }
    );
}

#[test]
fn refresh_model_list_result_matches() {
    let auto = RefreshModelListResult::Auto {
        models: vec![ModelInfo { id: "glm-4.6".into(), display_name: None }],
        fetched_at: 1_700_000_000_000,
    };
    let view: dto::RefreshModelsResult = through_json(&auto);
    assert_eq!(
        view,
        dto::RefreshModelsResult::Auto {
            models: vec![dto::ModelInfo { id: "glm-4.6".into(), display_name: None }],
            fetched_at: 1_700_000_000_000,
        }
    );

    let manual = RefreshModelListResult::ManualFallback { reason: "provider 未声明 model_discovery".into() };
    let view: dto::RefreshModelsResult = through_json(&manual);
    assert_eq!(view, dto::RefreshModelsResult::ManualFallback { reason: "provider 未声明 model_discovery".into() });
}

#[test]
fn refresh_balance_result_matches() {
    let success = RefreshBalanceResult::Success {
        snapshot: BalanceSnapshot { is_available: Some(true), entries: vec![], fetched_at: chrono::Utc::now() },
        fetched_at: 1_700_000_000_000,
    };
    let view: dto::RefreshBalanceResult = through_json(&success);
    assert_eq!(
        view,
        dto::RefreshBalanceResult::Success {
            snapshot: dto::BalanceSnapshot { is_available: Some(true), entries: vec![] },
            fetched_at: 1_700_000_000_000,
        }
    );

    let failed = RefreshBalanceResult::Failed { reason: "网络超时".into() };
    let view: dto::RefreshBalanceResult = through_json(&failed);
    assert_eq!(view, dto::RefreshBalanceResult::Failed { reason: "网络超时".into() });

    let unsupported = RefreshBalanceResult::Unsupported;
    let view: dto::RefreshBalanceResult = through_json(&unsupported);
    assert_eq!(view, dto::RefreshBalanceResult::Unsupported);
}

#[test]
fn subscription_over_its_quota_matches() {
    let mut row = SubscriptionRow::test_fixture("zhipu", "default");
    row.token_quotas = TokenQuotas { daily: Some(1000), monthly: Some(100), ..Default::default() };
    let mut rt = SubscriptionRuntime::from_row(row);
    rt.quota_usage.add(chrono::Utc::now(), 60, 30, 5, 5);
    let view: dto::Subscription = through_json(&SubscriptionDto::from_runtime(&rt, Vec::new(), &HashMap::new()));
    assert!(!view.is_dispatchable);
    let tight = view.tightest_quota().expect("设了两个上限");
    assert_eq!(tight.period, dto::QuotaPeriod::Monthly);
    assert_eq!((tight.limit, tight.used(), tight.exceeded), (Some(100), 100, true));
    let daily = view.quota_usage.iter().find(|q| q.period == dto::QuotaPeriod::Daily).unwrap();
    assert_eq!((daily.ratio(), daily.exceeded), (Some(0.1), false));
}

#[test]
fn every_backend_quota_period_is_known_to_the_tui() {
    for period in ALL_PERIODS {
        let view: dto::QuotaPeriod = through_json(&period);
        assert_ne!(view, dto::QuotaPeriod::Unknown, "{period:?}");
    }
}

#[test]
fn overall_stats_match() {
    let real = OverallStatsDto {
        total_requests: 1284,
        success_count: 1266,
        error_count: 15,
        timeout_count: 3,
        success_rate_pct: 98.6,
        avg_duration_ms: Some(1234.5),
        p95_duration_ms: Some(4000),
        total_input_tokens: 1,
        total_output_tokens: 20,
        total_cache_creation_tokens: 300,
        total_cache_read_tokens: 4000,
        total_tool_use_count: 7,
        tool_use_request_count: 5,
    };
    let view: dto::OverallStats = through_json(&real);
    assert_eq!((view.total_requests, view.success_rate_pct, view.total_tokens()), (1284, 98.6, 4321));
}

#[test]
fn hourly_series_point_matches() {
    let real = DailySeriesPointDto {
        day: "2026-09-18".into(),
        hour: Some(13),
        request_count: 42,
        success_count: 40,
        error_count: 2,
        timeout_count: 0,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_cache_creation_tokens: 0,
        total_cache_read_tokens: 0,
        avg_duration_ms: None,
    };
    let view: dto::SeriesPoint = through_json(&real);
    assert_eq!(view, dto::SeriesPoint { hour: Some(13), request_count: 42 });
    assert_eq!(dto::hourly_buckets(&[view])[13], 42);
}

/// TUI 发的是 `{"range":"today"}`。
#[test]
fn stats_range_today_is_what_the_tui_sends() {
    let parsed: StatsRange = serde_json::from_value(serde_json::json!("today")).unwrap();
    assert_eq!(parsed, StatsRange::Today);
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

/// 三种调度模式各自序列化后能被 TUI 的 `dto::VirtualModel` 解析, 字段值原样保留。
#[test]
fn virtual_model_dto_matches() {
    for (mode, expected) in [
        (RoutingMode::Sequential, dto::RoutingMode::Sequential),
        (RoutingMode::RoundRobin, dto::RoutingMode::RoundRobin),
        (RoutingMode::Sticky, dto::RoutingMode::Sticky),
    ] {
        let real = VirtualModelDto {
            name: "model-sonnet".into(),
            mode,
            subscription_ids: vec!["11111111-1111-1111-1111-111111111111".into()],
        };
        let view: dto::VirtualModel = through_json(&real);
        assert_eq!(view.name, "model-sonnet");
        assert_eq!(view.mode, expected, "{mode:?}");
        assert_eq!(view.subscription_ids, vec!["11111111-1111-1111-1111-111111111111".to_string()]);
    }
}

/// 后端的每一个调度模式, TUI 都必须认得 —— 落到 `Unknown` 说明 TUI 的枚举漏了一个。
#[test]
fn every_backend_routing_mode_is_known_to_the_tui() {
    for mode in [RoutingMode::Sequential, RoutingMode::RoundRobin, RoutingMode::Sticky] {
        let view: dto::RoutingMode = through_json(&mode);
        assert_ne!(view, dto::RoutingMode::Unknown, "{mode:?}");
    }
}

/// `runtime.rs` 直接拿 `RoutingMode::as_wire()` 的返回值 (而不是走 `Serialize`) 拼请求体
/// (见该文件 `call_mutation` 里 `Mutation::UpdateVirtualModel` 分支的注释) —— 这条测试锁住
/// `as_wire()` 吐出来的字符串仍然是后端 `RoutingMode` 认得的线上名字。「咬合检查」: 把
/// `RoutingMode::RoundRobin::as_wire()` 临时改成 "roundrobin" 应该让这条测试炸。
#[test]
fn routing_mode_wire_names_round_trip() {
    for (tui_mode, expected_real) in [
        (dto::RoutingMode::Sequential, RoutingMode::Sequential),
        (dto::RoutingMode::RoundRobin, RoutingMode::RoundRobin),
        (dto::RoutingMode::Sticky, RoutingMode::Sticky),
    ] {
        let wire = tui_mode.as_wire();
        let real: RoutingMode = serde_json::from_value(serde_json::Value::String(wire.to_string()))
            .unwrap_or_else(|e| panic!("后端 RoutingMode 读不了 TUI 发的线上名字 {wire:?}: {e}"));
        assert_eq!(
            serde_json::to_value(real).unwrap(),
            serde_json::to_value(expected_real).unwrap(),
            "{wire:?} 应该解析回 {expected_real:?}"
        );
    }
}

/// 用 TUI 的 `dto::ModelSlots` / `dto::SlotEfforts` 序列化出一份 `update_subscription` 的 patch,
/// 后端 `SubscriptionPatch` 必须原样反序列化——auto 槽位 (字段缺失) 读成 `None`, 带值的槽位原样,
/// `fallback` 空串照常出现 (不是 `Option`)。
#[test]
fn update_subscription_patch_from_the_tui_deserializes() {
    let model_slots = dto::ModelSlots { fable: "f".into(), opus: "o".into(), sonnet: "s".into(), haiku: "h".into(), fallback: String::new(), jev: String::new() };
    let mut slot_efforts = dto::SlotEfforts::default();
    slot_efforts.set(dto::Slot::Opus, Some("high".into()));

    let patch_json = serde_json::json!({ "model_slots": model_slots, "slot_efforts": slot_efforts });
    let patch: SubscriptionPatch = serde_json::from_value(patch_json).unwrap();

    let slots = patch.model_slots.expect("model_slots 应该有值");
    assert_eq!(slots.fable, "f");
    assert_eq!(slots.opus, "o");
    assert_eq!(slots.sonnet, "s");
    assert_eq!(slots.haiku, "h");
    assert_eq!(slots.fallback, "", "fallback 是普通 String, 空值也该照常出现");

    let efforts = patch.slot_efforts.expect("slot_efforts 应该有值");
    assert_eq!(efforts.opus.as_deref(), Some("high"));
    assert_eq!(efforts.fable, None, "auto 槽位应该读成 None");
    assert_eq!(efforts.sonnet, None);
    assert_eq!(efforts.haiku, None);
}

/// `update_virtual_model` 的 `input` 用 TUI 侧的 `as_wire()` 拼出来, 后端 `UpdateVirtualModelInput`
/// 必须原样反序列化。
#[test]
fn update_virtual_model_input_from_the_tui_deserializes() {
    let input_json = serde_json::json!({
        "mode": dto::RoutingMode::RoundRobin.as_wire(),
        "subscription_ids": ["11111111-1111-1111-1111-111111111111"],
    });
    let input: UpdateVirtualModelInput = serde_json::from_value(input_json).unwrap();
    assert_eq!(
        serde_json::to_value(input.mode).unwrap(),
        serde_json::to_value(RoutingMode::RoundRobin).unwrap()
    );
    assert_eq!(input.subscription_ids, vec!["11111111-1111-1111-1111-111111111111".to_string()]);
}

/// TUI 的槽位 effort 选项必须与后端白名单逐字相同, 否则 TUI 会提供一个后端拒绝的档位
/// (或者漏掉一个后端接受的档位)。
#[test]
fn tui_effort_choices_equal_the_backend_allowlist() {
    assert_eq!(dto::EFFORT_CHOICES.as_slice(), ALLOWED_SLOT_EFFORTS);
}

/// TUI 调用的每个 command 名都必须还在 web_commands! 表里 —— 改名会在这里炸, 而不是在用户终端里变成 unknown_command。
#[test]
fn commands_are_registered() {
    use crate::proxy::web::api::REGISTERED;
    for name in cc_router_tui::client::commands::ALL {
        assert!(REGISTERED.contains(name), "TUI 调用的 command `{name}` 不在 web_commands! 表里");
    }
}

/// TUI 关心的每个事件名都必须还在后端 `BRIDGED_EVENTS` 里 —— 否则事件发生了但永远推不到
/// TUI, `/ui/api/events` 那条 SSE 连接上什么都不会来。
#[test]
fn tui_event_names_are_bridged() {
    for name in cc_router_tui::client::events::ALL {
        assert!(
            crate::proxy::web::events::BRIDGED_EVENTS.contains(name),
            "TUI 关心的事件 `{name}` 不在后端 BRIDGED_EVENTS 里"
        );
    }
}

/// `route_attempt_payload` 的线上形状: started 的键恰好是 `subscription_id` /
/// `virtual_model` (没有 `success`), finished 多一个 `success`; 两者都能解析成 `dto::RouteAttempt`。
#[test]
fn route_attempt_payloads_match() {
    use crate::proxy::pipeline::route_attempt_payload;
    use crate::virtual_model::model::VirtualModelName;

    let id = uuid::Uuid::nil();

    let started = route_attempt_payload(id, VirtualModelName::Sonnet, None);
    let mut started_keys: Vec<&str> = started.as_object().unwrap().keys().map(String::as_str).collect();
    started_keys.sort_unstable();
    assert_eq!(started_keys, vec!["subscription_id", "virtual_model"], "started payload 不该带 success 键");

    let finished = route_attempt_payload(id, VirtualModelName::Sonnet, Some(true));
    let mut finished_keys: Vec<&str> = finished.as_object().unwrap().keys().map(String::as_str).collect();
    finished_keys.sort_unstable();
    assert_eq!(finished_keys, vec!["subscription_id", "success", "virtual_model"]);

    let started_view: dto::RouteAttempt = through_json(&started);
    assert_eq!(started_view.virtual_model, "model-sonnet");
    assert_eq!(started_view.success, None);

    let finished_view: dto::RouteAttempt = through_json(&finished);
    assert_eq!(finished_view.virtual_model, "model-sonnet");
    assert_eq!(finished_view.success, Some(true));
}

/// `RequestLogDto` 全部字段为 `Some` 时, `dto::RequestLog` 逐字段接住。
#[test]
fn request_log_matches() {
    let real = RequestLogDto {
        id: "r1".into(),
        timestamp: 1_700_000_000_000,
        virtual_model_name: "model-sonnet".into(),
        subscription_id: "s1".into(),
        provider_id: "zhipu".into(),
        endpoint_id: "default".into(),
        real_model_name: "glm-4.6".into(),
        response_model_name: Some("glm-4.6-vision".into()),
        is_streaming: true,
        status: RequestStatus::Success.as_str().to_string(),
        http_status: Some(200),
        total_latency_ms: Some(1234),
        input_tokens: Some(10),
        output_tokens: Some(20),
        cache_creation_tokens: Some(30),
        cache_read_tokens: Some(40),
        error_message: Some("boom".into()),
        upstream_response_body: Some("{}".into()),
        client_tool: Some("claude_code".into()),
        client_user_agent: Some("ua".into()),
        client_version: Some("1.2.3".into()),
        client_ip: Some("127.0.0.1".into()),
        entry_kind: Some("messages".into()),
        downstream_http_version: Some("HTTP/1.1".into()),
        client_effort: Some("high".into()),
        effective_effort: Some("high".into()),
        effort_source: Some("client".into()),
        upstream_effort: Some("high".into()),
        stop_reason: Some("end_turn".into()),
        tools_offered_count: Some(3),
        tool_result_count: Some(1),
        tool_use_count: Some(2),
        tool_use_names: Some(r#"["Read","Edit"]"#.into()),
    };
    let view: dto::RequestLog = through_json(&real);
    assert_eq!(
        view,
        dto::RequestLog {
            id: "r1".into(),
            timestamp: 1_700_000_000_000,
            virtual_model_name: "model-sonnet".into(),
            subscription_id: "s1".into(),
            provider_id: "zhipu".into(),
            endpoint_id: "default".into(),
            real_model_name: "glm-4.6".into(),
            response_model_name: Some("glm-4.6-vision".into()),
            is_streaming: true,
            status: dto::RequestStatus::Success,
            http_status: Some(200),
            total_latency_ms: Some(1234),
            input_tokens: Some(10),
            output_tokens: Some(20),
            cache_creation_tokens: Some(30),
            cache_read_tokens: Some(40),
            error_message: Some("boom".into()),
            upstream_response_body: Some("{}".into()),
            client_tool: Some("claude_code".into()),
            client_user_agent: Some("ua".into()),
            client_version: Some("1.2.3".into()),
            client_ip: Some("127.0.0.1".into()),
            entry_kind: Some("messages".into()),
            downstream_http_version: Some("HTTP/1.1".into()),
            client_effort: Some("high".into()),
            effective_effort: Some("high".into()),
            effort_source: Some("client".into()),
            upstream_effort: Some("high".into()),
            stop_reason: Some("end_turn".into()),
            tools_offered_count: Some(3),
            tool_result_count: Some(1),
            tool_use_count: Some(2),
            tool_use_names: Some(r#"["Read","Edit"]"#.into()),
        }
    );

    // 老数据 / 大部分字段为 NULL 的真实形状: 除必填字段外全部 None。
    let sparse = RequestLogDto {
        id: "r2".into(),
        timestamp: 1,
        virtual_model_name: "model-fable".into(),
        subscription_id: "s2".into(),
        provider_id: "openai".into(),
        endpoint_id: "default".into(),
        real_model_name: "gpt-5".into(),
        response_model_name: None,
        is_streaming: false,
        status: RequestStatus::Error.as_str().to_string(),
        http_status: None,
        total_latency_ms: None,
        input_tokens: None,
        output_tokens: None,
        cache_creation_tokens: None,
        cache_read_tokens: None,
        error_message: None,
        upstream_response_body: None,
        client_tool: None,
        client_user_agent: None,
        client_version: None,
        client_ip: None,
        entry_kind: None,
        downstream_http_version: None,
        client_effort: None,
        effective_effort: None,
        effort_source: None,
        upstream_effort: None,
        stop_reason: None,
        tools_offered_count: None,
        tool_result_count: None,
        tool_use_count: None,
        tool_use_names: None,
    };
    let view: dto::RequestLog = through_json(&sparse);
    assert_eq!(
        view,
        dto::RequestLog {
            id: "r2".into(),
            timestamp: 1,
            virtual_model_name: "model-fable".into(),
            subscription_id: "s2".into(),
            provider_id: "openai".into(),
            endpoint_id: "default".into(),
            real_model_name: "gpt-5".into(),
            response_model_name: None,
            is_streaming: false,
            status: dto::RequestStatus::Error,
            http_status: None,
            total_latency_ms: None,
            input_tokens: None,
            output_tokens: None,
            cache_creation_tokens: None,
            cache_read_tokens: None,
            error_message: None,
            upstream_response_body: None,
            client_tool: None,
            client_user_agent: None,
            client_version: None,
            client_ip: None,
            entry_kind: None,
            downstream_http_version: None,
            client_effort: None,
            effective_effort: None,
            effort_source: None,
            upstream_effort: None,
            stop_reason: None,
            tools_offered_count: None,
            tool_result_count: None,
            tool_use_count: None,
            tool_use_names: None,
        }
    );
}

/// `ListRequestsResult` → `dto::RequestPage`: `items` 与 `total` 都原样接住。
#[test]
fn list_requests_result_matches() {
    let real = ListRequestsResult {
        items: vec![RequestLogDto {
            id: "r1".into(),
            timestamp: 1,
            virtual_model_name: "model-sonnet".into(),
            subscription_id: "s1".into(),
            provider_id: "zhipu".into(),
            endpoint_id: "default".into(),
            real_model_name: "glm-4.6".into(),
            response_model_name: None,
            is_streaming: false,
            status: RequestStatus::Timeout.as_str().to_string(),
            http_status: None,
            total_latency_ms: None,
            input_tokens: None,
            output_tokens: None,
            cache_creation_tokens: None,
            cache_read_tokens: None,
            error_message: None,
            upstream_response_body: None,
            client_tool: None,
            client_user_agent: None,
            client_version: None,
            client_ip: None,
            entry_kind: None,
            downstream_http_version: None,
            client_effort: None,
            effective_effort: None,
            effort_source: None,
            upstream_effort: None,
            stop_reason: None,
            tools_offered_count: None,
            tool_result_count: None,
            tool_use_count: None,
            tool_use_names: None,
        }],
        total: 42,
    };
    let view: dto::RequestPage = through_json(&real);
    assert_eq!(view.total, 42);
    assert_eq!(view.items.len(), 1);
    assert_eq!(view.items[0].id, "r1");
    assert_eq!(view.items[0].status, dto::RequestStatus::Timeout);
}

/// 后端的每一个请求状态, TUI 都必须认得 (落到 `Unknown` 说明枚举漏了一个); 并且 TUI 发过滤条件
/// 用的 `as_wire()` 必须与后端 `RequestStatus::as_str()` 逐字相同——这条契约既锁字符串值,
/// 也锁住两边多加了状态时不会互相脱节。
#[test]
fn every_backend_request_status_is_known_to_the_tui() {
    for status in [RequestStatus::Success, RequestStatus::Error, RequestStatus::Timeout] {
        let wire = status.as_str();
        let view: dto::RequestStatus = serde_json::from_value(serde_json::Value::String(wire.to_string()))
            .unwrap_or_else(|e| panic!("TUI 读不了后端 RequestStatus::as_str() 吐出的 {wire:?}: {e}"));
        assert_ne!(view, dto::RequestStatus::Unknown, "{wire:?}");
        assert_eq!(view.as_wire(), wire, "TUI as_wire() 应该与后端 as_str() 逐字相同");
    }
}

/// TUI 全过滤的 `RequestQuery::to_args()` 顶层键恰好是 `page` / `pageSize` / `filters` 三个
/// (camelCase 只在顶层, `filters` 保持 snake_case); `filters` 必须能被后端 `RequestLogFilters`
/// 反序列化, 三个字段值原样保留。
#[test]
fn request_query_args_deserialize_into_the_backend_filters() {
    let query = dto::RequestQuery {
        page: 3,
        filters: dto::RequestFilters {
            subscription_id: Some("s1".into()),
            virtual_model_name: Some("model-sonnet".into()),
            status: Some(dto::RequestStatus::Error),
        },
    };
    let args = query.to_args();
    let obj = args.as_object().expect("to_args() 应该是个 JSON 对象");
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["filters", "page", "pageSize"]);

    let filters: RequestLogFilters = serde_json::from_value(obj["filters"].clone())
        .unwrap_or_else(|e| panic!("后端 RequestLogFilters 读不了 TUI 发的 filters: {e}"));
    assert_eq!(filters.subscription_id.as_deref(), Some("s1"));
    assert_eq!(filters.virtual_model_name.as_deref(), Some("model-sonnet"));
    assert_eq!(filters.status.as_deref(), Some("error"));
}

fn provider_from_yaml(yaml: &str) -> Provider {
    serde_yaml::from_str(yaml).unwrap()
}

/// 两个 endpoint 的智谱: 名字 / 描述 / 端点名用三语写法, 另一处用纯字符串写法, 两种都要走通。
fn zhipu_provider() -> Provider {
    provider_from_yaml(
        r#"
id: zhipu
display_name: { zh: "智谱", en: "Zhipu GLM", ja: "Zhipu GLM（日）" }
description: { zh: "智谱 AI", en: "Zhipu AI", ja: "Zhipu AI" }
compatibility: verified
endpoints:
  - id: default
    label: { zh: "默认", en: "Default", ja: "デフォルト" }
    base_url: "https://open.bigmodel.cn/api/anthropic"
    messages_path: "/v1/messages"
  - id: intl
    label: { zh: "国际版", en: "International", ja: "国際版" }
    base_url: "https://intl.bigmodel.cn/api/anthropic"
    messages_path: "/v1/messages"
default_endpoint: intl
auth: { type: api_key, header_name: Authorization, header_format: bearer }
model_discovery: { example_models: ["glm-4.6"] }
"#,
    )
}

/// `list_providers` 的一项: 两个 endpoint、`auth.type = "api_key"` (键名是 `type` 不是 `auth_type`,
/// 见 `provider::model::Auth` 的 `#[serde(rename)]`)、`model_discovery` 带 `example_models`。同时
/// 覆盖 `Provider::is_oauth()` / `default_endpoint()` 两个取值方法。
#[test]
fn provider_info_matches() {
    let real = ProviderInfo::from(&zhipu_provider());
    let view: dto::Provider = through_json(&real);
    assert_eq!(view.id, "zhipu");
    assert_eq!(view.display_name, "智谱");
    assert_eq!(view.description.as_deref(), Some("智谱 AI"), "description 字段被改名会静默变 None, 厂商选择器的 hint 会整列消失");
    assert_eq!(view.endpoints.len(), 2);
    assert_eq!(view.endpoints[0].base_url, "https://open.bigmodel.cn/api/anthropic");
    assert_eq!(view.default_endpoint.as_deref(), Some("intl"));
    assert_eq!(view.auth.auth_type, "api_key", "auth 的键名应该是 type, 不是 auth_type");
    assert!(view.model_discovery.enabled);
    assert_eq!(view.model_discovery.example_models, vec!["glm-4.6".to_string()]);
    assert!(!view.is_oauth());
    assert_eq!(view.default_endpoint().map(|e| e.id.as_str()), Some("intl"), "应该取 default_endpoint 指的那一个");

    // 顶层是中文, 英日文从 translations 叠加; 纯字符串写法的字段三语相同。
    let en = view.clone().localized(Lang::En);
    assert_eq!(en.display_name, "Zhipu GLM");
    assert_eq!(en.description.as_deref(), Some("Zhipu AI"));
    assert_eq!(en.endpoints[0].label, "Default");
    assert_eq!(en.endpoints[1].label, "International");
    let ja = view.clone().localized(Lang::Ja);
    assert_eq!(ja.display_name, "Zhipu GLM（日）");
    assert_eq!(ja.endpoints[1].label, "国際版");

    // 指不到 (default_endpoint 是个不存在的 id) 就落回第一个。
    let mut dangling = view.clone();
    dangling.default_endpoint = Some("no-such-id".into());
    assert_eq!(dangling.default_endpoint().map(|e| e.id.as_str()), Some("default"));

    // chatgpt_oauth / kiro_oauth 这两类应该被 TUI 判定为 OAuth (厂商选择器里置灰)。
    let oauth_real = ProviderInfo::from(&provider_from_yaml(
        r#"
id: chatgpt
display_name: ChatGPT
compatibility: untested
category: second_party
endpoints: []
auth: { type: chatgpt_oauth, header_name: Authorization, header_format: bearer }
"#,
    ));
    let oauth_view: dto::Provider = through_json(&oauth_real);
    assert!(oauth_view.is_oauth());
}

/// `CreateInput::to_args()["input"]` 能被后端 `CreateSubscriptionInput` 反序列化——内置模板
/// (`from_template`) 与自定义 (`custom`) 两个 source 变体各来一次; `custom` 再单独验证
/// `models_url = None` 时整个键都不出现 (不是发 `null`), 有值时正常带上。
#[test]
fn create_subscription_input_matches() {
    let via_template = dto::CreateInput {
        display_name: "智谱主号".into(),
        api_key: Secret::new("sk-test"),
        model_slots: dto::ModelSlots { fable: "f".into(), opus: "o".into(), sonnet: "s".into(), haiku: "h".into(), fallback: String::new(), jev: String::new() },
        source: dto::CreateSource::Builtin { provider_id: "zhipu".into(), endpoint_id: "default".into() },
    };
    let args = via_template.to_args();
    let input: CreateSubscriptionInput = serde_json::from_value(args["input"].clone())
        .unwrap_or_else(|e| panic!("后端读不了 TUI 发的 from_template CreateInput: {e}\n{args:#}"));
    assert_eq!(input.display_name, "智谱主号");
    assert_eq!(input.api_key, "sk-test");
    assert_eq!(input.model_slots.fable, "f");
    match input.source {
        CreateSource::FromTemplate { provider_id, endpoint_id, url_params } => {
            assert_eq!(provider_id, "zhipu");
            assert_eq!(endpoint_id, "default");
            // TUI does not send url_params yet: the backend must read it as empty.
            assert!(url_params.is_empty());
        }
        other => panic!("应该是 FromTemplate: {other:?}"),
    }

    // custom 分支: 5 个协议全过一遍, 逐个断言后端反序列化出来的 `protocol` 就是对应的后端变体——
    // 只测缺省值 (Anthropic) 测不出「字段被改名, 后端 `#[serde(default)]`
    // 悄悄把它吞成 Anthropic」这类回归: `CreateSource::Custom.protocol` 恰好默认就是 Anthropic,
    // `CreateSource` 也没有 `deny_unknown_fields`, 单测一个缺省值毫无区分力。
    for tui_protocol in dto::CustomProtocol::ALL {
        let expected_real = expected_backend_protocol(tui_protocol);
        let input = dto::CreateInput {
            display_name: "中转站".into(),
            api_key: Secret::new("sk-relay"),
            model_slots: dto::ModelSlots::pending(),
            source: dto::CreateSource::Custom(Box::new(dto::CustomSource {
                provider_display_name: "中转站".into(),
                base_url: "https://relay.example.com".into(),
                messages_path: "/v1/messages".into(),
                auth_header_name: "Authorization".into(),
                auth_header_format: dto::AuthHeaderFormat::Bearer,
                protocol: tui_protocol,
                models_url: None,
            })),
        };
        let args = input.to_args();
        let parsed: CreateSubscriptionInput = serde_json::from_value(args["input"].clone())
            .unwrap_or_else(|e| panic!("后端读不了 TUI 发的 custom CreateInput ({tui_protocol:?}): {e}\n{args:#}"));
        match parsed.source {
            CreateSource::Custom { protocol, .. } => {
                assert_eq!(protocol, expected_real, "{tui_protocol:?} 应该解析回 {expected_real:?}, 不是被 serde(default) 吞掉");
            }
            other => panic!("应该是 Custom: {other:?}"),
        }
    }

    // `models_url` 的 None/Some 两种形状与 `protocol` 本身无关, 用一个协议测就够。
    let mut via_custom = dto::CreateInput {
        display_name: "中转站".into(),
        api_key: Secret::new("sk-relay"),
        model_slots: dto::ModelSlots::pending(),
        source: dto::CreateSource::Custom(Box::new(dto::CustomSource {
            provider_display_name: "中转站".into(),
            base_url: "https://relay.example.com".into(),
            messages_path: "/v1/messages".into(),
            auth_header_name: "Authorization".into(),
            auth_header_format: dto::AuthHeaderFormat::Bearer,
            protocol: dto::CustomProtocol::Anthropic,
            models_url: None,
        })),
    };
    let args = via_custom.to_args();
    assert!(args["input"]["source"].get("models_url").is_none(), "models_url 为 None 时整个键不该出现: {args:#}");
    let input: CreateSubscriptionInput = serde_json::from_value(args["input"].clone())
        .unwrap_or_else(|e| panic!("后端读不了 TUI 发的 custom CreateInput (无 models_url): {e}\n{args:#}"));
    match input.source {
        CreateSource::Custom { models_url, base_url, .. } => {
            assert_eq!(models_url, None);
            assert_eq!(base_url, "https://relay.example.com");
        }
        other => panic!("应该是 Custom: {other:?}"),
    }

    let dto::CreateSource::Custom(custom) = &mut via_custom.source else { unreachable!() };
    custom.models_url = Some("https://relay.example.com/v1/models".into());
    let args = via_custom.to_args();
    assert_eq!(args["input"]["source"]["models_url"], "https://relay.example.com/v1/models");
    let input: CreateSubscriptionInput = serde_json::from_value(args["input"].clone())
        .unwrap_or_else(|e| panic!("后端读不了 TUI 发的 custom CreateInput (带 models_url): {e}\n{args:#}"));
    match input.source {
        CreateSource::Custom { models_url, .. } => assert_eq!(models_url.as_deref(), Some("https://relay.example.com/v1/models")),
        other => panic!("应该是 Custom: {other:?}"),
    }
}

/// `ProbeInput::to_args()["input"]` 能被后端 `ProbeCustomModelsInput` 反序列化; `ProbeModelsResult`
/// 的两个变体能从后端 `ProbeCustomModelsResult` 序列化结果读回来。
#[test]
fn probe_custom_models_input_and_result_match() {
    let input = dto::ProbeInput {
        base_url: "https://relay.example.com".into(),
        auth_header_name: "Authorization".into(),
        auth_header_format: dto::AuthHeaderFormat::Bearer,
        api_key: Secret::new("sk-test"),
        protocol: dto::CustomProtocol::OpenaiChatCompletions,
    };
    let args = input.to_args();
    let real: ProbeCustomModelsInput = serde_json::from_value(args["input"].clone())
        .unwrap_or_else(|e| panic!("后端读不了 TUI 发的 ProbeInput: {e}\n{args:#}"));
    assert_eq!(real.base_url, "https://relay.example.com");
    assert_eq!(real.auth_header_name, "Authorization");
    assert_eq!(real.auth_header_format, AuthHeaderFormat::Bearer);
    assert_eq!(real.api_key, "sk-test");
    assert_eq!(real.protocol, CustomProtocol::OpenaiChatCompletions);

    let auto = ProbeCustomModelsResult::Auto {
        models: vec![ModelInfo { id: "glm-4.6".into(), display_name: None }],
        models_url: "https://relay.example.com/v1/models".into(),
    };
    let view: dto::ProbeModelsResult = through_json(&auto);
    assert_eq!(
        view,
        dto::ProbeModelsResult::Auto {
            models: vec![dto::ModelInfo { id: "glm-4.6".into(), display_name: None }],
            models_url: "https://relay.example.com/v1/models".into(),
        }
    );

    let manual = ProbeCustomModelsResult::ManualFallback { reason: "拒绝连接".into() };
    let view: dto::ProbeModelsResult = through_json(&manual);
    assert_eq!(view, dto::ProbeModelsResult::ManualFallback { reason: "拒绝连接".into() });
}

/// `CustomProtocol::ALL` 的 `as_wire()` 逐个能被后端 `CustomProtocol` 反序列化, 解析回同一个变体。
/// 遍历 `dto::CustomProtocol::ALL` 本身 (而不是手写 5 对): TUI 将来往
/// `ALL` 里加第 6 个协议时, `expected_backend_protocol` 的穷尽 `match` 会 `E0004`, 逼着这条测试
/// 也跟着覆盖新协议——手写列表不会自动长出新的一对。
#[test]
fn custom_protocol_wire_names_round_trip() {
    for tui_protocol in dto::CustomProtocol::ALL {
        let expected_real = expected_backend_protocol(tui_protocol);
        let wire = tui_protocol.as_wire();
        let real: CustomProtocol = serde_json::from_value(serde_json::Value::String(wire.to_string()))
            .unwrap_or_else(|e| panic!("后端 CustomProtocol 读不了 TUI 发的线上名字 {wire:?}: {e}"));
        assert_eq!(real, expected_real, "{wire:?} 应该解析回 {expected_real:?}");
    }
}

/// `AuthHeaderFormat::as_wire()` 的两个取值能被后端 `AuthHeaderFormat` 反序列化。
#[test]
fn auth_header_format_wire_names_round_trip() {
    for (tui_format, expected_real) in [(dto::AuthHeaderFormat::Raw, AuthHeaderFormat::Raw), (dto::AuthHeaderFormat::Bearer, AuthHeaderFormat::Bearer)]
    {
        let wire = tui_format.as_wire();
        let real: AuthHeaderFormat = serde_json::from_value(serde_json::Value::String(wire.to_string()))
            .unwrap_or_else(|e| panic!("后端 AuthHeaderFormat 读不了 TUI 发的线上名字 {wire:?}: {e}"));
        assert_eq!(real, expected_real, "{wire:?}");
    }
}

/// TUI 拿来判断「该在厂商选择器里置灰」的 `OAUTH_AUTH_TYPES` 集合, 必须与后端「OAuth 类」
/// `AuthType` 变体集合**完全相等**——不只是「TUI 的两个字符串后端认得」这种单向锁。
/// `is_oauth_auth_type` 是对后端 `AuthType` 的穷尽 `match`, 后端加新变体 (CLAUDE.md 写了
/// Gemini OAuth / GitHub Copilot 之类的扩展点) 时这里会编译失败, 逼着当场决定它算不算 OAuth,
/// 而不是让 TUI 把它当 api_key 厂商展示、用户粘个 key 建出一条永远鉴权失败的订阅却没有任何测试炸。
#[test]
fn the_auth_types_the_tui_greys_out_exist_in_the_backend() {
    use AuthType::*;
    let backend_oauth: std::collections::BTreeSet<&str> = [
        ApiKey,
        ChatgptOauth,
        KiroOauth,
        GeminiApiKey,
        OpenaiResponsesApiKey,
        OpenaiChatCompletionsApiKey,
        GeminiInteractionsApiKey,
    ]
    .into_iter()
    .filter(|auth| is_oauth_auth_type(*auth))
    .map(|auth| auth.as_str())
    .collect();
    let tui_oauth: std::collections::BTreeSet<&str> = dto::OAUTH_AUTH_TYPES.into_iter().collect();
    assert_eq!(tui_oauth, backend_oauth, "TUI 的 OAUTH_AUTH_TYPES 应该恰好等于后端标注为 OAuth 的 AuthType 集合");
}

/// `PENDING_MODEL` 与桌面端 `src/routes/SubscriptionNew.tsx` 的字面量约定 (`uniformSlots("(pending)")`)
/// 逐字相同——不是自证: `include_str!` 真的把桌面端源码编译进来, 拿 **由 `PENDING_MODEL` 现算出来**
/// 的 `uniformSlots("<PENDING_MODEL>")` 去源码里找。无论哪一边把这个占位值改掉 (桌面端改写法,
/// 或 TUI 改 `PENDING_MODEL`), 这条测试都会跟着炸——注意桌面端同一个文件里还有一处
/// `uniformSlots("")` (`Step`1 初始化用的空值), 所以不能只找「第一处 `uniformSlots("` 出现的位置」,
/// 必须把 `PENDING_MODEL` 拼进 needle 里精确匹配。
#[test]
fn pending_placeholder_matches_the_desktop_wizard() {
    let desktop_source = include_str!("../../src/routes/SubscriptionNew.tsx");
    let needle = format!("uniformSlots(\"{}\")", dto::PENDING_MODEL);
    assert!(
        desktop_source.contains(&needle),
        "桌面端 SubscriptionNew.tsx 里没找到 {needle:?} —— PENDING_MODEL 与桌面端的占位值约定不一致了, 或桌面端换了写法"
    );

    let slots = dto::ModelSlots::pending();
    assert_eq!(slots.fable, dto::PENDING_MODEL);
    assert_eq!(slots.opus, dto::PENDING_MODEL);
    assert_eq!(slots.sonnet, dto::PENDING_MODEL);
    assert_eq!(slots.haiku, dto::PENDING_MODEL);
    assert_eq!(slots.fallback, "", "兜底槽应该留空串, 不是占位值");
}

/// systemone 订阅: endpoint_protocol 与 jev 槽都能被 TUI 读到。
#[test]
fn systemone_subscription_matches() {
    let mut row = SubscriptionRow::test_fixture("ollama", "localhost_systemone");
    row.endpoint_protocol = crate::provider::model::EndpointProtocol::Systemone;
    row.model_slots.jev = "clef-flash".into();
    let view: dto::Subscription = through_json(&SubscriptionDto::from_runtime(&SubscriptionRuntime::from_row(row), vec![], &HashMap::new()));
    assert!(view.is_systemone());
    assert_eq!(view.model_slots.jev, "clef-flash");
    assert!(dto::vm_accepts(dto::JEV_VM, &view));
    assert!(!dto::vm_accepts("model-opus", &view));
}

/// 端点级 protocol / example_models 能被 TUI 读到; 老 DTO 缺字段时按 messages。
#[test]
fn provider_endpoint_protocol_matches() {
    let p = provider_from_yaml(
        r#"
id: demo
display_name: Demo
compatibility: untested
endpoints:
  - id: s1
    label: S1
    base_url: "http://localhost:11434"
    messages_path: "/v1/systemone"
    protocol: systemone
    example_models: ["clef-flash"]
auth: { type: api_key, header_name: Authorization, header_format: bearer }
"#,
    );
    let view: dto::Provider = through_json(&ProviderInfo::from(&p));
    assert!(view.endpoints[0].is_systemone());
    assert_eq!(view.endpoints[0].example_models, vec!["clef-flash".to_string()]);
    let legacy: dto::ProviderEndpoint = serde_json::from_value(serde_json::json!({"id": "a", "label": "A", "base_url": "x"})).unwrap();
    assert!(!legacy.is_systemone());
}

#[test]
fn systemone_entry_kind_reaches_the_tui() {
    let real = crate::proxy::client_fingerprint::RequestEntryKind::SystemOne.as_str();
    assert_eq!(real, "systemone", "TUI 日志详情按 /v1/{{entry_kind}} 渲染");
}
