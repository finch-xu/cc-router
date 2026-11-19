use super::*;
use serde_json::json;

#[test]
fn hooks_keep_metadata_without_recording_content() {
    let event = Event::from_payload(
        Client::Claude,
        &json!({
            "session_id":"session-a", "hook_event_name":"PostToolUseFailure",
            "cwd":"/repo", "tool_name":"Bash", "tool_input":{"command":"sensitive"},
            "tool_response":"sensitive", "error":"Exit code 1\nsensitive", "duration_ms":84
        }),
        15,
    )
    .unwrap();
    let output = serde_json::to_string(&event).unwrap();
    assert!(!output.contains("sensitive"));
    assert_eq!(event.activity.kind, "tool_failed");
    assert_eq!(event.activity.duration_ms, Some(84));
}

#[test]
fn contexts_and_states_are_isolated_by_client_and_session() {
    let mut store = Store::default();
    for (client, id, event, at) in [
        (Client::Claude, "same", "UserPromptSubmit", 10),
        (Client::Claude, "same", "PreCompact", 20),
        (Client::Claude, "same", "PostCompact", 25),
        (Client::Codex, "same", "Interrupt", 30),
    ] {
        store.apply(
            Event::from_payload(
                client,
                &json!({
                    "session_id": id, "turn_id":"turn", "hook_event_name":event, "trigger":"auto"
                }),
                at,
            )
            .unwrap(),
        );
    }
    let sessions = store.snapshot(31);
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].status, "interrupted");
    assert_eq!(sessions[1].status, "running");
    assert_eq!(sessions[1].activities[0].kind, "compact_finished");
    assert!(sessions[1].context.is_none());
}

#[test]
fn compaction_waits_for_a_new_context_snapshot() {
    let mut store = Store::default();
    let hook = |name, at| {
        Event::from_payload(
            Client::Claude,
            &json!({
                "session_id": "test", "hook_event_name": name, "trigger": "manual"
            }),
            at,
        )
        .unwrap()
    };
    let status = |used: u64, at| {
        Event::from_payload(Client::Claude, &json!({
        "session_id": "test", "model": {"id": "sonnet"},
        "context_window": {"context_window_size": 200, "total_input_tokens": used, "used_percentage": used / 2}
    }), at).unwrap()
    };
    store.apply(hook("UserPromptSubmit", 10));
    store.apply(status(150, 12));
    store.apply(hook("PreCompact", 20));
    store.apply(hook("PostCompact", 25));
    store.apply(status(150, 13));
    assert!(store.snapshot(26)[0].context.is_none());
    store.apply(status(40, 27));
    let session = &store.snapshot(28)[0];
    assert_eq!(session.context.as_ref().unwrap().percent, 20.0);
    assert_eq!(session.activities[0].context_before, Some(75.0));
    assert_eq!(session.activities[0].context_after, Some(20.0));
}

#[test]
fn codex_approval_is_provisional_and_retries_keep_the_turn_open() {
    let mut store = Store::default();
    let hook = |name, at, fields: Value| {
        let mut raw = json!({"session_id":"sample", "turn_id":"turn", "hook_event_name":name});
        raw.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        Event::from_payload(Client::Codex, &raw, at).unwrap()
    };
    store.apply(hook("UserPromptSubmit", 100, json!({})));
    store.apply(hook("PermissionRequest", 101, json!({"tool_name":"Bash"})));
    assert_eq!(store.snapshot(102)[0].status, "approval_requested");
    store.apply(hook(
        "PostModelRequest",
        104,
        json!({"status":"failed", "error":"private error"}),
    ));
    assert_eq!(store.snapshot(105)[0].status, "running");
    store.apply(hook("PreModelRequest", 106, json!({})));
    store.apply(hook(
        "PostModelRequest",
        108,
        json!({
            "status": "completed",
            "token_usage": {"input_tokens": 30, "output_tokens": 2, "cached_input_tokens": 15}
        }),
    ));
    store.apply(hook("Stop", 109, json!({})));
    let rows = store.snapshot(110);
    assert_eq!(rows[0].status, "idle");
    assert_eq!(rows[0].observed_usage.unwrap().requests, Some(1));
    assert_eq!(rows[0].observed_usage.unwrap().input, 30);
    assert!(!serde_json::to_string(&rows).unwrap().contains("private"));
}

