//! Subscription balance / quota discovery.
//!
//! Same shape as [`super::model_discovery`]: yaml declares endpoint + parser,
//! response parsing is hard-coded per provider. Heterogeneous response fields
//! (DeepSeek multi-currency array vs Minimax token quota vs …) defeat any
//! declarative JSONPath scheme, so dispatch lives in Rust.
//!
//! Per-parser invariants enforced here:
//! - Heterogeneous responses are flattened into UI-ready [`BalanceSnapshot`];
//!   the frontend renders without provider switches.
//! - Severity thresholds belong to the parser (provider-specific knowledge),
//!   e.g. DeepSeek CNY < ¥10 is `Low`.
//! - Amounts stay as strings end-to-end to avoid `f64` precision drift
//!   (DeepSeek returns `"39.28"`).

use chrono::Utc;
use serde::Deserialize;
use sqlx::SqlitePool;
use tracing::{info, warn};

use crate::error::AppError;
use crate::provider::model::BalanceParser;
use crate::subscription::{
    model::{BalanceEntry, BalanceSeverity, BalanceSnapshot, SubscriptionRow},
    store,
};

#[derive(Debug, thiserror::Error)]
pub enum BalanceError {
    #[error("该订阅所属 provider 未声明余额查询接口")]
    NotSupported,
    #[error("余额查询已禁用 (yaml enabled: false)")]
    Disabled,
    #[error("network: {0}")]
    Network(#[from] reqwest::Error),
    #[error("http {0}")]
    Http(u16),
    #[error("解析失败: {0}")]
    InvalidResponse(String),
    #[error("app error: {0}")]
    App(#[from] AppError),
}

impl From<serde_json::Error> for BalanceError {
    fn from(e: serde_json::Error) -> Self {
        Self::InvalidResponse(e.to_string())
    }
}

pub async fn fetch(
    client: &reqwest::Client,
    row: &SubscriptionRow,
) -> Result<BalanceSnapshot, BalanceError> {
    let bd = row
        .balance_discovery
        .as_ref()
        .ok_or(BalanceError::NotSupported)?;
    if !bd.enabled {
        return Err(BalanceError::Disabled);
    }

    let req = row.apply_auth_and_required_headers(client.request(bd.method.as_reqwest(), &bd.url));
    let resp = req.send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(BalanceError::Http(status.as_u16()));
    }
    let text = resp.text().await?;

    match bd.parser {
        BalanceParser::Deepseek => parse_deepseek(&text),
        BalanceParser::Openrouter => parse_openrouter(&text),
    }
}

pub async fn fetch_and_cache(
    pool: &SqlitePool,
    client: &reqwest::Client,
    row: &SubscriptionRow,
) -> Result<BalanceSnapshot, BalanceError> {
    let snapshot = fetch(client, row).await?;
    if let Err(e) = store::save_balance_cache(pool, &row.id, &snapshot).await {
        warn!(?e, subscription_id = %row.id, "balance cache 持久化失败");
    } else {
        info!(subscription_id = %row.id, "balance cached");
    }
    Ok(snapshot)
}

// ---------- DeepSeek parser ----------

#[derive(Debug, Deserialize)]
struct DeepSeekBalanceResp {
    is_available: Option<bool>,
    #[serde(default)]
    balance_infos: Vec<DeepSeekBalanceInfo>,
}

#[derive(Debug, Deserialize)]
struct DeepSeekBalanceInfo {
    currency: String,
    total_balance: String,
    #[serde(default)]
    granted_balance: String,
    #[serde(default)]
    topped_up_balance: String,
}

fn parse_deepseek(text: &str) -> Result<BalanceSnapshot, BalanceError> {
    let resp: DeepSeekBalanceResp = serde_json::from_str(text)?;
    if resp.balance_infos.is_empty() {
        return Err(BalanceError::InvalidResponse(
            "balance_infos 数组为空".into(),
        ));
    }

    let account_unavailable = resp.is_available == Some(false);
    let entries = resp
        .balance_infos
        .into_iter()
        .map(|info| deepseek_entry(info, account_unavailable))
        .collect();

    Ok(BalanceSnapshot {
        is_available: resp.is_available,
        entries,
        fetched_at: Utc::now(),
    })
}

fn deepseek_entry(info: DeepSeekBalanceInfo, account_unavailable: bool) -> BalanceEntry {
    // Severity falls back to 0 on parse failure so a malformed amount displays
    // as Critical rather than blowing up the whole snapshot. value_text keeps
    // the raw string for display fidelity.
    let total = info.total_balance.parse::<f64>().unwrap_or(0.0);
    let (low, critical) = deepseek_thresholds(&info.currency);
    let severity = if account_unavailable {
        BalanceSeverity::Critical
    } else {
        classify_by_threshold(total, low, critical)
    };
    let symbol = currency_symbol(&info.currency);
    let hint = format!(
        "充值 {symbol}{topped}, 赠送 {symbol}{granted}",
        topped = info.topped_up_balance,
        granted = info.granted_balance,
    );
    BalanceEntry {
        label: format!("余额 ({})", info.currency),
        value_text: info.total_balance,
        unit: info.currency,
        hint: Some(hint),
        severity,
    }
}

/// Returns `(low_threshold, critical_threshold)`. USD scales 10x smaller than CNY
/// (1 CNY ≈ 0.14 USD), other currencies fall back to CNY-style thresholds.
fn deepseek_thresholds(currency: &str) -> (f64, f64) {
    match currency {
        "USD" => (2.0, 0.2),
        _ => (10.0, 1.0),
    }
}

fn classify_by_threshold(value: f64, low: f64, critical: f64) -> BalanceSeverity {
    if value < critical {
        BalanceSeverity::Critical
    } else if value < low {
        BalanceSeverity::Low
    } else {
        BalanceSeverity::Normal
    }
}

fn currency_symbol(currency: &str) -> &'static str {
    match currency {
        "CNY" | "JPY" => "¥",
        "USD" => "$",
        "EUR" => "€",
        _ => "",
    }
}

