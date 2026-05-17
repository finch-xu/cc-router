//! 从请求 headers 识别客户端工具 (Claude Code / Claude Desktop / Codex CLI / Zed / Cursor /
//! OpenCode / Anthropic SDK / cc-router 转发 / 未识别).
//!
//! 设计:
//! - 纯函数, 无 IO 无 async, 便于单测
//! - 按优先级顺序匹配, 强信号 (`x-app`, `x-stainless-lang`) 优先于 UA substring
//! - 未识别时 `tool=None`, 仍保留原始 UA 供前端展示和日后规则升级
//!
//! 待实测验证: 当前规则基于公开资料和推测, 必须在本地起服务用真实工具发请求,
//! 把抓到的 UA 沉淀进单测 fixture 防回归.
//!
//! `SUPPORTED_TOOLS` 中的取值必须和前端 `src/types.ts::ClientToolId` union 手工同步.

use axum::http::HeaderMap;

/// 原始 UA 落库前的安全上限. 与 `RequestLogEntry.upstream_response_body` 的截断逻辑同思路:
/// 防恶意/异常客户端发超大 UA 让 SQLite requests 表短期膨胀.
/// 512B 足够覆盖正常 SDK + CLI 的合成 UA 格式.
const UA_MAX_BYTES: usize = 512;

fn truncate_ua(mut s: String) -> String {
    if s.len() > UA_MAX_BYTES {
        // 按 UTF-8 字符边界安全截断
        let mut cut = UA_MAX_BYTES;
        while !s.is_char_boundary(cut) && cut > 0 {
            cut -= 1;
        }
        s.truncate(cut);
    }
    s
}

/// 所有可识别的客户端工具短标签 (kebab-case).
/// 顺序无意义, 前端筛选器下拉直接消费这个列表 (i18n 文案在前端 CLIENT_TOOLS 表里维护).
pub const SUPPORTED_TOOLS: &[&str] = &[
    "claude-code",
    "claude-desktop",
    "codex-cli",
    "cc-router",
    "zed",
    "cursor",
    "opencode",
    "anthropic-sdk-python",
    "anthropic-sdk-js",
];

/// 识别结果. `tool=None` 表示未识别 (前端展示 "unk").
/// `user_agent` 总是保留原始 UA (即便识别成功), 用于详情抽屉展示.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientInfo {
    pub tool: Option<&'static str>,
    pub user_agent: Option<String>,
    pub version: Option<String>,
}

/// 整套客户端上下文 (识别结果 + 对端 IP). 在 handler 入口构造一次,
/// 沿 pipeline → dispatch 链克隆给每个 RequestLogEntry 构造点.
/// 用 String 存 IP 避免下游模块到处 import std::net::IpAddr.
#[derive(Debug, Clone, Default)]
pub struct ClientContext {
    pub info: ClientInfo,
    pub ip: Option<String>,
}

