//! 通知子系统 (设计 §7.3–§7.5): 通知插件 (`type: "notifier"`) 与内置系统桌面通知共用同一份事件表、
//! 四语默认文案、10 分钟去重和每小时限流。
//!
//! 纪律:
//! - 任何投递失败只记进内存发送记录 (`DeliveryRecord`, 最近 20 条/插件, 不记录请求体) 并 warn,
//!   **永不影响代理请求**。
//! - 入口 `NotifyHub::emit` 是非阻塞 `try_send`, 不会阻塞状态机或请求日志消费端。

pub mod strings;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cc_router_plugin_spec::notifier_plugin::HttpMethod;
use cc_router_plugin_spec::notifier_template::{build_request, RenderedBody};
use chrono::{Local, Timelike, Utc};
use serde::Serialize;
use serde_json::{json, Map, Value};
use sqlx::Row;
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;
use tokio::sync::mpsc;
use tracing::warn;

use crate::observability::request_log::local_day_key;
use crate::plugins::notifier::LoadedNotifier;
use crate::plugins::pricing::{cost_of, subscription_contexts, CostTotals, Tokens};
use crate::state::AppState;
use crate::tray::TrayLocale;

/// 同一事件 + 同一目标的去重窗口 (设计 §7.4)
const DEDUPE_WINDOW: Duration = Duration::from_secs(10 * 60);
/// 每个插件每小时最多发送条数 (设计 §7.4)
const MAX_PER_HOUR: usize = 30;
const HOUR: Duration = Duration::from_secs(60 * 60);
/// 每个插件在内存里保留最近 N 条发送记录
const MAX_RECORDS_PER_PLUGIN: usize = 20;
/// 单次 HTTP 推送超时 (设计 §7.4)
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// 所有可触发通知的事件 (与 `notifier_plugin::EVENTS` 一一对应, 设计 §7.3)。
#[derive(Debug, Clone)]
pub enum NotifyEvent {
    SubscriptionAuthFailed {
        subscription_id: String,
        subscription_name: String,
        provider_name: String,
        reason: String,
    },
    SubscriptionCooldown {
        subscription_id: String,
        subscription_name: String,
        provider_name: String,
        reason: String,
    },
    SubscriptionRecovered {
        subscription_id: String,
        subscription_name: String,
        provider_name: String,
    },
    SubscriptionQuotaReached {
        subscription_id: String,
        subscription_name: String,
        provider_name: String,
        period: String,
        used: u64,
        limit: u64,
    },
    SubscriptionQuotaWarning {
        subscription_id: String,
        subscription_name: String,
        provider_name: String,
        period: String,
        used: u64,
        limit: u64,
        percent: u64,
    },
    SubscriptionBalanceLow {
        subscription_id: String,
        subscription_name: String,
        provider_name: String,
        remaining: String,
        unit: String,
    },
    RequestsAllFailed {
        virtual_model: String,
        count: u64,
    },
    ProxyStopped {
        reason: String,
    },
    UpdateAvailable {
        version: String,
    },
    UsageDailySummary {
        requests: u64,
        tokens: u64,
        cost_usd: String,
        cost_cny: String,
        top_subscriptions: Vec<String>,
    },
    Test,
}

