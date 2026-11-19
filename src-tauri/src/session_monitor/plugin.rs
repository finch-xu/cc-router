use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::{fs, process::Command};

use super::collector::{flag, private_dir};
use super::{Client, Monitor, CLAUDE_HOOKS, CODEX_HOOKS, COMMON_HOOKS};
use crate::error::{AppError, AppResult};

const MARKETPLACE: &str = "cc-router-local";
const PLUGIN: &str = "cc-router-session-monitor";
const PLUGIN_VERSION: &str = "1.0.0";
const ORIGINAL_STATUS: &str = "status-line-original.json";
const INSTALLED_STATUS: &str = "status-line-installed.json";

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Enable,
    Pause,
    Clear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Connection {
    Unconfigured,
    Enabled,
    Paused,
    CleanupPending,
}

#[derive(Serialize)]
pub struct Status {
    client: Client,
    connection: Connection,
    last_report_ms: Option<i64>,
    plugin_version: Option<&'static str>,
    error: Option<String>,
}

pub(super) fn cleanup_flag(root: &Path, client: Client) -> PathBuf {
    root.join(format!("{}.cleanup-pending", client.name()))
}

pub(super) fn pause_flag(root: &Path, client: Client) -> PathBuf {
    root.join(format!("{}.paused", client.name()))
}

async fn client_status(root: &Path, client: Client) -> AppResult<Connection> {
    Ok(if fs::try_exists(cleanup_flag(root, client)).await? {
        Connection::CleanupPending
    } else if fs::try_exists(flag(root, client)).await? {
        Connection::Enabled
    } else if fs::try_exists(pause_flag(root, client)).await? {
        Connection::Paused
    } else {
        Connection::Unconfigured
    })
}

pub(super) async fn status(monitor: &Monitor) -> AppResult<Vec<Status>> {
    let mut rows = Vec::new();
    for client in [Client::Claude, Client::Codex] {
        let connection = client_status(&monitor.root, client).await?;
        let (last_report_ms, error) = monitor
            .store
            .lock()
            .map(|store| {
                (
                    store.last_report.get(&client).copied(),
                    store.errors.get(&client).cloned(),
                )
            })
            .unwrap_or_default();
        rows.push(Status {
            client,
            connection,
            last_report_ms,
            error,
            plugin_version: matches!(connection, Connection::Enabled | Connection::Paused)
                .then_some(PLUGIN_VERSION),
        });
    }
    Ok(rows)
}

fn single_quote(s: &str, powershell: bool) -> String {
    format!(
        "'{}'",
        s.replace('\'', if powershell { "''" } else { "'\"'\"'" })
    )
}

fn hook_command(exe: &Path, root: &Path, client: Client, powershell: bool) -> AppResult<String> {
    let path = |value: &Path| {
        let text = value
            .to_str()
            .ok_or_else(|| AppError::BadRequest("Session monitor requires UTF-8 paths".into()))?;
        let text = if cfg!(windows) {
            text.replace('\\', "/")
        } else {
            text.to_owned()
        };
        Ok::<_, AppError>(single_quote(&text, powershell))
    };
    let prefix = if powershell { "& " } else { "" };
    Ok(format!(
        "{prefix}{} --session-hook {} {}",
        path(exe)?,
        client.name(),
        path(root)?
    ))
}

async fn write_json(path: &Path, value: &Value) -> AppResult<()> {
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|error| AppError::internal(error.to_string()))?;
    write_private(path, bytes).await
}

async fn write_private(path: &Path, bytes: Vec<u8>) -> AppResult<()> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        use std::io::Write;
        let parent = path
            .parent()
            .ok_or_else(|| AppError::BadRequest("Missing parent directory".into()))?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(&bytes)?;
        file.persist(path).map_err(|error| error.error)?;
        Ok(())
    })
    .await
    .map_err(|error| AppError::internal(error.to_string()))?
}

