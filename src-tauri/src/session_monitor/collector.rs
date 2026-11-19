use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::Utc;
use serde_json::Value;
use uuid::Uuid;

use super::{Client, Context, Event, Monitor, RateLimit, Usage};

const MAX_PAYLOAD: u64 = 8 * 1024 * 1024;
const ROLLOUT_TAIL: u64 = 512 * 1024;
const MAX_PENDING: usize = 512;
const HEARTBEAT_TTL: Duration = Duration::from_secs(15);

pub(super) fn flag(root: &Path, client: Client) -> PathBuf {
    root.join(format!("{}.enabled", client.name()))
}

pub(super) fn private_dir(path: &Path) -> std::io::Result<()> {
    let mut permissions = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        permissions.mode(0o700);
    }
    match permissions.create(path) {
        Err(error)
            if error.kind() == std::io::ErrorKind::AlreadyExists
                && fs::symlink_metadata(path)?.is_dir() =>
        {
            Ok(())
        }
        result => result,
    }
}

fn available(root: &Path, client: Client) -> bool {
    flag(root, client).is_file()
        && fs::metadata(root.join("heartbeat"))
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|time| SystemTime::now().duration_since(time).ok())
            .is_some_and(|age| age < HEARTBEAT_TTL)
}

pub fn cli_entry() -> bool {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--session-hook")) {
        return false;
    }
    let Some(client) = args
        .next()
        .and_then(|arg| arg.to_str().and_then(Client::parse))
    else {
        return true;
    };
    let Some(root) = args
        .next()
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    else {
        return true;
    };

    let mut payload = Vec::new();
    if std::io::stdin()
        .take(MAX_PAYLOAD + 1)
        .read_to_end(&mut payload)
        .is_ok()
        && payload.len() as u64 <= MAX_PAYLOAD
    {
        if available(&root, client) {
            if let Ok(value) = serde_json::from_slice::<Value>(&payload) {
                let _ = collect(&root, client, &value);
            }
        }
    }
    if client == Client::Codex {
        // Stop and SubagentStop expect JSON when stdout is present. An empty object
        // carries no model-facing context and lets the normal lifecycle continue.
        let _ = std::io::stdout().write_all(b"{}\n");
    }
    true
}