/// 入口: 按规则识别 headers, 返回 ClientInfo.
pub fn identify(headers: &HeaderMap) -> ClientInfo {
    let user_agent = header_str(headers, "user-agent")
        .map(String::from)
        .map(truncate_ua);
    let x_app = header_str(headers, "x-app").map(|s| s.to_ascii_lowercase());
    let x_stainless_lang = header_str(headers, "x-stainless-lang").map(|s| s.to_ascii_lowercase());
    let x_stainless_pkg_ver = header_str(headers, "x-stainless-package-version").map(String::from);

    let (tool, version) = classify(
        user_agent.as_deref(),
        x_app.as_deref(),
        x_stainless_lang.as_deref(),
        x_stainless_pkg_ver.as_deref(),
    );

    ClientInfo {
        tool,
        user_agent,
        version,
    }
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// 把 UA / x-app / x-stainless-* 分类成 (tool, version).
/// 抽成纯函数方便单测 (无须构造 HeaderMap).
fn classify(
    ua: Option<&str>,
    x_app: Option<&str>,
    x_stainless_lang: Option<&str>,
    x_stainless_pkg_ver: Option<&str>,
) -> (Option<&'static str>, Option<String>) {
    let ua_lower = ua.map(|s| s.to_ascii_lowercase());
    let ua_lower_ref = ua_lower.as_deref();

    // 1. x-app: cli → Claude Code (强信号, Anthropic Claude Code CLI 私有 header)
    if matches!(x_app, Some("cli")) {
        let ver = ua
            .and_then(|u| extract_version_after(u, "claude-cli/"))
            .or_else(|| ua.and_then(|u| extract_version_after(u, "claude-code/")));
        return (Some("claude-code"), ver);
    }

    // 2. UA 含 claude-cli/ → Claude Code (兜底, x-app 缺失场景)
    if ua_lower_ref.is_some_and(|u| u.contains("claude-cli/")) {
        let ver = ua.and_then(|u| extract_version_after(u, "claude-cli/"));
        return (Some("claude-code"), ver);
    }
    if ua_lower_ref.is_some_and(|u| u.contains("claude-code/")) {
        let ver = ua.and_then(|u| extract_version_after(u, "claude-code/"));
        return (Some("claude-code"), ver);
    }

    // 3. cc-router 转发 (优先于 codex_cli_rs, 因为 cc-router 自己的 codex 上游 UA 同时含两者)
    if ua_lower_ref.is_some_and(|u| u.contains("cc-router/")) {
        let ver = ua.and_then(|u| extract_version_after(u, "cc-router/"));
        return (Some("cc-router"), ver);
    }

    // 4. UA 含 codex_cli_rs/ → Codex CLI (OpenAI 官方 Rust CLI)
    if ua_lower_ref.is_some_and(|u| u.contains("codex_cli_rs/")) {
        let ver = ua.and_then(|u| extract_version_after(u, "codex_cli_rs/"));
        return (Some("codex-cli"), ver);
    }

    // 5. Zed 编辑器
    if ua_lower_ref.is_some_and(|u| u.contains("zed/") || u.starts_with("zed ")) {
        let ver = ua.and_then(|u| {
            extract_version_after(u, "Zed/").or_else(|| extract_version_after(u, "zed/"))
        });
        return (Some("zed"), ver);
    }

    // 6. OpenCode
    if ua_lower_ref.is_some_and(|u| u.contains("opencode/")) {
        let ver = ua.and_then(|u| {
            extract_version_after(u, "OpenCode/").or_else(|| extract_version_after(u, "opencode/"))
        });
        return (Some("opencode"), ver);
    }

    // 7. Cursor 编辑器
    if ua_lower_ref.is_some_and(|u| u.contains("cursor/")) {
        let ver = ua.and_then(|u| {
            extract_version_after(u, "Cursor/").or_else(|| extract_version_after(u, "cursor/"))
        });
        return (Some("cursor"), ver);
    }

    // 8. Claude Desktop (待实测确认; Desktop 当前未必走第三方 endpoint)
    if ua_lower_ref.is_some_and(|u| u.contains("claude-desktop/") || u.contains("claude/")) {
        let ver = ua.and_then(|u| {
            extract_version_after(u, "Claude-Desktop/")
                .or_else(|| extract_version_after(u, "claude-desktop/"))
                .or_else(|| extract_version_after(u, "Claude/"))
        });
        return (Some("claude-desktop"), ver);
    }

    // 9. Anthropic SDK (按 stainless 语言判定)
    if matches!(x_stainless_lang, Some("python")) {
        return (
            Some("anthropic-sdk-python"),
            x_stainless_pkg_ver.map(String::from),
        );
    }
    if matches!(x_stainless_lang, Some("js"))
        || ua_lower_ref.is_some_and(|u| u.contains("@anthropic-ai/sdk"))
    {
        let ver = x_stainless_pkg_ver
            .map(String::from)
            .or_else(|| ua.and_then(|u| extract_version_after(u, "@anthropic-ai/sdk/")));
        return (Some("anthropic-sdk-js"), ver);
    }

    (None, None)
}

/// 从 UA 字符串里找 `marker` 后紧跟的版本号 token (到第一个空格/分号/括号/逗号/制表符前为止).
/// 返回值仅在看起来像版本号 (首字符是数字) 时才有效, 否则 None.
fn extract_version_after(ua: &str, marker: &str) -> Option<String> {
    let idx = ua.find(marker)?;
    let rest = &ua[idx + marker.len()..];
    let end = rest
        .find(|c: char| c.is_whitespace() || matches!(c, ';' | '(' | ')' | ',' | '\t'))
        .unwrap_or(rest.len());
    let token = rest[..end].trim_end_matches('/');
    if token.is_empty() || !token.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        None
    } else {
        Some(token.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    fn h(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        m
    }

    #[test]
    fn claude_code_via_x_app_header() {
        let info = identify(&h(&[
            ("user-agent", "claude-cli/1.0.45 (external, cli)"),
            ("x-app", "cli"),
        ]));
        assert_eq!(info.tool, Some("claude-code"));
        assert_eq!(info.version.as_deref(), Some("1.0.45"));
    }

    #[test]
    fn claude_code_via_ua_only() {
        let info = identify(&h(&[("user-agent", "claude-cli/2.3.1 (some other suffix)")]));
        assert_eq!(info.tool, Some("claude-code"));
        assert_eq!(info.version.as_deref(), Some("2.3.1"));
    }

    #[test]
    fn cc_router_takes_precedence_over_codex_cli_rs() {
        // cc-router 转发自己的 codex provider 出去时, UA 是 "codex_cli_rs/x (...) cc-router/y"
        // 该请求若被另一个 cc-router 接收, 应归为 cc-router (转发) 而不是 codex-cli
        let info = identify(&h(&[(
            "user-agent",
            "codex_cli_rs/0.20.0 (Macos; arm64) cc-router/2.5.0",
        )]));
        assert_eq!(info.tool, Some("cc-router"));
        assert_eq!(info.version.as_deref(), Some("2.5.0"));
    }

    #[test]
    fn codex_cli_pure() {
        let info = identify(&h(&[("user-agent", "codex_cli_rs/0.20.0 (Macos; arm64)")]));
        assert_eq!(info.tool, Some("codex-cli"));
        assert_eq!(info.version.as_deref(), Some("0.20.0"));
    }

    #[test]
    fn zed_editor() {
        let info = identify(&h(&[("user-agent", "Zed/0.158.2 (macos arm64)")]));
        assert_eq!(info.tool, Some("zed"));
        assert_eq!(info.version.as_deref(), Some("0.158.2"));
    }

    #[test]
    fn opencode() {
        let info = identify(&h(&[("user-agent", "opencode/0.4.0")]));
        assert_eq!(info.tool, Some("opencode"));
        assert_eq!(info.version.as_deref(), Some("0.4.0"));
    }

    #[test]
    fn cursor() {
        let info = identify(&h(&[("user-agent", "Cursor/0.42.5 (darwin x64)")]));
        assert_eq!(info.tool, Some("cursor"));
        assert_eq!(info.version.as_deref(), Some("0.42.5"));
    }

    #[test]
    fn anthropic_python_sdk_via_stainless() {
        let info = identify(&h(&[
            ("user-agent", "Anthropic/Python 0.40.0"),
            ("x-stainless-lang", "python"),
            ("x-stainless-package-version", "0.40.0"),
        ]));
        assert_eq!(info.tool, Some("anthropic-sdk-python"));
        assert_eq!(info.version.as_deref(), Some("0.40.0"));
    }

    #[test]
    fn anthropic_js_sdk_via_stainless() {
        let info = identify(&h(&[
            ("user-agent", "@anthropic-ai/sdk/0.32.1 (Node.js v20)"),
            ("x-stainless-lang", "js"),
            ("x-stainless-package-version", "0.32.1"),
        ]));
        assert_eq!(info.tool, Some("anthropic-sdk-js"));
        assert_eq!(info.version.as_deref(), Some("0.32.1"));
    }

    #[test]
    fn anthropic_js_sdk_via_ua_only() {
        // 老版本 SDK 可能没有 stainless headers, 只能靠 UA 兜底
        let info = identify(&h(&[(
            "user-agent",
            "@anthropic-ai/sdk/0.20.0 (Browser)",
        )]));
        assert_eq!(info.tool, Some("anthropic-sdk-js"));
        assert_eq!(info.version.as_deref(), Some("0.20.0"));
    }

    #[test]
    fn unknown_random_curl() {
        let info = identify(&h(&[("user-agent", "curl/7.81.0")]));
        assert_eq!(info.tool, None);
        assert_eq!(info.user_agent.as_deref(), Some("curl/7.81.0"));
        assert_eq!(info.version, None);
    }

    #[test]
    fn unknown_empty_headers() {
        let info = identify(&HeaderMap::new());
        assert_eq!(info.tool, None);
        assert_eq!(info.user_agent, None);
        assert_eq!(info.version, None);
    }

    #[test]
    fn user_agent_always_preserved_even_when_identified() {
        let info = identify(&h(&[
            ("user-agent", "claude-cli/1.0.45 (external, cli)"),
            ("x-app", "cli"),
        ]));
        assert_eq!(
            info.user_agent.as_deref(),
            Some("claude-cli/1.0.45 (external, cli)")
        );
    }

    #[test]
    fn extract_version_basic() {
        assert_eq!(
            extract_version_after("claude-cli/1.2.3 (x)", "claude-cli/"),
            Some("1.2.3".to_string())
        );
        assert_eq!(
            extract_version_after("claude-cli/1.2.3-beta;ok", "claude-cli/"),
            Some("1.2.3-beta".to_string())
        );
        assert_eq!(
            extract_version_after("foo/no-version (x)", "foo/"),
            None,
            "首字符非数字应返回 None"
        );
        assert_eq!(extract_version_after("nothing here", "claude-cli/"), None);
    }

    #[test]
    fn user_agent_truncated_to_safe_max() {
        let huge = "claude-cli/1.0.0 ".to_string() + &"x".repeat(10_000);
        let info = identify(&h(&[("user-agent", huge.as_str()), ("x-app", "cli")]));
        // 识别仍然成功 (前缀完整保留)
        assert_eq!(info.tool, Some("claude-code"));
        assert_eq!(info.version.as_deref(), Some("1.0.0"));
        // UA 已被截断到上限
        assert!(info.user_agent.as_deref().is_some_and(|u| u.len() <= 512));
    }

    #[test]
    fn supported_tools_consistent_with_classify() {
        // 防呆: 若新增 classify 返回值忘记加进 SUPPORTED_TOOLS, 这条不会失败 (静态保证不了),
        // 但可以反向校验 SUPPORTED_TOOLS 里每个值至少有一个 fixture 能命中——靠人工或独立 lint 工具.
        // 这里仅断言常量数量, 改动时 force review.
        assert_eq!(SUPPORTED_TOOLS.len(), 9);
    }
}
