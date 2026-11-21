//! 「这个请求在干什么」—— 实时路由页的事件流用 (不落库, 只随 `live_request` 事件下发)。
//!
//! 只看 Anthropic 形态的请求体 (chat / responses 入口在入站翻译之后算), 不碰 hooks:
//! - 轮次 / 步数 / 上一步工具名: 任何客户端都能从 `messages` 推出来, 是确定值;
//! - 用途: Claude Code 的主对话会声明派子 Agent 的工具 (`Task` / `Agent`), 子 Agent 没有;
//!   不声明工具的是后台小任务 (命令安全检查、会话标题、网页摘要…), 再按提示词指纹细分。
//!   除压缩与主对话外都是推断, `guessed = true`, 界面上标「推断」。
//!
//! **隐私**: 只取工具名, 绝不保存命令、文件内容或提示词原文; 指纹匹配只在本函数内读文本。
//! **纪律**同 [`crate::proxy::tool_log`]: 拿不到值就 None / 空, 绝不 panic、绝不影响请求。

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PurposeKind {
    /// 主对话 (用户的一轮提问及其工具循环)
    Main,
    /// 子 Agent (Claude Code 的 Task / Agent 工具派出的子任务)
    Agent,
    /// 上下文压缩
    Compact,
    /// Bash 命令安全检查 (命令前缀 / 注入检测)
    Guard,
    /// 会话标题 / 话题检测
    Title,
    /// WebFetch 结果摘要
    Fetch,
    /// 连通 / 配额探测 (max_tokens = 1)
    Probe,
    /// 其他不带工具的后台小任务
    Background,
    /// 无法判断 (非 Claude Code / Codex 客户端)
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RequestPurpose {
    pub kind: PurposeKind,
    /// true = 靠指纹推断, 界面标「推断」
    pub guessed: bool,
    /// 第几轮: 不含 tool_result 的 user 消息数
    pub turn: Option<u32>,
    /// 本轮第几步: 最后一条用户提问之后的 assistant 消息数 + 1
    pub step: Option<u32>,
    /// 本请求回传的是哪些工具的结果 (上一条 assistant 消息里的 tool_use 名, 保序, 最多 8 个)
    pub prev_tools: Vec<String>,
}

const MAX_PREV_TOOLS: usize = 8;
const MAX_TOOL_NAME: usize = 64;
/// 指纹匹配最多读这么多字节的提示词, 防超大 system 拖慢请求
const SCAN_LIMIT: usize = 32 * 1024;

const COMPACT_MARKERS: &[&str] = &["create a detailed summary of the conversation"];
const GUARD_MARKERS: &[&str] = &["command prefix", "<policy_spec>", "command injection"];
const TITLE_MARKERS: &[&str] = &["new conversation topic", "conversation title", "session title", "title for"];
const FETCH_MARKERS: &[&str] = &["web page content", "web content:"];

fn is_claude_code(tool: Option<&str>) -> bool {
    tool == Some("claude-code")
}

fn is_codex(tool: Option<&str>) -> bool {
    matches!(tool, Some("codex-cli") | Some("codex-desktop"))
}

fn blocks(content: &Value) -> &[Value] {
    content.as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn has_tool_result(msg: &Value) -> bool {
    blocks(&msg["content"]).iter().any(|b| b["type"] == "tool_result")
}

fn is_role(msg: &Value, role: &str) -> bool {
    msg.get("role").and_then(Value::as_str) == Some(role)
}

/// 把 string / 文本块数组拼成小写文本, 截到 [`SCAN_LIMIT`]
fn lower_text(v: &Value) -> String {
    let mut out = String::new();
    let mut push = |s: &str| {
        if out.len() < SCAN_LIMIT {
            let room = SCAN_LIMIT - out.len();
            let end = s.char_indices().map(|(i, _)| i).take_while(|i| *i <= room).last().unwrap_or(0);
            out.push_str(if s.len() <= room { s } else { &s[..end] });
            out.push('\n');
        }
    };
    match v {
        Value::String(s) => push(s),
        Value::Array(items) => {
            for b in items {
                if let Some(s) = b.get("text").and_then(Value::as_str) {
                    push(s);
                }
            }
        }
        _ => {}
    }
    out.to_lowercase()
}

fn contains_any(hay: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| hay.contains(n))
}

pub fn classify(body: &Value, client_tool: Option<&str>) -> RequestPurpose {
    let msgs: &[Value] = body.get("messages").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);

    // 轮次 / 步数
    let human = |m: &Value| is_role(m, "user") && !has_tool_result(m);
    let turn = (!msgs.is_empty()).then(|| msgs.iter().filter(|m| human(m)).count() as u32);
    let step = msgs.iter().rposition(human).map(|i| {
        msgs[i + 1..].iter().filter(|m| is_role(m, "assistant")).count() as u32 + 1
    });

    // 上一步工具: 最后一条是带 tool_result 的 user 消息时, 取它前面那条 assistant 的 tool_use 名
    let mut prev_tools = Vec::new();
    if let [.., prev, last] = msgs {
        if is_role(last, "user") && has_tool_result(last) && is_role(prev, "assistant") {
            prev_tools = blocks(&prev["content"])
                .iter()
                .filter(|b| matches!(b["type"].as_str(), Some("tool_use") | Some("server_tool_use")))
                .filter_map(|b| b["name"].as_str())
                .take(MAX_PREV_TOOLS)
                .map(|n| n.chars().take(MAX_TOOL_NAME).collect())
                .collect();
        }
    }

    let tool_names: Vec<&str> = body
        .get("tools")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|t| t["name"].as_str()).collect())
        .unwrap_or_default();
    let last_user_text = msgs
        .iter()
        .rev()
        .find(|m| is_role(m, "user"))
        .map(|m| lower_text(&m["content"]))
        .unwrap_or_default();

    let (kind, guessed) = if contains_any(&last_user_text, COMPACT_MARKERS) {
        (PurposeKind::Compact, false)
    } else if is_claude_code(client_tool) {
        if body.get("max_tokens").and_then(Value::as_u64) == Some(1) {
            (PurposeKind::Probe, true)
        } else if tool_names.is_empty() {
            let text = format!("{}{}", lower_text(body.get("system").unwrap_or(&Value::Null)), last_user_text);
            let kind = if contains_any(&text, GUARD_MARKERS) {
                PurposeKind::Guard
            } else if contains_any(&text, TITLE_MARKERS) {
                PurposeKind::Title
            } else if contains_any(&text, FETCH_MARKERS) {
                PurposeKind::Fetch
            } else {
                PurposeKind::Background
            };
            (kind, true)
        } else if tool_names.iter().any(|n| matches!(*n, "Task" | "Agent")) {
            (PurposeKind::Main, false)
        } else {
            (PurposeKind::Agent, true)
        }
    } else if is_codex(client_tool) {
        if tool_names.is_empty() { (PurposeKind::Background, true) } else { (PurposeKind::Main, false) }
    } else {
        (PurposeKind::Unknown, false)
    };

    RequestPurpose { kind, guessed, turn, step, prev_tools }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cc(body: Value) -> RequestPurpose {
        classify(&body, Some("claude-code"))
    }

    #[test]
    fn main_turn_counts_turns_steps_and_previous_tools() {
        let p = cc(json!({
            "tools": [{"name": "Task"}, {"name": "Bash"}],
            "messages": [
                {"role": "user", "content": "first"},
                {"role": "assistant", "content": "ok"},
                {"role": "user", "content": [{"type": "text", "text": "second"}]},
                {"role": "assistant", "content": [{"type": "tool_use", "id": "a", "name": "Read", "input": {"file_path": "/secret"}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "a", "content": "x"}]},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "b", "name": "Bash", "input": {"command": "rm -rf"}},
                    {"type": "tool_use", "id": "c", "name": "Edit", "input": {}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "b", "content": "x"},
                    {"type": "tool_result", "tool_use_id": "c", "content": "y"},
                    {"type": "text", "text": "<system-reminder>"}
                ]}
            ]
        }));
        assert_eq!(p.kind, PurposeKind::Main);
        assert!(!p.guessed);
        assert_eq!(p.turn, Some(2));
        assert_eq!(p.step, Some(3));
        assert_eq!(p.prev_tools, vec!["Bash", "Edit"]);
    }

    #[test]
    fn new_question_is_step_one_without_previous_tools() {
        let p = cc(json!({"tools": [{"name": "Agent"}], "messages": [{"role": "user", "content": "hi"}]}));
        assert_eq!((p.turn, p.step), (Some(1), Some(1)));
        assert!(p.prev_tools.is_empty());
    }

    #[test]
    fn claude_code_without_spawn_tool_is_a_guessed_subagent() {
        let p = cc(json!({"tools": [{"name": "Grep"}], "messages": [{"role": "user", "content": "find"}]}));
        assert_eq!(p.kind, PurposeKind::Agent);
        assert!(p.guessed);
    }

    #[test]
    fn toolless_claude_code_requests_are_split_by_fingerprint() {
        let guard = cc(json!({"system": [{"type": "text", "text": "<policy_spec># Bash command prefix detection"}], "messages": [{"role": "user", "content": "git push"}]}));
        assert_eq!(guard.kind, PurposeKind::Guard);
        let title = cc(json!({"system": "Analyze if this message indicates a new conversation topic.", "messages": [{"role": "user", "content": "x"}]}));
        assert_eq!(title.kind, PurposeKind::Title);
        let fetch = cc(json!({"messages": [{"role": "user", "content": "Web page content:\n---\n..."}]}));
        assert_eq!(fetch.kind, PurposeKind::Fetch);
        let other = cc(json!({"messages": [{"role": "user", "content": "hello"}]}));
        assert_eq!(other.kind, PurposeKind::Background);
        assert!(other.guessed);
        let probe = cc(json!({"max_tokens": 1, "tools": [{"name": "Task"}], "messages": [{"role": "user", "content": "quota"}]}));
        assert_eq!(probe.kind, PurposeKind::Probe);
    }

    #[test]
    fn compaction_is_recognised_for_any_client() {
        let body = json!({"tools": [{"name": "Task"}], "messages": [{"role": "user", "content": [{"type": "text", "text": "Your task is to create a detailed summary of the conversation so far"}]}]});
        assert_eq!(cc(body.clone()).kind, PurposeKind::Compact);
        assert_eq!(classify(&body, None).kind, PurposeKind::Compact);
    }

    #[test]
    fn codex_and_unknown_clients() {
        let body = json!({"tools": [{"name": "shell"}], "messages": [{"role": "user", "content": "x"}]});
        assert_eq!(classify(&body, Some("codex-cli")).kind, PurposeKind::Main);
        let other = classify(&body, Some("opencode"));
        assert_eq!(other.kind, PurposeKind::Unknown);
        assert_eq!(other.turn, Some(1));
    }

    #[test]
    fn garbage_bodies_never_panic() {
        for body in [json!(null), json!({"messages": "x"}), json!({"messages": [1, null, {"role": 3}]}), json!({"system": 5, "tools": "x"})] {
            let p = cc(body);
            assert!(p.prev_tools.is_empty());
        }
        // 超长多字节提示词: 截断不能切在字符中间
        let long = "检".repeat(SCAN_LIMIT);
        let _ = cc(json!({"system": long, "messages": [{"role": "user", "content": "x"}]}));
    }
}