#[test]
fn only_exact_local_session_keys_join_requests() {
    use crate::proxy::client_fingerprint::{ClientContext, ClientInfo, RequestEntryKind};
    let mut context = ClientContext {
        info: ClientInfo {
            tool: Some("claude-code"),
            ..Default::default()
        },
        ip: Some("::ffff:127.0.0.1".into()),
        entry_kind: RequestEntryKind::Messages,
        session_key: Some("hdr:session-a".into()),
        ..Default::default()
    };
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-claude-code-session-id", "session-a".parse().unwrap());
    assert_eq!(
        context
            .clone()
            .with_monitor_identity(&headers, &json!({}))
            .monitor_session(),
        Some((Client::Claude, "session-a".into()))
    );
    assert_eq!(
        context
            .clone()
            .with_monitor_identity(&Default::default(), &json!({}))
            .monitor_session(),
        None
    );
    context.ip = Some("192.0.2.3".into());
    assert_eq!(
        context
            .with_monitor_identity(&headers, &json!({}))
            .monitor_session(),
        None
    );
}

#[test]
fn upstream_input_respects_each_protocols_cache_counting() {
    assert_eq!(proxy_input(Some(AuthType::ApiKey), 40, 55, 5), 100);
    assert_eq!(proxy_input(Some(AuthType::ChatgptOauth), 100, 55, 5), 100);
    assert_eq!(proxy_input(Some(AuthType::GeminiApiKey), 100, 55, 0), 100);
}

#[test]
fn codex_cache_key_never_links_a_fork_to_the_parent() {
    use crate::proxy::client_fingerprint::{ClientContext, ClientInfo, RequestEntryKind};
    let context = ClientContext {
        info: ClientInfo {
            tool: Some("codex-cli"),
            ..Default::default()
        },
        ip: Some("127.0.0.1".into()),
        entry_kind: RequestEntryKind::Responses,
        ..Default::default()
    };
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("session-id", "parent".parse().unwrap());
    let body = json!({ "prompt_cache_key":"parent", "client_metadata": {
        "x-codex-turn-metadata": json!({"session_id":"fork"}).to_string()
    }});
    assert_eq!(
        context
            .clone()
            .with_monitor_identity(&headers, &body)
            .monitor_session(),
        Some((Client::Codex, "fork".into()))
    );
    assert_eq!(
        context
            .with_monitor_identity(&headers, &json!({"prompt_cache_key":"parent"}))
            .monitor_session(),
        None
    );
}

#[test]
fn subagent_events_do_not_change_the_parent_turn() {
    let mut store = Store::default();
    for (event, turn, agent, at) in [
        ("UserPromptSubmit", "parent-turn", None, 10),
        ("SubagentStart", "parent-turn", Some("child"), 20),
        ("UserPromptSubmit", "child-turn", Some("child"), 30),
        ("PermissionRequest", "child-turn", Some("child"), 32),
        ("Stop", "parent-turn", None, 40),
        ("SubagentStop", "child-turn", Some("child"), 45),
    ] {
        store.apply(Event::from_payload(Client::Codex, &json!({
            "session_id": "parent", "hook_event_name": event, "turn_id": turn, "agent_id": agent,
        }), at).unwrap());
    }
    let rows = store.snapshot(46);
    assert_eq!(rows[0].status, "idle");
    assert_eq!(rows[0].turn_started_ms, Some(10));
    assert_eq!(rows[0].turn_finished_ms, Some(40));
    assert!(!rows[0].agents[0].running);
}

#[test]
fn passive_status_updates_do_not_keep_a_lost_turn_running() {
    let mut store = Store::default();
    store.apply(
        Event::from_payload(
            Client::Claude,
            &json!({
                "session_id":"sample", "hook_event_name":"UserPromptSubmit", "prompt_id":"turn",
            }),
            10,
        )
        .unwrap(),
    );
    store.apply(
        Event::from_payload(
            Client::Claude,
            &json!({
                "session_id":"sample", "model":{"id":"sonnet"},
            }),
            STALE_MS + 20,
        )
        .unwrap(),
    );
    let rows = store.snapshot(STALE_MS + 21);
    assert_eq!(rows[0].status, "unknown");
    assert_eq!(rows[0].turn_finished_ms, Some(10));
}
