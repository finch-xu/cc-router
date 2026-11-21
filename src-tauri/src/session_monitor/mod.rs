//! In-memory client session monitoring with a local, metadata-only hook inbox.

mod collector;
mod plugin;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager, State};

use crate::error::AppResult;
use crate::observability::request_log::{RequestLogEntry, RequestStatus};
use crate::provider::model::AuthType;

#[cfg(test)]
mod tests;

const MAX_SESSIONS: usize = 64;
const MAX_EVENTS: usize = 60;
const STALE_MS: i64 = 15 * 60 * 1000;
const RETAIN_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Client {
    Claude,
    Codex,
}

impl Client {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

fn label(value: &Value, field: &str, limit: usize) -> Option<String> {
    let value = value.get(field)?.as_str()?.trim();
    (!value.is_empty()).then(|| {
        value
            .chars()
            .filter(|c| !c.is_control())
            .take(limit)
            .collect()
    })
}

pub fn session_id(id: &str) -> Option<&str> {
    (!id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_')))
    .then_some(id)
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
struct Usage {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    reasoning: Option<u64>,
    requests: Option<u64>,
}

impl Usage {
    fn read(value: &Value) -> Option<Self> {
        let number = |key| value.get(key).and_then(Value::as_u64);
        let result = Self {
            input: number("input_tokens")?,
            output: number("output_tokens")?,
            cache_read: number("cached_input_tokens")
                .or_else(|| number("cache_read_input_tokens"))
                .unwrap_or_default(),
            cache_write: number("cache_write_input_tokens")
                .or_else(|| number("cache_creation_input_tokens"))
                .unwrap_or_default(),
            reasoning: number("reasoning_output_tokens"),
            requests: None,
        };
        Some(result)
    }

    fn add_request(&mut self, other: Self) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_write = self.cache_write.saturating_add(other.cache_write);
        if let Some(reasoning) = other.reasoning {
            self.reasoning = Some(self.reasoning.unwrap_or_default().saturating_add(reasoning));
        }
        self.requests = Some(self.requests.unwrap_or_default().saturating_add(1));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Context {
    used: u64,
    window: u64,
    percent: f64,
    approximate: bool,
    at_ms: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Cache {
    hit_ratio: Option<f64>,
    expires_at: Option<u64>,
    warm: Option<bool>,
    requests: Option<u64>,
    misses: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RateLimit {
    window: String,
    minutes: Option<u64>,
    used_percentage: Option<f64>,
    resets_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Activity {
    kind: String,
    at_ms: i64,
    tool: Option<String>,
    detail: Option<String>,
    duration_ms: Option<u64>,
    tool_use_id: Option<String>,
    agent_type: Option<String>,
    context_before: Option<f64>,
    context_after: Option<f64>,
}

impl Activity {
    fn new(kind: &str, at_ms: i64) -> Self {
        Self {
            kind: kind.into(),
            at_ms,
            tool: None,
            detail: None,
            duration_ms: None,
            tool_use_id: None,
            agent_type: None,
            context_before: None,
            context_after: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Event {
    client: Client,
    id: String,
    turn_id: Option<String>,
    agent_id: Option<String>,
    agent_type: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    previous_model: Option<String>,
    permission: Option<String>,
    usage: Option<Usage>,
    context: Option<Context>,
    snapshot_usage: Option<Usage>,
    cache: Option<Cache>,
    limits: Option<Vec<RateLimit>>,
    activity: Activity,
}

const COMMON_HOOKS: &[&str] = &[
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "Stop",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "PreCompact",
    "PostCompact",
    "SubagentStart",
    "SubagentStop",
];
const CLAUDE_HOOKS: &[&str] = &[
    "StopFailure",
    "PostToolUseFailure",
    "PermissionDenied",
    "Notification",
    "PostModelSwitch",
    "CwdChanged",
];
const CODEX_HOOKS: &[&str] = &["Interrupt", "PreModelRequest", "PostModelRequest"];

impl Event {
    fn from_payload(client: Client, raw: &Value, at: i64) -> Option<Self> {
        let id = session_id(raw.get("session_id")?.as_str()?)?.to_owned();
        let event = raw.get("hook_event_name").and_then(Value::as_str);
        let window = raw.get("context_window");
        let context = (client == Client::Claude && event.is_none())
            .then_some(window)
            .flatten();
        let kind = match event {
            None if client == Client::Claude && raw.get("model").is_some_and(Value::is_object) => {
                "status_line"
            }
            Some("SessionStart") if raw["source"] == "compact" => "session_restored",
            Some("SessionStart") => "session_started",
            Some("SessionEnd") => "session_ended",
            Some("UserPromptSubmit") if raw["prompt_source"] == "steer" => "steered",
            Some("UserPromptSubmit") => "turn_started",
            Some("Stop") => "turn_finished",
            Some("StopFailure") => "turn_failed",
            Some("Interrupt") => "interrupted",
            Some("PreToolUse") => "tool_called",
            Some("PostToolUse") if raw["executed"] == false => "tool_skipped",
            Some("PostToolUse") => "tool_finished",
            Some("PostToolUseFailure") => "tool_failed",
            Some("PermissionRequest") => "approval_requested",
            Some("PermissionDenied") => "approval_denied",
            Some("Notification") if raw["notification_type"] == "permission_prompt" => {
                "waiting_approval"
            }
            Some("PreCompact") => "compact_started",
            Some("PostCompact") => "compact_finished",
            Some("SubagentStart") => "agent_started",
            Some("SubagentStop") => "agent_finished",
            Some("PostModelSwitch") => "model_switched",
            Some("PreModelRequest") => "model_started",
            Some("PostModelRequest") if raw["status"] == "completed" => "model_finished",
            Some("PostModelRequest") if raw["status"] == "cancelled" => "model_cancelled",
            Some("PostModelRequest") if raw["status"] == "failed" => "model_failed",
            Some("CwdChanged") => "cwd_changed",
            _ => return None,
        };
        let mut activity = Activity::new(kind, at);
        activity.tool = label(raw, "tool_name", 100);
        activity.duration_ms = raw["duration_ms"].as_u64();
        activity.tool_use_id = label(raw, "tool_use_id", 128);
        if !matches!(kind, "agent_started" | "agent_finished") && raw["agent_id"].is_string() {
            activity.agent_type = label(raw, "agent_type", 80);
        }
        if kind == "tool_finished" && client == Client::Codex {
            // Codex can call PostToolUse after a Bash failure. Keep only a structured
            // exit code when the tool supplies one, never copy the tool response.
            let result = &raw["tool_response"];
            if let Some(code) = result["exit_code"].as_i64().filter(|code| *code != 0) {
                activity.kind = "tool_failed".into();
                activity.detail = Some(code.to_string());
            }
        }
        activity.detail = activity.detail.or_else(|| match kind {
            "compact_started" | "compact_finished" => {
                label(raw, "trigger", 20).filter(|s| matches!(s.as_str(), "auto" | "manual"))
            }
            "session_started" => label(raw, "source", 32)
                .filter(|s| matches!(s.as_str(), "startup" | "resume" | "clear" | "fork")),
            "turn_failed" => label(raw, "error", 64).filter(|s| {
                matches!(
                    s.as_str(),
                    "rate_limit"
                        | "authentication_failed"
                        | "oauth_org_not_allowed"
                        | "account_on_hold"
                        | "billing_error"
                        | "invalid_request"
                        | "model_not_found"
                        | "server_error"
                        | "max_output_tokens"
                        | "cloud_credential_error"
                        | "unknown"
                )
            }),
            "tool_failed" => raw["error"]
                .as_str()
                .and_then(|s| s.lines().next())
                .and_then(|s| s.strip_prefix("Exit code "))
                .and_then(|s| s.parse::<i32>().ok())
                .map(|code| code.to_string()),
            "agent_started" | "agent_finished" => label(raw, "agent_type", 80),
            "model_switched" => label(raw, "to_model", 128),
            "model_started" | "model_finished" | "model_failed" | "model_cancelled" => {
                label(raw, "model", 128)
            }
            _ => None,
        });
        let context = context.and_then(|value| {
            let window = value["context_window_size"]
                .as_u64()
                .filter(|window| *window > 0)?;
            let percent = value["used_percentage"].as_f64()?.clamp(0.0, 100.0);
            let used = value["total_input_tokens"].as_u64()?;
            Some(Context {
                used,
                window,
                percent,
                approximate: false,
                at_ms: at,
            })
        });
        let cache = raw["prompt_cache"].as_object().map(|_| {
            let cache = &raw["prompt_cache"];
            Cache {
                hit_ratio: cache["hit_ratio"].as_f64(),
                expires_at: cache["expires_at"].as_u64(),
                warm: cache["warm"].as_bool(),
                requests: cache["requests"].as_u64(),
                misses: cache["misses"].as_u64(),
            }
        });
        let limits = (kind == "status_line").then(|| {
            ["five_hour", "seven_day", "spend_limit"]
                .into_iter()
                .filter_map(|key| {
                    let data = raw["rate_limits"].get(key)?;
                    Some(RateLimit {
                        window: key.to_owned(),
                        minutes: None,
                        used_percentage: data["used_percentage"].as_f64(),
                        resets_at: data["resets_at"].as_u64(),
                    })
                })
                .collect()
        });
        Some(Self {
            client,
            id,
            activity,
            context,
            cache,
            limits,
            turn_id: label(raw, "turn_id", 128).or_else(|| label(raw, "prompt_id", 128)),
            agent_id: label(raw, "agent_id", 128),
            agent_type: label(raw, "agent_type", 80),
            cwd: label(raw, "new_cwd", 500).or_else(|| label(raw, "cwd", 500)),
            model: label(raw, "to_model", 128)
                .or_else(|| label(raw, "model", 128))
                .or_else(|| label(&raw["model"], "id", 128)),
            previous_model: label(raw, "from_model", 128),
            permission: label(raw, "permission_mode", 48),
            usage: (event == Some("PostModelRequest"))
                .then(|| Usage::read(&raw["token_usage"]))
                .flatten(),
            snapshot_usage: None,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
struct Agent {
    id: String,
    kind: Option<String>,
    running: bool,
}

#[derive(Debug, Clone, Serialize)]
struct Route {
    provider_id: String,
    subscription_id: uuid::Uuid,
    virtual_model: String,
    model: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Session {
    client: Client,
    id: String,
    cwd: Option<String>,
    model: Option<String>,
    permission: Option<String>,
    status: &'static str,
    at_ms: i64,
    #[serde(skip)]
    state_at_ms: i64,
    turn_id: Option<String>,
    turn_started_ms: Option<i64>,
    turn_finished_ms: Option<i64>,
    context: Option<Context>,
    cache: Option<Cache>,
    limits: Vec<RateLimit>,
    observed_usage: Option<Usage>,
    cumulative_usage: Option<Usage>,
    proxy_usage: Option<Usage>,
    route: Option<Route>,
    agents: Vec<Agent>,
    activities: VecDeque<Activity>,
    #[serde(skip)]
    compact_before: Option<f64>,
    #[serde(skip)]
    compact_finished_ms: Option<i64>,
    #[serde(skip)]
    compact_started_ms: Option<i64>,
    #[serde(skip)]
    previous_status: Option<&'static str>,
    #[serde(skip)]
    proxy_ids: VecDeque<uuid::Uuid>,
}

impl Session {
    fn new(client: Client, id: String, at_ms: i64) -> Self {
        Self {
            client,
            id,
            at_ms,
            status: "unknown",
            state_at_ms: at_ms,
            cwd: None,
            model: None,
            permission: None,
            turn_id: None,
            turn_started_ms: None,
            turn_finished_ms: None,
            context: None,
            cache: None,
            limits: Vec::new(),
            observed_usage: None,
            cumulative_usage: None,
            proxy_usage: None,
            route: None,
            agents: Vec::new(),
            activities: VecDeque::new(),
            compact_before: None,
            compact_finished_ms: None,
            compact_started_ms: None,
            previous_status: None,
            proxy_ids: VecDeque::new(),
        }
    }

    fn push_activity(&mut self, activity: Activity) {
        let index = self
            .activities
            .partition_point(|entry| entry.at_ms > activity.at_ms);
        self.activities.insert(index, activity);
        self.activities.truncate(MAX_EVENTS);
    }
}

#[derive(Default)]
struct Store {
    sessions: HashMap<(Client, String), Session>,
    last_report: HashMap<Client, i64>,
    errors: HashMap<Client, String>,
}

impl Store {
    fn apply(&mut self, event: Event) {
        let mut activity = event.activity;
        let at = activity.at_ms;
        self.last_report
            .entry(event.client)
            .and_modify(|last| *last = (*last).max(at))
            .or_insert(at);
        let kind = activity.kind.as_str();
        let key = (event.client, event.id.clone());
        if self.sessions.len() >= MAX_SESSIONS && !self.sessions.contains_key(&key) {
            if let Some(old) = self
                .sessions
                .iter()
                .min_by_key(|(_, session)| session.at_ms)
                .map(|(key, _)| key.clone())
            {
                self.sessions.remove(&old);
            }
        }
        let session = self
            .sessions
            .entry(key)
            .or_insert_with(|| Session::new(event.client, event.id, at));
        if let Some(usage) = event.usage {
            session
                .observed_usage
                .get_or_insert_default()
                .add_request(usage);
        }
        if let Some(snapshot) = event.snapshot_usage {
            let current = &mut session.cumulative_usage;
            if current.is_none_or(|usage| {
                usage.input <= snapshot.input && usage.output <= snapshot.output
            }) || kind == "session_started"
            {
                *current = Some(snapshot);
            }
        }
        if activity.duration_ms.is_none()
            && matches!(kind, "tool_finished" | "tool_failed")
            && activity.tool_use_id.is_some()
        {
            if let Some(start) = session
                .activities
                .iter()
                .find(|item| item.kind == "tool_called" && item.tool_use_id == activity.tool_use_id)
            {
                activity.duration_ms = u64::try_from(at - start.at_ms).ok();
            }
        }
        let current = at >= session.at_ms;
        let own_turn = event.turn_id.is_none()
            || session.turn_id.is_none()
            || event.turn_id == session.turn_id
            || kind == "turn_started";
        let agent_lifecycle = matches!(kind, "agent_started" | "agent_finished");
        let child_event = event.agent_id.is_some() && !agent_lifecycle;
        if current {
            session.at_ms = at;
            if !child_event && !agent_lifecycle {
                session.cwd = event.cwd.or_else(|| session.cwd.take());
                session.permission = event.permission.or_else(|| session.permission.take());
                if event.client == Client::Codex && kind == "model_started" {
                    if let (Some(previous), Some(next)) = (&session.model, &event.model) {
                        if previous != next {
                            let mut switched = Activity::new("model_switched", at);
                            switched.detail = Some(format!("{previous} → {next}"));
                            session.push_activity(switched);
                        }
                    }
                }
                session.model = event.model.or_else(|| session.model.take());
            }
            if kind == "status_line" {
                session.cache = event.cache;
            }
            if let Some(limits) = event.limits {
                session.limits = limits;
            }
            if agent_lifecycle || (own_turn && !child_event) {
                if !matches!(
                    kind,
                    "status_line"
                        | "agent_started"
                        | "agent_finished"
                        | "cwd_changed"
                        | "model_switched"
                ) {
                    session.state_at_ms = at;
                }
                let kind = activity.kind.as_str();
                match kind {
                    "session_started" => {
                        session.status = "idle";
                        session.turn_id = None;
                        session.turn_started_ms = None;
                        session.turn_finished_ms = None;
                        session.context = None;
                        session.agents.clear();
                    }
                    "turn_started" => {
                        if event.client == Client::Codex
                            && session.turn_finished_ms.is_none()
                            && event.turn_id.is_some()
                            && event.turn_id == session.turn_id
                        {
                            activity.kind = "steered".into();
                        } else {
                            session.status = "running";
                            session.turn_id = event.turn_id;
                            session.turn_started_ms = Some(at);
                            session.turn_finished_ms = None;
                        }
                    }
                    "session_ended" | "turn_finished" | "turn_failed" | "interrupted" => {
                        session.status = match kind {
                            "session_ended" => "ended",
                            "turn_finished" => "idle",
                            "turn_failed" => "failed",
                            _ => "interrupted",
                        };
                        if session.turn_finished_ms.is_none() {
                            session.turn_finished_ms = session.turn_started_ms.map(|_| at);
                        }
                    }
                    "approval_requested" => session.status = "approval_requested",
                    "waiting_approval" => session.status = "waiting_approval",
                    "tool_called" => session.status = "tool_requested",
                    "tool_finished" | "tool_skipped" | "tool_failed" | "approval_denied"
                    | "model_finished" | "model_failed" | "model_cancelled" => {
                        session.status = "running"
                    }
                    "model_started" => {
                        session.status = "model_running";
                        if session
                            .activities
                            .front()
                            .is_some_and(|item| item.kind == "model_failed")
                        {
                            activity.kind = "model_retry".into();
                        }
                    }
                    "compact_started" => {
                        session.previous_status = Some(session.status);
                        session.status = "compacting";
                        session.compact_before =
                            session.context.as_ref().map(|value| value.percent);
                        session.compact_finished_ms = None;
                        session.compact_started_ms = Some(at);
                    }
                    "compact_finished" => {
                        session.status = session.previous_status.take().unwrap_or("running");
                        session.context = None;
                        session.compact_finished_ms = Some(at);
                        activity.context_before = session.compact_before.take();
                    }
                    "agent_started" | "agent_finished" => {
                        if let Some(id) = event.agent_id {
                            let running = kind == "agent_started";
                            if let Some(agent) =
                                session.agents.iter_mut().find(|item| item.id == id)
                            {
                                agent.running = running;
                            } else if session.agents.len() < 64 {
                                session.agents.push(Agent {
                                    id,
                                    kind: event.agent_type,
                                    running,
                                });
                            }
                        }
                    }
                    "model_switched" => {
                        if let (Some(from), Some(to)) =
                            (event.previous_model, activity.detail.take())
                        {
                            activity.detail = Some(format!("{from} → {to}"));
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(context) = event.context.filter(|context| {
            !child_event
                && session
                    .context
                    .as_ref()
                    .is_none_or(|previous| context.at_ms >= previous.at_ms)
                && session.compact_finished_ms.is_none_or(|finished| {
                    context.at_ms > session.compact_started_ms.unwrap_or(finished)
                })
        }) {
            if let Some(finished) = session.compact_finished_ms.take() {
                session.compact_started_ms = None;
                if let Some(compact) = session.activities.iter_mut().find(|activity| {
                    activity.kind == "compact_finished" && activity.at_ms == finished
                }) {
                    compact.context_after = Some(context.percent);
                }
            }
            session.context = Some(context);
        }
        if activity.kind != "status_line" {
            session.push_activity(activity);
        }
    }

    fn snapshot(&mut self, now: i64) -> Vec<Session> {
        self.sessions
            .retain(|_, value| now.saturating_sub(value.at_ms) < RETAIN_MS);
        let mut rows: Vec<_> = self.sessions.values().cloned().collect();
        for row in &mut rows {
            if row.status != "ended" && now.saturating_sub(row.state_at_ms) >= STALE_MS {
                row.status = "unknown";
                if row.turn_finished_ms.is_none() {
                    row.turn_finished_ms = row.turn_started_ms.map(|_| row.state_at_ms);
                }
            }
        }
        rows.sort_by(|a, b| b.at_ms.cmp(&a.at_ms).then_with(|| a.id.cmp(&b.id)));
        rows
    }
}

pub struct Monitor {
    root: PathBuf,
    store: Mutex<Store>,
    install_lock: tokio::sync::Mutex<()>,
    drain_lock: tokio::sync::Mutex<()>,
}

impl Monitor {
    pub fn new(app_data: PathBuf) -> Self {
        Self {
            root: app_data.join("session-monitor"),
            store: Mutex::default(),
            install_lock: tokio::sync::Mutex::default(),
            drain_lock: tokio::sync::Mutex::default(),
        }
    }

    pub async fn clear_opt_in(&self) {
        let _guard = self.install_lock.lock().await;
        for client in [Client::Claude, Client::Codex] {
            // Factory reset stops consent to collect. Keep the connection visible
            // so the user can still remove the external plugin from the desktop.
            let path = collector::flag(&self.root, client);
            if let Err(error) =
                tokio::fs::rename(&path, plugin::pause_flag(&self.root, client)).await
            {
                if error.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(?error, "session monitoring opt-in could not be cleared");
                }
            }
        }
        collector::drain(self).await;
        if let Ok(mut store) = self.store.lock() {
            *store = Store::default();
        }
    }

    pub fn start(app: AppHandle) {
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let Some(monitor) = app.try_state::<Self>() else {
                    return;
                };
                collector::drain(&monitor).await;
            }
        });
    }
}

fn proxy_input(auth: Option<AuthType>, input: u32, read: u32, write: u32) -> u64 {
    // Anthropic passthrough reports uncached input separately. Responses and
    // Gemini preserve their original input totals, which already include cache.
    u64::from(input)
        + if auth == Some(AuthType::ApiKey) {
            u64::from(read) + u64::from(write)
        } else {
            0
        }
}

pub async fn observe_proxy(app: &AppHandle, entry: &RequestLogEntry, auth_type: Option<AuthType>) {
    let Some((client, id)) = entry.monitor_session.as_ref() else {
        return;
    };
    let Some(monitor) = app.try_state::<Monitor>() else {
        return;
    };
    let known = monitor
        .store
        .lock()
        .is_ok_and(|store| store.sessions.contains_key(&(*client, id.clone())));
    if !known && collector::flag(&monitor.root, *client).is_file() {
        // A very fast response can finish between the hook write and the scheduled drain.
        collector::drain(&monitor).await;
    }
    let Ok(mut store) = monitor.store.lock() else {
        return;
    };
    let Some(session) = store.sessions.get_mut(&(*client, id.clone())) else {
        return;
    };
    if session.proxy_ids.contains(&entry.id) {
        return;
    }
    session.proxy_ids.push_back(entry.id);
    if session.proxy_ids.len() > 256 {
        session.proxy_ids.pop_front();
    }
    let success = matches!(entry.status, RequestStatus::Success);
    let kind = if success {
        "proxy_completed"
    } else {
        "proxy_failed"
    };
    let mut activity = Activity::new(kind, Utc::now().timestamp_millis());
    activity.detail = if success {
        Some(entry.provider_id.clone())
    } else {
        entry.http_status.map(|status| status.to_string())
    };
    activity.duration_ms = entry.total_latency_ms;
    session.push_activity(activity);
    if success {
        session.route = Some(Route {
            provider_id: entry.provider_id.clone(),
            subscription_id: entry.subscription_id,
            virtual_model: entry.virtual_model_name.as_str().into(),
            model: entry
                .response_model_name
                .as_ref()
                .unwrap_or(&entry.real_model_name)
                .clone(),
        });
    }
    if entry.upstream_input_tokens.is_some()
        || entry.upstream_output_tokens.is_some()
        || entry.upstream_cache_read.is_some()
        || entry.upstream_cache_creation.is_some()
    {
        session
            .proxy_usage
            .get_or_insert_default()
            .add_request(Usage {
                input: proxy_input(
                    auth_type,
                    entry.upstream_input_tokens.unwrap_or_default(),
                    entry.upstream_cache_read.unwrap_or_default(),
                    entry.upstream_cache_creation.unwrap_or_default(),
                ),
                output: u64::from(entry.upstream_output_tokens.unwrap_or_default()),
                cache_read: u64::from(entry.upstream_cache_read.unwrap_or_default()),
                cache_write: u64::from(entry.upstream_cache_creation.unwrap_or_default()),
                reasoning: None,
                requests: None,
            });
    }
}

#[tauri::command]
pub fn get_live_sessions(state: State<'_, Monitor>) -> Vec<Session> {
    state
        .store
        .lock()
        .map(|mut s| s.snapshot(Utc::now().timestamp_millis()))
        .unwrap_or_default()
}

#[tauri::command]
pub async fn session_monitor_status(state: State<'_, Monitor>) -> AppResult<Vec<plugin::Status>> {
    plugin::status(&state).await
}

#[tauri::command]
pub async fn configure_session_monitor(
    state: State<'_, Monitor>,
    client: Client,
    action: MonitorAction,
) -> AppResult<()> {
    let _guard = state.install_lock.lock().await;
    let result = plugin::configure(&state, client, action).await;
    if let Ok(mut store) = state.store.lock() {
        match &result {
            Ok(()) => {
                store.errors.remove(&client);
            }
            Err(error) => {
                store.errors.insert(client, error.to_string());
            }
        }
    }
    result
}

#[tauri::command]
pub async fn session_monitor_client_version(client: Client) -> Option<String> {
    plugin::version(client).await
}

pub use plugin::Action as MonitorAction;

pub use collector::cli_entry;