fn collect(root: &Path, client: Client, raw: &Value) -> std::io::Result<()> {
    let Some(mut event) = Event::from_payload(client, raw, Utc::now().timestamp_millis()) else {
        return Ok(());
    };
    if client == Client::Codex
        && matches!(
            event.activity.kind.as_str(),
            "session_started" | "model_finished" | "turn_finished" | "compact_finished"
        )
    {
        if let Some(rollout) = raw["transcript_path"].as_str().and_then(owned_rollout) {
            if let Some(snapshot) = rollout_snapshot(&rollout, &event.id, event.activity.at_ms) {
                event.context = snapshot.context;
                event.snapshot_usage = snapshot.usage;
                event.limits = snapshot.limits;
            }
        }
    }
    let inbox = root.join("inbox");
    if fs::read_dir(&inbox)?.take(MAX_PENDING).count() >= MAX_PENDING {
        return Ok(());
    }
    let path = inbox.join(Uuid::new_v4().to_string());
    let tmp = path.with_extension("tmp");
    let mut file = OpenOptions::new();
    file.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        file.mode(0o600);
    }
    let result = (|| {
        let mut file = file.open(&tmp)?;
        serde_json::to_writer(&mut file, &event)?;
        drop(file);
        fs::rename(&tmp, path.with_extension("json"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn owned_rollout(name: &str) -> Option<PathBuf> {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            crate::integrations::home_dir()
                .ok()
                .map(|home| home.join(".codex"))
        })?;
    let directory = home.join("sessions").canonicalize().ok()?;
    let path = Path::new(name).canonicalize().ok()?;
    (path.starts_with(directory)
        && path.extension().is_some_and(|ext| ext == "jsonl")
        && path.metadata().is_ok_and(|meta| meta.is_file()))
    .then_some(path)
}

#[derive(Default)]
struct RolloutSnapshot {
    context: Option<Context>,
    usage: Option<Usage>,
    limits: Option<Vec<RateLimit>>,
}

fn rollout_snapshot(path: &Path, session_id: &str, now: i64) -> Option<RolloutSnapshot> {
    if !path
        .file_stem()?
        .to_str()?
        .strip_suffix(session_id)
        .is_some_and(|prefix| prefix.ends_with('-'))
    {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(ROLLOUT_TAIL)))
        .ok()?;
    let mut tail = Vec::new();
    file.take(ROLLOUT_TAIL).read_to_end(&mut tail).ok()?;
    let mut snapshot = RolloutSnapshot::default();
    for line in tail.rsplit(|&byte| byte == b'\n').skip(1) {
        if !line.windows(13).any(|bytes| bytes == b"\"token_count\"") {
            continue;
        }
        let Ok(event) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        if event["payload"]["type"] != "token_count" {
            continue;
        }
        let payload = &event["payload"];
        let info = &payload["info"];
        if snapshot.usage.is_none() {
            snapshot.usage = Usage::read(&info["total_token_usage"]);
        }
        if snapshot.limits.is_none() {
            let data = &payload["rate_limits"];
            let windows: Vec<_> = ["primary", "secondary"]
                .into_iter()
                .filter_map(|key| {
                    let window = data.get(key)?.as_object()?;
                    Some(RateLimit {
                        window: key.into(),
                        minutes: window.get("window_minutes").and_then(Value::as_u64),
                        used_percentage: window.get("used_percent").and_then(Value::as_f64),
                        resets_at: window.get("resets_at").and_then(Value::as_u64),
                    })
                })
                .collect();
            if !windows.is_empty() {
                snapshot.limits = Some(windows);
            }
        }
        if snapshot.context.is_none() {
            if let (Some(window), Some(used)) = (
                info["model_context_window"]
                    .as_u64()
                    .filter(|value| *value > 0),
                info["last_token_usage"]["total_tokens"].as_u64(),
            ) {
                let at_ms = event["timestamp"]
                    .as_str()
                    .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                    .map(|time| time.timestamp_millis())
                    .unwrap_or(now);
                snapshot.context = Some(Context {
                    used,
                    window,
                    percent: (100.0 * used as f64 / window as f64).min(100.0),
                    approximate: true,
                    at_ms,
                });
            }
        }
        if snapshot.context.is_some() && snapshot.usage.is_some() && snapshot.limits.is_some() {
            break;
        }
    }
    Some(snapshot)
}

fn enabled_at(root: &Path, client: Client) -> Option<i64> {
    let time = fs::metadata(flag(root, client)).ok()?.modified().ok()?;
    i64::try_from(
        time.duration_since(SystemTime::UNIX_EPOCH)
            .ok()?
            .as_millis(),
    )
    .ok()
}

pub(super) async fn drain(monitor: &Monitor) {
    if !monitor.root.is_dir() {
        return;
    }
    let _guard = monitor.drain_lock.lock().await;
    let heartbeat = monitor.root.join("heartbeat");
    if fs::metadata(&heartbeat)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.elapsed().ok())
        .is_none_or(|elapsed| elapsed >= Duration::from_secs(4))
    {
        let _ = tokio::fs::write(heartbeat, b"").await;
    }
    let Ok(mut directory) = tokio::fs::read_dir(monitor.root.join("inbox")).await else {
        return;
    };
    let mut batch = Vec::new();
    let now = Utc::now().timestamp_millis();
    while let Ok(Some(entry)) = directory.next_entry().await {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "tmp") {
            if entry
                .metadata()
                .await
                .ok()
                .and_then(|data| data.modified().ok())
                .and_then(|time| time.elapsed().ok())
                .is_some_and(|age| age > HEARTBEAT_TTL)
            {
                let _ = tokio::fs::remove_file(path).await;
            }
            continue;
        }
        if path.extension().is_none_or(|ext| ext != "json")
            || !entry.file_type().await.is_ok_and(|kind| kind.is_file())
        {
            continue;
        }
        if entry
            .metadata()
            .await
            .is_ok_and(|data| data.len() < 32 * 1024)
        {
            if let Ok(bytes) = tokio::fs::read(&path).await {
                if let Ok(event) = serde_json::from_slice::<Event>(&bytes) {
                    if now.saturating_sub(event.activity.at_ms) < 60_000
                        && enabled_at(&monitor.root, event.client)
                            .is_some_and(|enabled| event.activity.at_ms >= enabled)
                    {
                        batch.push(event);
                    }
                }
            }
        }
        let _ = tokio::fs::remove_file(&path).await;
        if batch.len() >= MAX_PENDING {
            break;
        }
    }
    if !batch.is_empty() {
        batch.sort_by_key(|event| event.activity.at_ms);
        if let Ok(mut store) = monitor.store.lock() {
            for event in batch {
                if enabled_at(&monitor.root, event.client)
                    .is_some_and(|enabled| event.activity.at_ms >= enabled)
                {
                    store.apply(event);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reads_only_complete_codex_usage_records_for_the_session() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("rollout-2026-test-id.jsonl");
        let record = |input| {
            json!({
                "timestamp": "2026-11-19T01:00:00Z", "type":"event_msg", "payload": {
                    "type": "token_count", "info": {
                        "total_token_usage": {"input_tokens":input, "output_tokens": 7},
                        "last_token_usage": {"total_tokens": 50},
                        "model_context_window": 200
                    }
                }
            })
        };
        fs::write(&path, format!("{}\n{}", record(100), record(900))).unwrap();
        assert!(rollout_snapshot(&path, "different-id", 0).is_none());
        let snapshot = rollout_snapshot(&path, "test-id", 0).unwrap();
        assert_eq!(snapshot.context.unwrap().percent, 25.0);
        assert_eq!(snapshot.usage.unwrap().input, 100);
    }

    #[test]
    fn inbox_contains_only_projected_metadata() {
        let root = tempfile::tempdir().unwrap();
        private_dir(&root.path().join("inbox")).unwrap();
        collect(
            root.path(),
            Client::Claude,
            &json!({
                "hook_event_name":"UserPromptSubmit", "session_id":"session-a",
                "prompt":"private prompt", "cwd":"/workspace"
            }),
        )
        .unwrap();
        let files: Vec<_> = fs::read_dir(root.path().join("inbox")).unwrap().collect();
        assert_eq!(files.len(), 1);
        let file = files.into_iter().next().unwrap().unwrap().path();
        let payload = fs::read_to_string(file).unwrap();
        assert!(payload.contains("session-a"));
        assert!(!payload.contains("private prompt"));
    }
}