impl NotifyEvent {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SubscriptionAuthFailed { .. } => "subscription.auth_failed",
            Self::SubscriptionCooldown { .. } => "subscription.cooldown",
            Self::SubscriptionRecovered { .. } => "subscription.recovered",
            Self::SubscriptionQuotaReached { .. } => "subscription.quota_reached",
            Self::SubscriptionQuotaWarning { .. } => "subscription.quota_warning",
            Self::SubscriptionBalanceLow { .. } => "subscription.balance_low",
            Self::RequestsAllFailed { .. } => "requests.all_failed",
            Self::ProxyStopped { .. } => "proxy.stopped",
            Self::UpdateAvailable { .. } => "update.available",
            Self::UsageDailySummary { .. } => "usage.daily_summary",
            Self::Test => "test",
        }
    }

    /// 10 分钟去重键 (同类事件按订阅 / 虚拟模型 / 版本分桶)。
    pub fn dedupe_key(&self) -> String {
        match self {
            Self::SubscriptionAuthFailed { subscription_id, .. }
            | Self::SubscriptionCooldown { subscription_id, .. }
            | Self::SubscriptionRecovered { subscription_id, .. }
            | Self::SubscriptionBalanceLow { subscription_id, .. } => subscription_id.clone(),
            Self::SubscriptionQuotaReached { subscription_id, period, .. }
            | Self::SubscriptionQuotaWarning { subscription_id, period, .. } => format!("{subscription_id}:{period}"),
            Self::RequestsAllFailed { virtual_model, .. } => virtual_model.clone(),
            Self::ProxyStopped { .. } => "proxy".into(),
            Self::UpdateAvailable { version } => version.clone(),
            Self::UsageDailySummary { .. } => "daily".into(),
            Self::Test => "test".into(),
        }
    }

    /// 按当前界面语言预渲染默认 `title` / `text` (供桌面通知和通知插件的 `event.title` / `event.text` 使用)。
    pub fn render_default(&self, locale: TrayLocale) -> (String, String) {
        let s = strings::strings_for(locale);
        match self {
            Self::SubscriptionAuthFailed { subscription_name, provider_name, reason, .. } => {
                ((s.auth_failed_title)(subscription_name), (s.auth_failed_text)(provider_name, reason))
            }
            Self::SubscriptionCooldown { subscription_name, provider_name, reason, .. } => {
                ((s.cooldown_title)(subscription_name), (s.cooldown_text)(provider_name, reason))
            }
            Self::SubscriptionRecovered { subscription_name, provider_name, .. } => {
                ((s.recovered_title)(subscription_name), (s.recovered_text)(provider_name))
            }
            Self::SubscriptionQuotaReached { subscription_name, period, used, limit, .. } => {
                ((s.quota_reached_title)(subscription_name), (s.quota_reached_text)(period, *used, *limit))
            }
            Self::SubscriptionQuotaWarning { subscription_name, period, used, limit, percent, .. } => (
                (s.quota_warning_title)(subscription_name, *percent),
                (s.quota_warning_text)(period, *used, *limit, *percent),
            ),
            Self::SubscriptionBalanceLow { subscription_name, provider_name, remaining, unit, .. } => (
                (s.balance_low_title)(subscription_name),
                (s.balance_low_text)(provider_name, remaining, unit),
            ),
            Self::RequestsAllFailed { virtual_model, count } => {
                ((s.all_failed_title)(virtual_model), (s.all_failed_text)(virtual_model, *count))
            }
            Self::ProxyStopped { reason } => (s.proxy_stopped_title.to_string(), (s.proxy_stopped_text)(reason)),
            Self::UpdateAvailable { version } => {
                ((s.update_available_title)(version), (s.update_available_text)(version))
            }
            Self::UsageDailySummary { requests, tokens, cost_usd, cost_cny, .. } => {
                let mut parts = Vec::new();
                if !cost_usd.is_empty() {
                    parts.push(format!("${cost_usd}"));
                }
                if !cost_cny.is_empty() {
                    parts.push(format!("¥{cost_cny}"));
                }
                let cost = parts.join(" + ");
                (s.daily_summary_title.to_string(), (s.daily_summary_text)(*requests, *tokens, &cost))
            }
            Self::Test => (s.test_title.to_string(), s.test_text.to_string()),
        }
    }

    /// 构造给通知插件模板用的事件上下文 (只含设计 §7.3 公开字段, 永不含 Key / 提示词 / 头 / IP)。
    pub fn template_context(&self, locale: TrayLocale) -> Map<String, Value> {
        let (title, text) = self.render_default(locale);
        let mut ctx = Map::new();
        ctx.insert("event".into(), json!({ "kind": self.kind(), "title": title, "text": text }));
        ctx.insert("time".into(), Value::String(Local::now().format("%Y-%m-%d %H:%M:%S").to_string()));
        match self {
            Self::SubscriptionAuthFailed { subscription_name, provider_name, reason, .. }
            | Self::SubscriptionCooldown { subscription_name, provider_name, reason, .. } => {
                ctx.insert("subscription".into(), json!({ "name": subscription_name }));
                ctx.insert("provider".into(), json!({ "name": provider_name }));
                ctx.insert("reason".into(), Value::String(reason.clone()));
            }
            Self::SubscriptionRecovered { subscription_name, provider_name, .. } => {
                ctx.insert("subscription".into(), json!({ "name": subscription_name }));
                ctx.insert("provider".into(), json!({ "name": provider_name }));
            }
            Self::SubscriptionQuotaReached { subscription_name, provider_name, period, used, limit, .. } => {
                ctx.insert("subscription".into(), json!({ "name": subscription_name }));
                ctx.insert("provider".into(), json!({ "name": provider_name }));
                ctx.insert("period".into(), Value::String(period.clone()));
                ctx.insert("used".into(), json!(used));
                ctx.insert("limit".into(), json!(limit));
            }
            Self::SubscriptionQuotaWarning {
                subscription_name,
                provider_name,
                period,
                used,
                limit,
                percent,
                ..
            } => {
                ctx.insert("subscription".into(), json!({ "name": subscription_name }));
                ctx.insert("provider".into(), json!({ "name": provider_name }));
                ctx.insert("period".into(), Value::String(period.clone()));
                ctx.insert("used".into(), json!(used));
                ctx.insert("limit".into(), json!(limit));
                ctx.insert("percent".into(), json!(percent));
            }
            Self::SubscriptionBalanceLow { subscription_name, provider_name, remaining, unit, .. } => {
                ctx.insert("subscription".into(), json!({ "name": subscription_name }));
                ctx.insert("provider".into(), json!({ "name": provider_name }));
                ctx.insert("remaining".into(), Value::String(remaining.clone()));
                ctx.insert("unit".into(), Value::String(unit.clone()));
            }
            Self::RequestsAllFailed { virtual_model, count } => {
                ctx.insert("virtual_model".into(), Value::String(virtual_model.clone()));
                ctx.insert("count".into(), json!(count));
            }
            Self::ProxyStopped { reason } => {
                ctx.insert("reason".into(), Value::String(reason.clone()));
            }
            Self::UpdateAvailable { version } => {
                ctx.insert("version".into(), Value::String(version.clone()));
            }
            Self::UsageDailySummary { requests, tokens, cost_usd, cost_cny, top_subscriptions } => {
                ctx.insert("requests".into(), json!(requests));
                ctx.insert("tokens".into(), json!(tokens));
                ctx.insert("costUsd".into(), Value::String(cost_usd.clone()));
                ctx.insert("costCny".into(), Value::String(cost_cny.clone()));
                ctx.insert("topSubscriptions".into(), json!(top_subscriptions));
            }
            Self::Test => {}
        }
        ctx
    }
}

