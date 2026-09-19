//! 主 crate ↔ cc-router-tui 的契约。TUI 不依赖主 crate, 它的视图结构体是手写的;
//! 这里把**真实** DTO 序列化后塞进那些结构体, 后端改字段名 / 改枚举值 / 改头名字时当场失败。
//! 新增 TUI 用到的 DTO 时, 在这里加一条。

use std::path::Path;

use cc_router_tui::client::{discovery, dto, http};
use serde::{de::DeserializeOwned, Serialize};

use crate::commands::proxy::ProxyStatus;
use crate::commands::statistics::{DailySeriesPointDto, OverallStatsDto, StatsRange};
use crate::commands::subscriptions::{
    RefreshBalanceResult, RefreshModelListResult, SubscriptionPatch, TestConnectionResult, ALLOWED_SLOT_EFFORTS,
};
use crate::commands::virtual_models::{UpdateVirtualModelInput, VirtualModelDto};
use crate::provider::model::AuthType;
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
    let real = SubscriptionDto::from_runtime(&rt, Vec::new());
    let view: dto::Subscription = through_json(&real);
    assert_eq!(view.id, id);
    assert_eq!(view.display_name, "智谱主号");
    assert_eq!(view.provider_display_name, "智谱");
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
        }],
        fetched_at: chrono::Utc::now(),
    });
    let real = SubscriptionDto::from_runtime(&rt, vec!["model-sonnet".into()]);
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
    let real_absent = TestConnectionResult { ok: false, message: "网络错误".into(), http_status: None, model_used: None, state_reset: false };
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
    let view: dto::Subscription = through_json(&SubscriptionDto::from_runtime(&rt, Vec::new()));
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
    let model_slots = dto::ModelSlots { fable: "f".into(), opus: "o".into(), sonnet: "s".into(), haiku: "h".into(), fallback: String::new() };
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