async fn read_json(path: &Path) -> AppResult<Option<Value>> {
    match fs::read(path).await {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| AppError::BadRequest(format!("{}: {error}", path.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn hooks(exe: &Path, root: &Path, client: Client) -> AppResult<Value> {
    let mut handler = json!({
        "type": "command", "command": hook_command(exe, root, client, false)?,
        "timeout": 2
    });
    if cfg!(windows) {
        let command = hook_command(exe, root, client, true)?;
        if client == Client::Claude {
            handler["shell"] = json!("powershell");
            handler["command"] = json!(command);
        } else {
            handler["commandWindows"] = json!(command);
        }
    }
    let extra = match client {
        Client::Claude => CLAUDE_HOOKS,
        Client::Codex => CODEX_HOOKS,
    };
    let events: Map<String, Value> = COMMON_HOOKS
        .iter()
        .chain(extra)
        .map(|name| ((*name).to_owned(), json!([{ "hooks": [handler.clone()] }])))
        .collect();
    Ok(json!({ "hooks": events }))
}

fn marketplace_root(root: &Path, client: Client) -> PathBuf {
    root.join(format!("marketplace-{}", client.name()))
}

async fn prepare_marketplace(root: &Path, exe: &Path, client: Client) -> AppResult<PathBuf> {
    let marketplace = marketplace_root(root, client);
    let plugin = marketplace.join("plugins").join(PLUGIN);
    let (manifest_dir, catalog_dir, entry) = match client {
        Client::Claude => (
            ".claude-plugin",
            ".claude-plugin",
            json!({
                "name": PLUGIN, "source": format!("./plugins/{PLUGIN}")
            }),
        ),
        Client::Codex => (
            ".codex-plugin",
            ".agents/plugins",
            json!({
                "name": PLUGIN, "source": {"source": "local", "path": format!("./plugins/{PLUGIN}")},
                "policy": {"installation": "AVAILABLE", "authentication": "ON_INSTALL"},
                "category": "Productivity"
            }),
        ),
    };
    fs::create_dir_all(marketplace.join(catalog_dir)).await?;
    fs::create_dir_all(plugin.join(manifest_dir)).await?;
    fs::create_dir_all(plugin.join("hooks")).await?;
    write_json(
        &marketplace.join(catalog_dir).join("marketplace.json"),
        &json!({
            "name": MARKETPLACE, "owner": {"name": "cc-router"}, "plugins": [entry]
        }),
    )
    .await?;
    write_json(&plugin.join(manifest_dir).join("plugin.json"), &json!({
        "name": PLUGIN, "version": PLUGIN_VERSION, "description": "Local session metadata for cc-router"
    })).await?;
    write_json(&plugin.join("hooks/hooks.json"), &hooks(exe, root, client)?).await?;
    Ok(marketplace)
}

fn executable(client: Client) -> PathBuf {
    let program = client.name();
    let mut names = vec![program.to_owned()];
    if cfg!(windows) {
        names = vec![
            format!("{program}.exe"),
            format!("{program}.cmd"),
            program.to_owned(),
        ];
    }
    let mut paths: Vec<_> = std::env::var_os("PATH")
        .as_ref()
        .map(|value| std::env::split_paths(value).collect())
        .unwrap_or_default();
    if let Ok(home) = crate::integrations::home_dir() {
        paths.extend([home.join(".local/bin"), home.join(".cargo/bin")]);
    }
    if cfg!(target_os = "macos") {
        paths.extend([
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ]);
    }
    if let Some(app_data) = std::env::var_os("APPDATA") {
        paths.push(PathBuf::from(app_data).join("npm"));
    }
    paths
        .iter()
        .flat_map(|path| names.iter().map(|name| path.join(name)))
        .find(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(program))
}

async fn cli_output(cli: &Path, args: &[&str], timeout: u64) -> AppResult<std::process::Output> {
    let mut child = Command::new(cli);
    child.args(args).stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    child.creation_flags(0x08000000);
    tokio::time::timeout(std::time::Duration::from_secs(timeout), child.output())
        .await
        .map_err(|_| AppError::BadRequest("CLI plugin command timed out".into()))?
        .map_err(|error| AppError::BadRequest(format!("{}: {error}", cli.display())))
}

pub(super) async fn version(client: Client) -> Option<String> {
    let output = cli_output(&executable(client), &["--version"], 3)
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(100).collect())
}

async fn run(cli: &Path, args: &[&str]) -> AppResult<()> {
    let output = cli_output(cli, args, 20).await?;
    if !output.status.success() {
        if args.starts_with(&["plugin", "uninstall"])
            && args.contains(&"--json")
            && serde_json::from_slice::<Value>(&output.stdout)
                .is_ok_and(|value| value["failureCode"] == "not_installed")
        {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if args.starts_with(&["plugin", "marketplace", "remove"]) {
            let not_configured = serde_json::from_slice::<Value>(&output.stdout)
                .is_ok_and(|value| value["failureCode"] == "not_configured")
                || stderr.lines().any(|line| {
                    line.trim()
                        == "Error: marketplace `cc-router-local` is not configured or installed"
                });
            if not_configured {
                return Ok(());
            }
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail: String = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        }
        .chars()
        .take(1300)
        .collect();
        return Err(AppError::BadRequest(format!(
            "{} {}: {detail}",
            cli.display(),
            args.join(" ")
        )));
    }
    Ok(())
}

fn claude_settings() -> AppResult<PathBuf> {
    let override_dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from);
    match override_dir {
        Some(path) => Ok(path.join("settings.json")),
        None => Ok(crate::integrations::home_dir()?.join(".claude/settings.json")),
    }
}

fn settings_object(settings: &mut Value) -> AppResult<&mut Map<String, Value>> {
    settings
        .as_object_mut()
        .ok_or_else(|| AppError::BadRequest("Claude settings.json must be an object".into()))
}

fn powershell_encoded(script: &str) -> String {
    use base64::Engine;
    let utf16: Vec<_> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
    format!("powershell -NoProfile -EncodedCommand {encoded}")
}

async fn prepare_status_wrapper(root: &Path, exe: &Path, original: &Value) -> AppResult<String> {
    let original_command = original["command"].as_str();
    // The wrapper still runs the original status line if cc-router was moved,
    // uninstalled, or the collector could not start.
    if cfg!(windows) {
        let collector = hook_command(exe, root, Client::Claude, true)?;
        let exe = single_quote(&exe.to_string_lossy().replace('\\', "/"), true);
        let original = original_command.map(|value| {
            let fallback = powershell_encoded(value);
            if original["shell"] == "powershell" {
                format!("$payload | {fallback}\n")
            } else {
                let value = single_quote(value, true);
                format!(concat!(
                    "$gitBash = $env:CLAUDE_CODE_GIT_BASH_PATH\n",
                    "if (!$gitBash) {{ $gitBash = Join-Path $env:ProgramFiles 'Git/bin/bash.exe' }}\n",
                    "if (Test-Path -LiteralPath $gitBash) {{ $payload | & $gitBash -c {value} }} ",
                    "else {{ $payload | {fallback} }}\n"
                ), value = value, fallback = fallback)
            }
        }).unwrap_or_default();
        let script = format!(
            concat!(
                "$OutputEncoding = [System.Text.UTF8Encoding]::new($false)\n",
                "$payload = [Console]::In.ReadToEnd()\n",
                "if (Test-Path -LiteralPath {}) {{ $payload | {} | Out-Null }}\n",
                "{}"
            ),
            exe, collector, original
        );
        // PowerShell command encoding keeps this usable from either Git Bash or
        // PowerShell without requiring an unsigned script file in the profile.
        let command = powershell_encoded(&script);
        if command.len() > 24_000 {
            return Err(AppError::BadRequest(
                "Existing Claude status line is too long to wrap on Windows".into(),
            ));
        }
        Ok(command)
    } else {
        let wrapper = root.join("status-line.sh");
        let collector = hook_command(exe, root, Client::Claude, false)?;
        let exe = single_quote(&exe.to_string_lossy(), false);
        let original = original_command
            .map(|value| {
                format!(
                    "printf '%s\\n' \"$payload\" | /bin/bash -c {}\n",
                    single_quote(value, false)
                )
            })
            .unwrap_or_default();
        let script = format!(
            concat!(
                "payload=$(cat)\n",
                "if [ -x {} ]; then printf '%s\\n' \"$payload\" | {} >/dev/null 2>&1 || :; fi\n",
                "{}"
            ),
            exe, collector, original
        );
        write_private(&wrapper, script.into_bytes()).await?;
        Ok(format!(
            "/bin/bash {}",
            single_quote(&wrapper.to_string_lossy(), false)
        ))
    }
}

async fn install_status_line(root: &Path, exe: &Path) -> AppResult<()> {
    let settings_path = claude_settings()?;
    let mut settings = read_json(&settings_path)
        .await?
        .unwrap_or_else(|| json!({}));
    let settings_map = settings_object(&mut settings)?;
    let current = settings_map
        .get("statusLine")
        .cloned()
        .unwrap_or(Value::Null);
    let installed = read_json(&root.join(INSTALLED_STATUS)).await?;
    let already_installed = installed
        .as_ref()
        .is_some_and(|installed| owns_status_line(&current, installed));
    let original = if already_installed {
        read_json(&root.join(ORIGINAL_STATUS))
            .await?
            .unwrap_or_default()
    } else {
        current.clone()
    };
    if !original.is_null() && original["command"].as_str().is_none() {
        return Err(AppError::BadRequest(
            "Claude statusLine does not contain a command".into(),
        ));
    }
    let mut replacement = current.as_object().cloned().unwrap_or_default();
    let command = prepare_status_wrapper(root, exe, &original).await?;
    replacement.insert("type".into(), json!("command"));
    replacement.insert("command".into(), json!(command));
    let replacement = Value::Object(replacement);
    if replacement == current {
        return Ok(());
    }
    write_json(&root.join(ORIGINAL_STATUS), &original).await?;
    write_json(&root.join(INSTALLED_STATUS), &replacement).await?;
    settings_map.insert("statusLine".into(), replacement);
    if let Some(parent) = settings_path.parent() {
        fs::create_dir_all(parent).await?;
    }
    write_json(&settings_path, &settings).await
}

fn owns_status_line(current: &Value, installed: &Value) -> bool {
    installed["command"].as_str().is_some() && current["command"] == installed["command"]
}

async fn restore_status_line(root: &Path) -> AppResult<()> {
    let Some(installed) = read_json(&root.join(INSTALLED_STATUS)).await? else {
        return Ok(());
    };
    let settings_path = claude_settings()?;
    let Some(mut settings) = read_json(&settings_path).await? else {
        return Ok(());
    };
    let map = settings_object(&mut settings)?;
    if map
        .get("statusLine")
        .is_some_and(|current| owns_status_line(current, &installed))
    {
        let original = read_json(&root.join(ORIGINAL_STATUS))
            .await?
            .unwrap_or_default();
        if let Some(command) = original.get("command") {
            map["statusLine"]["command"] = command.clone();
        } else {
            map.remove("statusLine");
        }
        write_json(&settings_path, &settings).await?;
    }
    Ok(())
}

async fn owns_marketplace(root: &Path, cli: &Path, client: Client) -> AppResult<Option<bool>> {
    let output = cli_output(cli, &["plugin", "marketplace", "list", "--json"], 5).await?;
    if !output.status.success() {
        return Err(AppError::BadRequest(
            "Could not read the CLI plugin marketplaces".into(),
        ));
    }
    let catalog: Value = serde_json::from_slice(&output.stdout)?;
    let (items, path_key) = match client {
        Client::Claude => (&catalog, "path"),
        Client::Codex => (&catalog["marketplaces"], "root"),
    };
    let items = items
        .as_array()
        .ok_or_else(|| AppError::BadRequest("Unrecognized CLI marketplace list".into()))?;
    let Some(entry) = items.iter().find(|entry| entry["name"] == MARKETPLACE) else {
        return Ok(None);
    };
    let expected = marketplace_root(root, client);
    let owned = entry[path_key].as_str().is_some_and(|path| {
        let path = Path::new(path);
        path == expected
            || match (path.canonicalize(), expected.canonicalize()) {
                (Ok(path), Ok(expected)) => path == expected,
                _ => false,
            }
    });
    Ok(Some(owned))
}

async fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path).await {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

pub(super) async fn configure(monitor: &Monitor, client: Client, action: Action) -> AppResult<()> {
    let root = &monitor.root;
    let cli = executable(client);
    let selector = format!("{PLUGIN}@{MARKETPLACE}");
    let marker = flag(root, client);
    let cleanup = cleanup_flag(root, client);
    let paused = pause_flag(root, client);
    let current = client_status(root, client).await?;
    if matches!(action, Action::Pause) {
        if current == Connection::Enabled {
            fs::rename(&marker, &paused).await?;
            stop_sessions(monitor, client).await;
        }
        return Ok(());
    }
    if matches!(action, Action::Enable) {
        match current {
            Connection::Enabled => return Ok(()),
            Connection::Paused => {
                // A new marker timestamp excludes events from before the pause.
                fs::write(&marker, b"").await?;
                remove_if_present(&paused).await?;
                return Ok(());
            }
            Connection::CleanupPending => {
                return Err(AppError::BadRequest(
                    "Finish removing the current connection before reconnecting".into(),
                ))
            }
            Connection::Unconfigured => {}
        }
        private_dir(root)?;
        private_dir(&root.join("inbox"))?;
        let exe = std::env::var_os("APPIMAGE")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or(std::env::current_exe()?);
        let marketplace = prepare_marketplace(root, &exe, client).await?;
        let marketplace = marketplace
            .to_str()
            .ok_or_else(|| AppError::BadRequest("Session monitor requires UTF-8 paths".into()))?;
        if owns_marketplace(root, &cli, client).await? == Some(false) {
            return Err(AppError::BadRequest(
                "The cc-router-local marketplace name is already in use".into(),
            ));
        }
        // Persist a recoverable state before the first external config write.
        fs::write(&cleanup, b"").await?;
        run(&cli, &["plugin", "marketplace", "add", marketplace]).await?;
        if client == Client::Claude {
            run(&cli, &["plugin", "install", &selector]).await?;
            install_status_line(root, &exe).await?;
        } else {
            run(&cli, &["plugin", "add", &selector]).await?;
        }
        fs::write(marker, b"").await?;
        remove_if_present(&cleanup).await?;
    } else {
        match current {
            Connection::Enabled => fs::rename(&marker, &cleanup).await?,
            Connection::Paused => fs::rename(&paused, &cleanup).await?,
            Connection::CleanupPending => {}
            Connection::Unconfigured => return Ok(()),
        }
        remove_if_present(&marker).await?;
        remove_if_present(&paused).await?;
        stop_sessions(monitor, client).await;
        if client == Client::Claude {
            restore_status_line(root).await?;
        }
        let owned = owns_marketplace(root, &cli, client).await?;
        if owned != Some(false) {
            match client {
                Client::Claude => run(&cli, &["plugin", "uninstall", &selector, "--json"]).await?,
                Client::Codex => run(&cli, &["plugin", "remove", &selector]).await?,
            }
        }
        if owned == Some(true) {
            let mut args = vec!["plugin", "marketplace", "remove", MARKETPLACE, "--json"];
            if client == Client::Claude {
                args.extend(["--scope", "user"]);
            }
            run(&cli, &args).await?;
        }
        match fs::remove_dir_all(marketplace_root(root, client)).await {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            result => result?,
        }
        if client == Client::Claude {
            // Existing terminals may still use status-line.sh until reopened.
            // Its embedded original command works without these ownership records.
            remove_if_present(&root.join(ORIGINAL_STATUS)).await?;
            remove_if_present(&root.join(INSTALLED_STATUS)).await?;
        }
        remove_if_present(&cleanup).await?;
        if let Ok(mut store) = monitor.store.lock() {
            store.last_report.remove(&client);
        }
    }
    Ok(())
}

async fn stop_sessions(monitor: &Monitor, client: Client) {
    super::collector::drain(monitor).await;
    if let Ok(mut store) = monitor.store.lock() {
        store.sessions.retain(|(tool, _), _| *tool != client);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_hook_paths_for_the_target_shell() {
        assert_eq!(single_quote("file's name", false), "'file'\"'\"'s name'");
        assert_eq!(single_quote("file's name", true), "'file''s name'");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn original_status_line_keeps_working_without_the_app_executable() {
        use tokio::io::AsyncWriteExt;
        let root = tempfile::tempdir().unwrap();
        let command = prepare_status_wrapper(
            root.path(),
            &root.path().join("missing"),
            &json!({
                "type":"command", "command":"read -r input; printf '%s' \"$input\"",
            }),
        )
        .await
        .unwrap();
        let mut child = Command::new("/bin/bash")
            .args(["-c", &command])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"{\"status\":true}\n")
            .await
            .unwrap();
        let output = child.wait_with_output().await.unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"{\"status\":true}");
    }

    #[tokio::test]
    #[ignore = "requires installed Claude and Codex CLIs and isolated CLAUDE_CONFIG_DIR and CODEX_HOME"]
    async fn local_client_plugins_round_trip() {
        let root = tempfile::tempdir().unwrap();
        for variable in ["CLAUDE_CONFIG_DIR", "CODEX_HOME"] {
            assert!(std::env::var_os(variable)
                .map(PathBuf::from)
                .is_some_and(|path| path.starts_with(std::env::temp_dir())));
        }
        let current = claude_settings().unwrap();
        fs::create_dir_all(current.parent().unwrap()).await.unwrap();
        let original = json!({"statusLine": {"type":"command","command":"printf example"}, "includeCoAuthoredBy": false});
        write_json(&current, &original).await.unwrap();
        let monitor = Monitor::new(root.path().to_owned());
        for client in [Client::Claude, Client::Codex] {
            configure(&monitor, client, Action::Enable).await.unwrap();
            assert!(flag(&monitor.root, client).is_file());
            configure(&monitor, client, Action::Pause).await.unwrap();
            assert_eq!(
                client_status(&monitor.root, client).await.unwrap(),
                Connection::Paused
            );
            if client == Client::Claude {
                let mut installed = read_json(&current).await.unwrap().unwrap();
                installed["statusLine"]["padding"] = json!(2);
                write_json(&current, &json!([])).await.unwrap();
                assert!(configure(&monitor, client, Action::Clear).await.is_err());
                assert!(!flag(&monitor.root, client).is_file());
                assert!(cleanup_flag(&monitor.root, client).is_file());
                write_json(&current, &installed).await.unwrap();
            }
            configure(&monitor, client, Action::Clear).await.unwrap();
            assert!(!flag(&monitor.root, client).is_file());
            assert!(!cleanup_flag(&monitor.root, client).is_file());
            assert!(!marketplace_root(&monitor.root, client).is_dir());
        }
        let restored = read_json(&current).await.unwrap().unwrap();
        assert_eq!(
            restored["statusLine"]["command"],
            original["statusLine"]["command"]
        );
        assert_eq!(restored["statusLine"]["padding"], 2);
    }
}

#[cfg(test)]
mod management_tests {
    use super::*;

    #[tokio::test]
    async fn factory_reset_pauses_collection_but_preserves_the_clear_action() {
        let root = tempfile::tempdir().unwrap();
        let monitor = Monitor::new(root.path().to_owned());
        fs::create_dir_all(&monitor.root).await.unwrap();
        fs::write(flag(&monitor.root, Client::Claude), b"")
            .await
            .unwrap();
        fs::write(cleanup_flag(&monitor.root, Client::Codex), b"")
            .await
            .unwrap();

        monitor.clear_opt_in().await;
        assert_eq!(
            client_status(&monitor.root, Client::Claude).await.unwrap(),
            Connection::Paused
        );
        assert_eq!(
            client_status(&monitor.root, Client::Codex).await.unwrap(),
            Connection::CleanupPending
        );
    }

    #[tokio::test]
    async fn pausing_and_resuming_preserves_the_installed_connection() {
        let root = tempfile::tempdir().unwrap();
        let monitor = Monitor::new(root.path().to_owned());
        fs::create_dir_all(&monitor.root).await.unwrap();
        fs::write(flag(&monitor.root, Client::Claude), b"")
            .await
            .unwrap();

        configure(&monitor, Client::Claude, Action::Pause)
            .await
            .unwrap();
        assert_eq!(
            client_status(&monitor.root, Client::Claude).await.unwrap(),
            Connection::Paused
        );
        assert!(!flag(&monitor.root, Client::Claude).is_file());
        configure(&monitor, Client::Claude, Action::Enable)
            .await
            .unwrap();
        assert_eq!(
            client_status(&monitor.root, Client::Claude).await.unwrap(),
            Connection::Enabled
        );
    }
}