/// 一次推送尝试的记录 (保留最近 20 条/插件, 刻意不记录请求体)。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeliveryRecord {
    pub timestamp_ms: i64,
    pub event: String,
    pub ok: bool,
    pub http_status: Option<u16>,
    pub error: Option<String>,
}

/// 纯内存去重与限流器 (可脱离网络与系统时钟独立单测)。
#[derive(Debug, Default)]
pub struct Limiter {
    /// `(target, event_kind, dedupe_key) -> last_sent`
    last_sent: HashMap<(String, String, String), Instant>,
    /// `plugin_id -> 最近 1 小时内的发送时刻`
    hourly: HashMap<String, VecDeque<Instant>>,
}

impl Limiter {
    /// 判断是否允许对 `target` 投递 `(event_kind, dedupe_key)`。`enforce_hourly` 仅对插件开启。
    pub fn allow(&mut self, target: &str, event_kind: &str, dedupe_key: &str, enforce_hourly: bool, now: Instant) -> bool {
        let key = (target.to_string(), event_kind.to_string(), dedupe_key.to_string());
        if let Some(&prev) = self.last_sent.get(&key) {
            if now.saturating_duration_since(prev) < DEDUPE_WINDOW {
                return false;
            }
        }
        if enforce_hourly {
            let q = self.hourly.entry(target.to_string()).or_default();
            while q.front().is_some_and(|&t| now.saturating_duration_since(t) >= HOUR) {
                q.pop_front();
            }
            if q.len() >= MAX_PER_HOUR {
                return false;
            }
            q.push_back(now);
        }
        self.last_sent.insert(key, now);
        true
    }
}

/// 全局通知调度中心, 挂在 `AppState.notify`。
pub struct NotifyHub {
    tx: mpsc::Sender<NotifyEvent>,
    records: Mutex<HashMap<String, VecDeque<DeliveryRecord>>>,
    limiter: Mutex<Limiter>,
    /// 上次通知过的更新版本号 (避免定时复检重复发 `update.available`)
    last_notified_update: Mutex<Option<String>>,
}

impl NotifyHub {
    pub fn new() -> (Arc<Self>, mpsc::Receiver<NotifyEvent>) {
        let (tx, rx) = mpsc::channel(256);
        let hub = Arc::new(Self {
            tx,
            records: Mutex::new(HashMap::new()),
            limiter: Mutex::new(Limiter::default()),
            last_notified_update: Mutex::new(None),
        });
        (hub, rx)
    }