// ---------- OpenRouter parser ----------
//
// GET https://openrouter.ai/api/v1/credits
// Response (verified 2026-05): {"data":{"total_credits": <num>, "total_usage": <num>}}
// `total_credits` is the lifetime topped-up amount; `total_usage` is the lifetime spend.
// Account balance shown in the OpenRouter dashboard = total_credits - total_usage.

#[derive(Debug, Deserialize)]
struct OpenRouterCreditsResp {
    data: OpenRouterCreditsData,
}

#[derive(Debug, Deserialize)]
struct OpenRouterCreditsData {
    total_credits: f64,
    total_usage: f64,
}

fn parse_openrouter(text: &str) -> Result<BalanceSnapshot, BalanceError> {
    let resp: OpenRouterCreditsResp = serde_json::from_str(text)?;
    let remaining = resp.data.total_credits - resp.data.total_usage;
    let (low, critical) = (2.0_f64, 0.2_f64);
    let severity = classify_by_threshold(remaining, low, critical);
    let hint = Some(format!(
        "充值 ${topup}, 已用 ${usage}",
        topup = format_amount(resp.data.total_credits),
        usage = format_amount(resp.data.total_usage),
    ));
    let entries = vec![BalanceEntry {
        label: "余额 (USD)".to_string(),
        value_text: format_amount(remaining),
        unit: "USD".to_string(),
        hint,
        severity,
    }];
    Ok(BalanceSnapshot {
        is_available: None,
        entries,
        fetched_at: Utc::now(),
    })
}