    /// 非阻塞入队; 队列满时静默丢弃并 warn, 不阻塞调用方。
    pub fn emit(&self, event: NotifyEvent) {
        if let Err(e) = self.tx.try_send(event) {
            warn!(?e, "notify event dropped");
        }
    }

    /// 仅在版本号变化时发一次 `update.available`。
    pub fn emit_update_if_new(&self, version: &str) {
        let mut guard = self.last_notified_update.lock().unwrap_or_else(|e| e.into_inner());
        if guard.as_deref() == Some(version) {
            return;
        }
        *guard = Some(version.to_string());
        drop(guard);
        self.emit(NotifyEvent::UpdateAvailable { version: version.to_string() });
    }

    pub fn record_delivery(&self, plugin_id: &str, rec: DeliveryRecord) {
        let mut map = self.records.lock().unwrap_or_else(|e| e.into_inner());
        let q = map.entry(plugin_id.to_string()).or_default();
        if q.len() >= MAX_RECORDS_PER_PLUGIN {
            q.pop_back();
        }
        q.push_front(rec);
    }

    pub fn deliveries(&self, plugin_id: &str) -> Vec<DeliveryRecord> {
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(plugin_id)
            .map(|q| q.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn allow(&self, target: &str, event: &NotifyEvent, enforce_hourly: bool) -> bool {
        if matches!(event, NotifyEvent::Test) {
            return true;
        }
        self.limiter
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .allow(target, event.kind(), &event.dedupe_key(), enforce_hourly, Instant::now())
    }
}

/// 发一次 HTTP 推送: 超时 10s, 网络错误或 5xx 重试 1 次 (设计 §7.4)。
pub async fn send_to_plugin(
    client: &reqwest::Client,
    loaded: &LoadedNotifier,
    event_name: &str,
    event_ctx: &Map<String, Value>,
) -> DeliveryRecord {
    let now_ms = Utc::now().timestamp_millis();
    let rendered = match build_request(&loaded.plugin, &loaded.params, event_name, event_ctx) {
        Ok(r) => r,
        Err(e) => {
            return DeliveryRecord {
                timestamp_ms: now_ms,
                event: event_name.to_string(),
                ok: false,
                http_status: None,
                error: Some(e),
            };
        }
    };

    let mut last_status: Option<u16> = None;
    let mut last_err: Option<String> = None;
    for attempt in 0..2 {
        let mut req = match rendered.method {
            HttpMethod::Get => client.get(&rendered.url),
            HttpMethod::Post => client.post(&rendered.url),
            HttpMethod::Put => client.put(&rendered.url),
        }
        .timeout(HTTP_TIMEOUT);
        for (k, v) in &rendered.headers {
            req = req.header(k.as_str(), v.as_str());
        }
        req = match &rendered.body {
            None => req,
            Some(RenderedBody::Text(t)) => req.header("content-type", "text/plain; charset=utf-8").body(t.clone()),
            Some(RenderedBody::Json(j)) => req.json(j),
            Some(RenderedBody::Form(pairs)) => req.form(pairs),
        };
        match req.send().await {
            Ok(resp) => {
                let code = resp.status().as_u16();
                if resp.status().is_success() {
                    return DeliveryRecord {
                        timestamp_ms: Utc::now().timestamp_millis(),
                        event: event_name.to_string(),
                        ok: true,
                        http_status: Some(code),
                        error: None,
                    };
                }
                last_status = Some(code);
                last_err = Some(format!("HTTP {code}"));
                // 4xx 不重试, 5xx 重试 1 次
                if code < 500 || attempt == 1 {
                    break;
                }
            }
            Err(e) => {
                last_status = None;
                last_err = Some(e.to_string());
                if attempt == 1 {
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }

    DeliveryRecord {
        timestamp_ms: Utc::now().timestamp_millis(),
        event: event_name.to_string(),
        ok: false,
        http_status: last_status,
        error: last_err,
    }
}

/// 发送一条内置系统桌面通知 (使用 `tauri-plugin-notification`)。
pub fn send_desktop(app: &AppHandle, title: &str, body: &str) -> Result<(), String> {
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| e.to_string())
}

fn parse_hhmm(s: &str) -> Option<(u32, u32)> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some((h, m))
}

/// 后台消费端: 处理通知事件 + 每分钟检查每日汇总定时触发。
pub async fn run_worker(state: AppState, mut rx: mpsc::Receiver<NotifyEvent>) {
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // 记录上次已触发每日汇总的本地日期 YYYY-MM-DD (避免同一天重复触发)
    let mut last_summary_day: Option<String> = None;

    loop {
        tokio::select! {
            maybe = rx.recv() => {
                let Some(event) = maybe else { break };
                dispatch_event(&state, event).await;
            }
            _ = tick.tick() => {
                maybe_fire_daily_summary(&state, &mut last_summary_day).await;
            }
        }
    }
}

async fn maybe_fire_daily_summary(state: &AppState, last_summary_day: &mut Option<String>) {
    let (enabled_desktop, summary_time) = {
        let s = state.settings.read().await;
        (
            s.desktop_notifications_enabled && s.desktop_notification_events.iter().any(|e| e == "usage.daily_summary"),
            s.daily_summary_time.clone(),
        )
    };
    let any_plugin = state
        .notifiers()
        .plugins()
        .iter()
        .any(|p| p.plugin.events.iter().any(|e| e == "usage.daily_summary"));
    if !enabled_desktop && !any_plugin {
        return;
    }
    let Some((target_h, target_m)) = parse_hhmm(&summary_time) else {
        return;
    };
    let now = Local::now();
    let today = now.format("%Y-%m-%d").to_string();
    if last_summary_day.as_deref() == Some(today.as_str()) {
        return;
    }
    if (now.hour(), now.minute()) < (target_h, target_m) {
        return;
    }
    *last_summary_day = Some(today);
    if let Ok(event) = build_daily_summary_event(state).await {
        dispatch_event(state, event).await;
    }
}

async fn build_daily_summary_event(state: &AppState) -> crate::error::AppResult<NotifyEvent> {
    let day = local_day_key(Utc::now().timestamp_millis());
    let rows = sqlx::query(
        "SELECT subscription_id, provider_id, real_model_name,
                SUM(request_count) AS reqs,
                SUM(input_tokens) AS inp,
                SUM(output_tokens) AS outp,
                SUM(cache_creation_tokens) AS cc,
                SUM(cache_read_tokens) AS cr
         FROM receipt_stats_daily
         WHERE day = ?
         GROUP BY subscription_id, provider_id, real_model_name",
    )
    .bind(&day)
    .fetch_all(&state.db)
    .await?;

    let pricing = state.pricing();
    let sub_ctx = subscription_contexts(state).await;
    let sub_names: HashMap<String, String> = {
        let map = state.subscriptions.read().await;
        let mut names = HashMap::with_capacity(map.len());
        for (id, rt) in map.iter() {
            names.insert(id.to_string(), rt.read().await.row.display_name.clone());
        }
        names
    };

    let mut total_reqs: u64 = 0;
    let mut total_tokens: u64 = 0;
    let mut totals = CostTotals::default();
    let mut by_sub: HashMap<String, u64> = HashMap::new();

    for r in rows {
        let sub_id: String = r.try_get("subscription_id").unwrap_or_default();
        let provider: String = r.try_get("provider_id").unwrap_or_default();
        let model: String = r.try_get("real_model_name").unwrap_or_default();
        let reqs = r.try_get::<i64, _>("reqs").unwrap_or(0).max(0) as u64;
        let inp = r.try_get::<i64, _>("inp").unwrap_or(0).max(0) as u64;
        let outp = r.try_get::<i64, _>("outp").unwrap_or(0).max(0) as u64;
        let cc = r.try_get::<i64, _>("cc").unwrap_or(0).max(0) as u64;
        let cr = r.try_get::<i64, _>("cr").unwrap_or(0).max(0) as u64;

        total_reqs += reqs;
        let toks = inp + outp;
        total_tokens += toks;
        let label = sub_names.get(&sub_id).cloned().unwrap_or(provider.clone());
        *by_sub.entry(label).or_default() += toks;

        if !pricing.is_empty() {
            let (auth_type, billing) = sub_ctx.get(&sub_id).copied().unzip();
            if let Some(line) = cost_of(
                &pricing,
                &provider,
                &model,
                auth_type,
                billing.unwrap_or_default(),
                Tokens { input_raw: inp, output: outp, cache_read: cr, cache_write: cc },
            ) {
                totals.add(&line);
            }
        }
    }

    let mut ranked: Vec<(String, u64)> = by_sub.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));
    let top_subscriptions = ranked.into_iter().take(3).map(|(n, _)| n).collect();

    let fmt_micros = |m: i64| if m > 0 { format!("{:.2}", m as f64 / 1_000_000.0) } else { String::new() };

    Ok(NotifyEvent::UsageDailySummary {
        requests: total_reqs,
        tokens: total_tokens,
        cost_usd: fmt_micros(totals.usd_micros),
        cost_cny: fmt_micros(totals.cny_micros),
        top_subscriptions,
    })
}

async fn dispatch_event(state: &AppState, event: NotifyEvent) {
    let (locale, desktop_on, desktop_events) = {
        let s = state.settings.read().await;
        (
            TrayLocale::from_pref(&s.preferred_language),
            s.desktop_notifications_enabled,
            s.desktop_notification_events.clone(),
        )
    };
    let kind = event.kind();

    // 1. 内置系统桌面通知 (设计 §7.5)
    if desktop_on
        && (matches!(event, NotifyEvent::Test) || desktop_events.iter().any(|e| e == kind))
        && state.notify.allow("desktop", &event, false)
    {
        let (title, body) = event.render_default(locale);
        if let Err(e) = send_desktop(&state.app_handle, &title, &body) {
            warn!(event = kind, error = %e, "发送系统桌面通知失败");
        }
    }

    // 2. 启用的通知插件 (设计 §7.4)
    let plugins = state.notifiers();
    if plugins.is_empty() {
        return;
    }
    let ctx = event.template_context(locale);
    for p in plugins.plugins() {
        if !matches!(event, NotifyEvent::Test) && !p.plugin.events.iter().any(|e| e == kind) {
            continue;
        }
        let target = format!("plugin:{}", p.plugin_id);
        if !state.notify.allow(&target, &event, true) {
            continue;
        }
        let client = state.http_client.clone();
        let loaded = p.clone();
        let ctx_clone = ctx.clone();
        let notify = state.notify.clone();
        let kind_str = kind.to_string();
        tokio::spawn(async move {
            let rec = send_to_plugin(&client, &loaded, &kind_str, &ctx_clone).await;
            if !rec.ok {
                warn!(plugin = %loaded.plugin_id, event = %kind_str, error = ?rec.error, "通知插件投递失败");
            }
            notify.record_delivery(&loaded.plugin_id, rec);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cc_router_plugin_spec::notifier_plugin::parse;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn limiter_dedupes_within_10m_and_caps_30_per_hour() {
        let mut lim = Limiter::default();
        let t0 = Instant::now();
        assert!(lim.allow("plugin:a", "subscription.auth_failed", "s1", true, t0));
        // 5 分钟内同事件 + 同订阅去重
        assert!(!lim.allow("plugin:a", "subscription.auth_failed", "s1", true, t0 + Duration::from_secs(300)));
        // 不同订阅不去重
        assert!(lim.allow("plugin:a", "subscription.auth_failed", "s2", true, t0 + Duration::from_secs(300)));
        // 10 分钟后放行
        assert!(lim.allow("plugin:a", "subscription.auth_failed", "s1", true, t0 + DEDUPE_WINDOW));

        // 达到每小时 30 条上限后拒绝
        let mut lim2 = Limiter::default();
        for i in 0..MAX_PER_HOUR {
            assert!(lim2.allow("plugin:b", "test", &format!("k{i}"), true, t0));
        }
        assert!(!lim2.allow("plugin:b", "test", "overflow", true, t0 + Duration::from_secs(60)));
        // 满 1 小时窗口滑动后恢复
        assert!(lim2.allow("plugin:b", "test", "after", true, t0 + HOUR));
    }

    #[tokio::test]
    async fn send_to_plugin_retries_once_on_500_and_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/alerts"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/alerts"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let spec = r#"{
            "notifierApi": 1,
            "params": [{ "id": "server", "type": "base_url", "label": "S" }],
            "events": ["test"],
            "request": {
                "method": "POST",
                "url": "{{params.server}}/alerts",
                "body": { "type": "json", "template": { "title": "{{title}}", "body": "{{body}}" } }
            }
        }"#;
        let (plugin, _) = parse(spec).unwrap();
        let mut params = Map::new();
        params.insert("server".into(), Value::String(server.uri()));
        let loaded = LoadedNotifier { plugin_id: "ntfy".into(), version: "1.0.0".into(), plugin, params };
        let ctx = NotifyEvent::Test.template_context(TrayLocale::En);
        let rec = send_to_plugin(&reqwest::Client::new(), &loaded, "test", &ctx).await;
        assert!(rec.ok, "{rec:?}");
        assert_eq!(rec.http_status, Some(200));
    }
}