/// 2 decimal places — keeps the UI consistent with DeepSeek's string-formatted amounts.
fn format_amount(v: f64) -> String {
    format!("{:.2}", v)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HAPPY_REAL_JSON: &str = r#"{
        "is_available": true,
        "balance_infos": [
            {
                "currency": "CNY",
                "total_balance": "39.28",
                "granted_balance": "0.00",
                "topped_up_balance": "39.28"
            }
        ]
    }"#;

    #[test]
    fn parse_deepseek_happy_path() {
        let snap = parse_deepseek(HAPPY_REAL_JSON).expect("parse");
        assert_eq!(snap.is_available, Some(true));
        assert_eq!(snap.entries.len(), 1);
        let e = &snap.entries[0];
        assert_eq!(e.label, "余额 (CNY)");
        assert_eq!(e.value_text, "39.28");
        assert_eq!(e.unit, "CNY");
        assert_eq!(e.severity, BalanceSeverity::Normal);
        assert_eq!(e.hint.as_deref(), Some("充值 ¥39.28, 赠送 ¥0.00"));
    }

    #[test]
    fn parse_deepseek_multi_currency() {
        let json = r#"{
            "is_available": true,
            "balance_infos": [
                {"currency": "CNY", "total_balance": "12.00", "granted_balance": "0", "topped_up_balance": "12.00"},
                {"currency": "USD", "total_balance": "5.00", "granted_balance": "0", "topped_up_balance": "5.00"}
            ]
        }"#;
        let snap = parse_deepseek(json).expect("parse");
        assert_eq!(snap.entries.len(), 2);
        assert_eq!(snap.entries[0].unit, "CNY");
        assert_eq!(snap.entries[1].unit, "USD");
    }

    #[test]
    fn parse_deepseek_account_unavailable_forces_critical() {
        let json = r#"{
            "is_available": false,
            "balance_infos": [
                {"currency": "CNY", "total_balance": "1000.00", "granted_balance": "0", "topped_up_balance": "1000.00"}
            ]
        }"#;
        let snap = parse_deepseek(json).expect("parse");
        assert_eq!(snap.entries[0].severity, BalanceSeverity::Critical);
        assert_eq!(snap.is_available, Some(false));
    }

    #[test]
    fn parse_deepseek_severity_thresholds_cny() {
        let make = |amount: &str| {
            let json = format!(
                r#"{{"is_available":true,"balance_infos":[{{"currency":"CNY","total_balance":"{amount}","granted_balance":"0","topped_up_balance":"0"}}]}}"#
            );
            parse_deepseek(&json).unwrap().entries[0].severity
        };
        assert_eq!(make("50.00"), BalanceSeverity::Normal);
        assert_eq!(make("10.00"), BalanceSeverity::Normal);
        assert_eq!(make("9.99"), BalanceSeverity::Low);
        assert_eq!(make("5.00"), BalanceSeverity::Low);
        assert_eq!(make("1.00"), BalanceSeverity::Low);
        assert_eq!(make("0.99"), BalanceSeverity::Critical);
        assert_eq!(make("0.00"), BalanceSeverity::Critical);
    }

    #[test]
    fn parse_deepseek_severity_thresholds_usd() {
        let make = |amount: &str| {
            let json = format!(
                r#"{{"is_available":true,"balance_infos":[{{"currency":"USD","total_balance":"{amount}","granted_balance":"0","topped_up_balance":"0"}}]}}"#
            );
            parse_deepseek(&json).unwrap().entries[0].severity
        };
        assert_eq!(make("10.00"), BalanceSeverity::Normal);
        assert_eq!(make("2.00"), BalanceSeverity::Normal);
        assert_eq!(make("1.99"), BalanceSeverity::Low);
        assert_eq!(make("0.50"), BalanceSeverity::Low);
        assert_eq!(make("0.19"), BalanceSeverity::Critical);
    }

    #[test]
    fn parse_deepseek_empty_balance_infos() {
        let json = r#"{"is_available":true,"balance_infos":[]}"#;
        let err = parse_deepseek(json).unwrap_err();
        assert!(matches!(err, BalanceError::InvalidResponse(_)));
    }

    #[test]
    fn parse_deepseek_invalid_json() {
        let err = parse_deepseek("not json at all").unwrap_err();
        assert!(matches!(err, BalanceError::InvalidResponse(_)));
    }

    #[test]
    fn parse_deepseek_missing_is_available_field() {
        let json = r#"{
            "balance_infos": [
                {"currency": "CNY", "total_balance": "20.00", "granted_balance": "0", "topped_up_balance": "20.00"}
            ]
        }"#;
        let snap = parse_deepseek(json).expect("parse");
        assert_eq!(snap.is_available, None);
        assert_eq!(snap.entries.len(), 1);
    }

    // ---------- OpenRouter ----------

    #[test]
    fn parse_openrouter_happy_path() {
        // Real shape verified against api.openrouter.ai/api/v1/credits (2026-05).
        let json = r#"{"data":{"total_credits":10,"total_usage":2.45121913}}"#;
        let snap = parse_openrouter(json).expect("parse");
        assert_eq!(snap.is_available, None);
        assert_eq!(snap.entries.len(), 1);
        let e = &snap.entries[0];
        assert_eq!(e.label, "余额 (USD)");
        assert_eq!(e.value_text, "7.55");
        assert_eq!(e.unit, "USD");
        assert_eq!(e.severity, BalanceSeverity::Normal);
        assert_eq!(e.hint.as_deref(), Some("充值 $10.00, 已用 $2.45"));
    }

    #[test]
    fn parse_openrouter_low_threshold() {
        let json = r#"{"data":{"total_credits":3.0,"total_usage":1.5}}"#; // remaining = 1.5
        let snap = parse_openrouter(json).unwrap();
        assert_eq!(snap.entries[0].severity, BalanceSeverity::Low);
    }

    #[test]
    fn parse_openrouter_critical_threshold() {
        let json = r#"{"data":{"total_credits":1.0,"total_usage":0.9}}"#; // remaining = 0.1
        let snap = parse_openrouter(json).unwrap();
        assert_eq!(snap.entries[0].severity, BalanceSeverity::Critical);
    }

    #[test]
    fn parse_openrouter_zero_remaining() {
        let json = r#"{"data":{"total_credits":5.0,"total_usage":5.0}}"#;
        let snap = parse_openrouter(json).unwrap();
        assert_eq!(snap.entries[0].value_text, "0.00");
        assert_eq!(snap.entries[0].severity, BalanceSeverity::Critical);
    }

    #[test]
    fn parse_openrouter_invalid_json() {
        let err = parse_openrouter("not json").unwrap_err();
        assert!(matches!(err, BalanceError::InvalidResponse(_)));
    }
}
