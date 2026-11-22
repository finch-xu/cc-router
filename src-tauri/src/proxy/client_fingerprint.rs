//! 从入站请求头识别客户端工具、SDK、HTTP 库和浏览器。
//!
//! 设计:
//! - 纯函数, 无 IO 无 async, 便于单测
//! - 专用客户端优先，其次是有品牌标记的 SDK、HTTP 库或运行时、脚本声明，最后是浏览器
//! - 未识别时 `tool=None`, 仍保留原始 UA 供前端展示和日后规则升级
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
    "codex-desktop",
    "cc-router",
    "zed",
    "cursor",
    "opencode",
    "anthropic-sdk-python",
    "anthropic-sdk-js",
    "openai-sdk-python",
    "openai-sdk-js",
    "python-requests",
    "python-httpx",
    "axios",
    "node-js",
    "curl",
    "browser",
    "go-net-http",
    "go-resty",
    "rust-ureq",
    "rust-reqwest",
    "rust-hyper",
    "python-urllib",
    "python-aiohttp",
    "bun",
    "deno",
    "java-http-client",
    "okhttp",
    "apache-http-client",
    "powershell",
    "guzzle",
    "faraday",
    "httpie",
    "wget",
    "script-go",
    "script-rust",
    "script-python",
    "script-javascript",
    "script-typescript",
    "script-java",
    "script-dotnet",
    "script-php",
    "script-ruby",
];

/// 成对登记分隔符 `/` 前的完整产品名和用于日志的客户端标签。
const HTTP_PRODUCTS: &[(&str, &str)] = &[
    ("python-requests", "python-requests"),
    ("python-httpx", "python-httpx"),
    ("python-urllib", "python-urllib"),
    ("aiohttp", "python-aiohttp"),
    ("axios", "axios"),
    ("bun", "bun"),
    ("deno", "deno"),
    ("go-http-client", "go-net-http"),
    ("go-resty", "go-resty"),
    ("ureq", "rust-ureq"),
    ("reqwest", "rust-reqwest"),
    ("hyper", "rust-hyper"),
    ("java-http-client", "java-http-client"),
    ("okhttp", "okhttp"),
    ("apache-httpclient", "apache-http-client"),
    ("powershell", "powershell"),
    ("windowspowershell", "powershell"),
    ("guzzlehttp", "guzzle"),
    ("httpie", "httpie"),
    ("wget", "wget"),
    ("curl", "curl"),
];

/// 客户端主动声明的语言。值只用于本地观测；转发策略已拒绝 `x-ccr-*`。
const SCRIPT_LANGUAGES: &[(&str, &str)] = &[
    ("go", "script-go"),
    ("rust", "script-rust"),
    ("python", "script-python"),
    ("javascript", "script-javascript"),
    ("typescript", "script-typescript"),
    ("java", "script-java"),
    ("dotnet", "script-dotnet"),
    ("php", "script-php"),
    ("ruby", "script-ruby"),
];

/// 识别结果. `tool=None` 表示未识别 (前端展示 "unk").
/// `user_agent` 总是保留原始 UA (即便识别成功), 用于详情抽屉展示.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientInfo {
    pub tool: Option<&'static str>,
    pub user_agent: Option<String>,
    pub version: Option<String>,
}

/// cc-router 对外暴露的四个入口端点. 用枚举不是字符串, 避免下游构造 entry 时拼写错.
/// 默认 Messages: 99% 流量是 /v1/messages, ClientContext::default() 在测试/fallback 路径上有用.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RequestEntryKind {
    #[default]
    Messages,
    Responses,
    /// POST /v1/chat/completions (OpenAI Chat Completions 兼容入口, v4.9+).
    ChatCompletions,
    /// POST /v1/systemone (Jev / System One 决策协议, 原样透传).
    SystemOne,
}

impl RequestEntryKind {
    /// 落 DB `requests.entry_kind` 的字符串; 前端按 `/v1/{entry_kind}` 渲染, 因此 Chat 带斜杠.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Messages => "messages",
            Self::Responses => "responses",
            Self::ChatCompletions => "chat/completions",
            Self::SystemOne => "systemone",
        }
    }
}

/// 整套客户端上下文 (识别结果 + 对端 IP + 入口端点 + 下游 HTTP 协议版本).
/// 在 handler 入口构造一次, 沿 pipeline → dispatch 链克隆给每个 RequestLogEntry 构造点.
/// 用 String 存 IP / http_version 避免下游模块到处 import std::net::IpAddr / axum::http::Version.
#[derive(Debug, Clone, Default)]
pub struct ClientContext {
    pub info: ClientInfo,
    pub ip: Option<String>,
    /// 请求进入的入口: /v1/messages、/v1/responses 或 /v1/chat/completions.
    pub entry_kind: RequestEntryKind,
    /// 下游 (CC ↔ cc-router) 协商的 HTTP 协议版本字符串, 形如 "HTTP/1.1" / "HTTP/2.0".
    /// 由 [`super::extractors::format_http_version`] 生成, 落 DB requests.downstream_http_version.
    pub http_version: Option<String>,
    /// 会话亲和键 (见 proxy::session_key). None = 本请求不参与 sticky.
    pub session_key: Option<String>,
    // Runtime telemetry identity has stricter rules than sticky routing.
    pub monitor_identity: Option<(crate::session_monitor::Client, String)>,
    /// 一次入站请求一个, 所有 attempt 共用 (requests.trace_id), 用于把失败切换的尝试归组。
    pub trace_id: Option<String>,
    /// 请求侧工具形状 (声明工具数 / 回传 tool_result 数), 对 Anthropic 形态 body 算一次。
    pub tools: crate::proxy::tool_log::RequestToolShape,
    /// 入站请求头, 已脱敏序列化 (proxy/header_log.rs); 关闭「记录请求头」时为 None。
    pub request_headers_log: Option<String>,
    /// 本请求是否记录请求头 (settings.log_request_headers 在入口处读一次)。
    pub log_headers: bool,
    /// 当前 attempt 发往上游的请求头 / 上游返回的响应头 (已脱敏序列化)。pipeline 每次 attempt
    /// 发送后用 [`ClientContext::with_upstream_headers`] 生成本 attempt 的 ctx; 入口处恒为 None。
    pub upstream_request_headers_log: Option<String>,
    pub upstream_response_headers_log: Option<String>,
    /// 当前 attempt 的网络耗时 (proxy/net_timing.rs), 随 [`ClientContext::with_upstream_headers`]
    /// 一起生成; 入口处 / 未发送时为 None。
    pub net_timing: Option<crate::proxy::net_timing::NetTimingFields>,
    /// 本次请求命中的规则插件记录 (JSON 数组, 见 `plugins/rules.rs`); 未命中或无规则插件为 None。
    pub plugins_applied: Option<String>,
}

impl ClientContext {
    pub(crate) fn monitor_session(&self) -> Option<(crate::session_monitor::Client, String)> {
        self.monitor_identity.clone()
    }

    pub(crate) fn with_monitor_identity(mut self, headers: &HeaderMap, body: &serde_json::Value) -> Self {
        self.monitor_identity = self.extract_monitor_identity(headers, body);
        self
    }

    fn extract_monitor_identity(&self, headers: &HeaderMap, body: &serde_json::Value) -> Option<(crate::session_monitor::Client, String)> {
        use crate::session_monitor::{session_id, Client};
        let peer = self.ip.as_deref()?.parse::<std::net::IpAddr>().ok()?;
        if !peer.to_canonical().is_loopback() {
            return None;
        }
        let header = |name| headers.get(name).and_then(|value| value.to_str().ok());
        let (client, id) = match (self.info.tool, self.entry_kind) {
            (Some("claude-code"), RequestEntryKind::Messages) => (Client::Claude, header("x-claude-code-session-id")?.to_owned()),
            (Some("codex-cli"), RequestEntryKind::Responses) => {
                let metadata = body["client_metadata"]["x-codex-turn-metadata"].as_str()
                    .or_else(|| header("x-codex-turn-metadata"));
                // Forks and agents may reuse prompt_cache_key and session-id for routing.
                // Prefer the event's session identity. Without it, only join an explicit
                // thread ID or the older underscore header, never a cache key.
                let explicit = body["client_metadata"]["session_id"].as_str();
                let id = match (explicit, metadata) {
                    (Some(id), _) => id.to_owned(),

                    (None, Some(raw)) if raw.len() <= 8192 => {
                        let value: serde_json::Value = serde_json::from_str(raw).ok()?;
                        value["session_id"].as_str()?.to_owned()
                    }
                    (None, Some(_)) => return None,
                    (None, None) => header("thread-id").or_else(|| header("session_id"))?.to_owned(),
                };
                (Client::Codex, id)
            }
            _ => return None,
        };
        Some((client, session_id(&id)?.to_owned()))
    }

    /// 本 attempt 的 ctx: 带上刚采集到的上游两组头与网络耗时, 其余字段不变。
    /// `attempt_start` 是本 attempt 计时起点 (与 total_latency_ms 同一个), 网络各段相对它算。
    pub fn with_upstream_headers(
        &self,
        cap: crate::proxy::header_log::UpstreamHeaderCapture,
        hints: &crate::proxy::header_log::SourceHints,
        client_headers: &axum::http::HeaderMap,
        attempt_start: std::time::Instant,
    ) -> Self {
        Self {
            net_timing: cap.net.as_ref().map(|t| t.fields(attempt_start)),
            upstream_request_headers_log: cap
                .request
                .and_then(|c| crate::proxy::header_log::tag_capture(c, hints, client_headers)),
            upstream_response_headers_log: cap.response,
            ..self.clone()
        }
    }

    /// 请求日志行的网络耗时列; 本 attempt 没有发出请求时全为 None。
    pub fn net_log(&self) -> crate::proxy::net_timing::NetTimingFields {
        self.net_timing.clone().unwrap_or_else(crate::proxy::net_timing::NetTimingFields::none)
    }

    /// 请求日志行的三列请求头。
    pub fn header_log(&self) -> crate::proxy::header_log::HeaderLogFields {
        crate::proxy::header_log::HeaderLogFields {
            client_request: self.request_headers_log.clone(),
            upstream_request: self.upstream_request_headers_log.clone(),
            upstream_response: self.upstream_response_headers_log.clone(),
        }
    }
}

/// 入口: 按规则识别 headers, 返回 ClientInfo.
pub fn identify(headers: &HeaderMap) -> ClientInfo {
    let user_agent = header_str(headers, "user-agent")
        .map(String::from)
        .map(truncate_ua);
    let (tool, version) = classify(user_agent.as_deref(), headers);

    ClientInfo {
        tool,
        user_agent,
        version,
    }
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// 按入站请求头返回 (tool, version).
fn classify(ua: Option<&str>, headers: &HeaderMap) -> (Option<&'static str>, Option<String>) {
    let ua_lower = ua.unwrap_or_default().to_ascii_lowercase();

    // 1. x-app: cli → Claude Code (强信号, Anthropic Claude Code CLI 私有 header)
    if header_str(headers, "x-app").is_some_and(|app| app.eq_ignore_ascii_case("cli")) {
        let ver = ua
            .and_then(|u| extract_version_after(u, "claude-cli/"))
            .or_else(|| ua.and_then(|u| extract_version_after(u, "claude-code/")));
        return (Some("claude-code"), ver);
    }

    // 2. UA 含 claude-cli/ → Claude Code (兜底, x-app 缺失场景)
    if ua_lower.contains("claude-cli/") {
        let ver = ua.and_then(|u| extract_version_after(u, "claude-cli/"));
        return (Some("claude-code"), ver);
    }
    if ua_lower.contains("claude-code/") {
        let ver = ua.and_then(|u| extract_version_after(u, "claude-code/"));
        return (Some("claude-code"), ver);
    }

    // 3. cc-router 转发 (优先于 codex_cli_rs, 因为 cc-router 自己的 codex 上游 UA 同时含两者)
    if ua_lower.contains("cc-router/") {
        let ver = ua.and_then(|u| extract_version_after(u, "cc-router/"));
        return (Some("cc-router"), ver);
    }

    // 4. UA 含 codex_cli_rs/ → Codex CLI (OpenAI 官方 Rust CLI 旧 UA)
    if ua_lower.contains("codex_cli_rs/") {
        let ver = ua.and_then(|u| extract_version_after(u, "codex_cli_rs/"));
        return (Some("codex-cli"), ver);
    }

    // 4b. UA 含 codex-tui/ → Codex CLI 新 frontend (ghostty 引擎, 与 codex_cli_rs 同源)
    if ua_lower.contains("codex-tui/") {
        let ver = ua.and_then(|u| extract_version_after(u, "codex-tui/"));
        return (Some("codex-cli"), ver);
    }

    // 4c. UA 含 "codex desktop/" → Codex Desktop 桌面客户端 (独立标识便于统计区分)
    if ua_lower.contains("codex desktop/") {
        let ver = ua.and_then(|u| {
            extract_version_after(u, "Codex Desktop/")
                .or_else(|| extract_version_after(u, "codex desktop/"))
        });
        return (Some("codex-desktop"), ver);
    }

    // 5. Zed 编辑器
    if ua_lower.contains("zed/") || ua_lower.starts_with("zed ") {
        let ver = ua.and_then(|u| {
            extract_version_after(u, "Zed/").or_else(|| extract_version_after(u, "zed/"))
        });
        return (Some("zed"), ver);
    }

    // 6. OpenCode
    if ua_lower.contains("opencode/") {
        let ver = ua.and_then(|u| {
            extract_version_after(u, "OpenCode/").or_else(|| extract_version_after(u, "opencode/"))
        });
        return (Some("opencode"), ver);
    }

    // 7. Cursor 编辑器
    if ua_lower.contains("cursor/") {
        let ver = ua.and_then(|u| {
            extract_version_after(u, "Cursor/").or_else(|| extract_version_after(u, "cursor/"))
        });
        return (Some("cursor"), ver);
    }

    // 8. Claude Desktop (待实测确认; Desktop 当前未必走第三方 endpoint)
    if ua_lower.contains("claude-desktop/") || ua_lower.contains("claude/") {
        let ver = ua.and_then(|u| {
            extract_version_after(u, "Claude-Desktop/")
                .or_else(|| extract_version_after(u, "claude-desktop/"))
                .or_else(|| extract_version_after(u, "Claude/"))
        });
        return (Some("claude-desktop"), ver);
    }

    let product = ua_lower.split_ascii_whitespace().next().unwrap_or_default();
    let mut ua_tokens = ua.unwrap_or_default().split_ascii_whitespace();
    let original_product = ua_tokens.next().unwrap_or_default();

    // 9. 厂商 SDK 的 UA 包含品牌和语言。Stainless 的语言头由多个厂商共用。
    let sdk = match product {
        "anthropic/python" | "asyncanthropic/python" => Some("anthropic-sdk-python"),
        "anthropic/js" | "@anthropic-ai/sdk" => Some("anthropic-sdk-js"),
        "openai/python" | "asyncopenai/python" => Some("openai-sdk-python"),
        "openai/js" => Some("openai-sdk-js"),
        _ if product.starts_with("@anthropic-ai/sdk/") => Some("anthropic-sdk-js"),
        _ => None,
    };
    if let Some(tool) = sdk {
        let version = header_str(headers, "x-stainless-package-version")
            .and_then(parse_version)
            .or_else(|| {
                if product.starts_with("@anthropic-ai/sdk/") {
                    parse_version(&original_product["@anthropic-ai/sdk/".len()..])
                } else {
                    ua_tokens.next().and_then(parse_version)
                }
            });
        return (Some(tool), version);
    }

    if let Some((tool, version)) = ua.and_then(identify_http_product) {
        return (Some(tool), version);
    }

    if let Some(tool) = declared_script(headers) {
        return (Some(tool), None);
    }

    if is_browser(&ua_lower, headers) {
        return (Some("browser"), None);
    }

    (None, None)
}

fn identify_http_product(ua: &str) -> Option<(&'static str, Option<String>)> {
    let mut products = ua.split_ascii_whitespace();
    let first = products.next()?;
    if first.eq_ignore_ascii_case("faraday") {
        // Faraday 默认发送 `Faraday v<version>`。
        let version = products.next()?.strip_prefix(['v', 'V'])?;
        return Some(("faraday", Some(parse_version(version)?)));
    }

    let (first_name, _) = first.split_once('/').unwrap_or((first, ""));
    // aiohttp 和 PowerShell 的默认 UA 分别带 Python 和 Mozilla 前缀。
    let product = if first_name.eq_ignore_ascii_case("python") {
        products
            .next()
            .filter(|token| has_product_name(token, "aiohttp"))?
    } else if first.eq_ignore_ascii_case("mozilla/5.0") {
        products.find(|token| {
            has_product_name(token, "powershell") || has_product_name(token, "windowspowershell")
        })?
    } else {
        first
    };

    let (name, suffix) = product.split_once('/').unwrap_or((product, ""));
    if name.eq_ignore_ascii_case("node") {
        return Some(("node-js", parse_version(suffix)));
    }
    if name.eq_ignore_ascii_case("undici") {
        // 这里的版本属于 HTTP 库。
        return Some(("node-js", None));
    }
    if suffix.is_empty() {
        return None;
    }
    let tool = HTTP_PRODUCTS
        .iter()
        .find_map(|&(product, tool)| name.eq_ignore_ascii_case(product).then_some(tool))?;
    // Go-http-client 的后缀是 HTTP 版本。
    let version = if tool == "go-net-http" {
        None
    } else {
        parse_version(suffix)
    };
    Some((tool, version))
}

fn has_product_name(token: &str, name: &str) -> bool {
    token
        .split_once('/')
        .is_some_and(|(product, _)| product.eq_ignore_ascii_case(name))
}

fn declared_script(headers: &HeaderMap) -> Option<&'static str> {
    let values = headers.get_all("x-ccr-client");
    let mut values = values.iter();
    let value = values.next()?.to_str().ok()?.trim();
    if values.next().is_some() {
        return None;
    }
    SCRIPT_LANGUAGES
        .iter()
        .find_map(|&(language, tool)| value.eq_ignore_ascii_case(language).then_some(tool))
}

fn is_browser(ua: &str, headers: &HeaderMap) -> bool {
    let has_client_hint =
        header_str(headers, "sec-ch-ua").is_some_and(|value| value.trim_start().starts_with('"'));
    // Node.js fetch 也会发 Sec-Fetch-Mode，所以同时要求浏览器的站点上下文。
    let has_fetch_metadata = matches!(
        header_str(headers, "sec-fetch-site"),
        Some("same-origin" | "same-site" | "cross-site" | "none")
    ) && matches!(
        header_str(headers, "sec-fetch-mode"),
        Some("cors" | "no-cors" | "same-origin" | "navigate" | "websocket")
    );
    has_client_hint
        || has_fetch_metadata
        || (ua.starts_with("mozilla/5.0 ")
            && (ua.split_ascii_whitespace().any(|token| {
                [
                    "chrome/",
                    "chromium/",
                    "crios/",
                    "headlesschrome/",
                    "firefox/",
                    "fxios/",
                    "edgios/",
                ]
                .iter()
                .any(|brand| token.starts_with(brand))
            }) || (ua.contains(" version/") && ua.contains(" safari/"))))
}

/// 从 UA 字符串里找 `marker` 后紧跟的版本号 token (到第一个空格/分号/括号/逗号/制表符前为止).
/// 返回值仅在看起来像版本号 (首字符是数字) 时才有效, 否则 None.
fn extract_version_after(ua: &str, marker: &str) -> Option<String> {
    let idx = ua.find(marker)?;
    parse_version(&ua[idx + marker.len()..])
}

fn parse_version(value: &str) -> Option<String> {
    let end = value
        .find(|c: char| c.is_whitespace() || matches!(c, ';' | '(' | ')' | ',' | '\t'))
        .unwrap_or(value.len());
    let token = value[..end].trim_end_matches('/');
    if !token.starts_with(|c: char| c.is_ascii_digit()) {
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
    fn codex_tui_new_frontend() {
        let info = identify(&h(&[(
            "user-agent",
            "codex-tui/0.133.0 (Mac OS 26.4.1; arm64) ghostty/1.3.1 (codex-tui; 0.133.0)",
        )]));
        assert_eq!(info.tool, Some("codex-cli"));
        assert_eq!(info.version.as_deref(), Some("0.133.0"));
    }

    #[test]
    fn codex_desktop_with_prerelease_version() {
        let info = identify(&h(&[(
            "user-agent",
            "Codex Desktop/0.133.0-alpha.1 (Mac OS 26.4.1; arm64) unknown (Codex Desktop; 26.519.41501)",
        )]));
        assert_eq!(info.tool, Some("codex-desktop"));
        assert_eq!(info.version.as_deref(), Some("0.133.0-alpha.1"));
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
    fn recognizes_http_libraries_from_their_product_tokens() {
        for (ua, tool, version) in [
            ("python-requests/2.34.2", "python-requests", "2.34.2"),
            ("python-httpx/0.28.1", "python-httpx", "0.28.1"),
            ("axios/1.21.1", "axios", "1.21.1"),
            ("curl/8.18.0", "curl", "8.18.0"),
        ] {
            let info = identify(&h(&[("user-agent", ua)]));
            assert_eq!(info.tool, Some(tool), "{ua}");
            assert_eq!(info.user_agent.as_deref(), Some(ua));
            assert_eq!(info.version.as_deref(), Some(version), "{ua}");
        }
    }

    #[test]
    fn node_fetch_is_not_a_browser() {
        let info = identify(&h(&[("user-agent", "node"), ("sec-fetch-mode", "cors")]));
        assert_eq!(info.tool, Some("node-js"));
        assert_eq!(info.version, None);
        let info = identify(&h(&[("user-agent", "undici")]));
        assert_eq!(info.tool, Some("node-js"));
        assert_eq!(info.version, None);
    }

    #[test]
    fn recognizes_additional_script_clients() {
        for (ua, tool, version) in [
            ("Go-http-client/1.1", "go-net-http", None),
            ("Go-http-client/2.0", "go-net-http", None),
            (
                "go-resty/2.17.2 (https://github.com/go-resty/resty)",
                "go-resty",
                Some("2.17.2"),
            ),
            ("ureq/3.5.2", "rust-ureq", Some("3.5.2")),
            ("reqwest/0.13.5", "rust-reqwest", Some("0.13.5")),
            ("hyper/1.7.0", "rust-hyper", Some("1.7.0")),
            ("Python-urllib/3.14", "python-urllib", Some("3.14")),
            (
                "Python/3.14 aiohttp/3.14.5",
                "python-aiohttp",
                Some("3.14.5"),
            ),
            ("Bun/1.3.14", "bun", Some("1.3.14")),
            ("Deno/2.9.6", "deno", Some("2.9.6")),
            (
                "Java-http-client/21.0.13.1",
                "java-http-client",
                Some("21.0.13.1"),
            ),
            ("okhttp/3.13.1", "okhttp", Some("3.13.1")),
            (
                "Apache-HttpClient/4.5.14 (Java/21.0.13.1)",
                "apache-http-client",
                Some("4.5.14"),
            ),
            (
                "Mozilla/5.0 (Linux; Ubuntu 26.04.1 LTS; ) PowerShell/7.7.0",
                "powershell",
                Some("7.7.0"),
            ),
            (
                "Mozilla/5.0 (Windows NT; Windows NT 6.1; en-US) WindowsPowerShell/3.0",
                "powershell",
                Some("3.0"),
            ),
            ("GuzzleHttp/7", "guzzle", Some("7")),
            ("Faraday v2.14.1", "faraday", Some("2.14.1")),
            ("HTTPie/3.2.4", "httpie", Some("3.2.4")),
            ("Wget/1.25.0", "wget", Some("1.25.0")),
        ] {
            let info = identify(&h(&[("user-agent", ua)]));
            assert_eq!(info.tool, Some(tool), "{ua}");
            assert_eq!(info.version.as_deref(), version, "{ua}");
            assert!(SUPPORTED_TOOLS.contains(&tool));
        }
    }

    #[test]
    fn accepts_only_supported_script_declarations() {
        for (value, expected) in [
            ("go", "script-go"),
            (" Rust ", "script-rust"),
            ("python", "script-python"),
            ("javascript", "script-javascript"),
            ("typescript", "script-typescript"),
            ("java", "script-java"),
            ("dotnet", "script-dotnet"),
            ("php", "script-php"),
            ("ruby", "script-ruby"),
        ] {
            let info = identify(&h(&[("x-ccr-client", value)]));
            assert_eq!(info.tool, Some(expected), "{value}");
            assert_eq!(info.version, None);
            assert!(SUPPORTED_TOOLS.contains(&expected));
        }
        for value in ["", "my-job", "rust, go", "rust/1.0", "claude-code"] {
            assert_eq!(identify(&h(&[("x-ccr-client", value)])).tool, None);
        }
        let mut duplicate = h(&[("x-ccr-client", "rust")]);
        duplicate.append("x-ccr-client", "go".parse().unwrap());
        assert_eq!(identify(&duplicate).tool, None);
    }

    #[test]
    fn script_declaration_only_overrides_browser_fallback() {
        for (ua, tool) in [
            ("claude-code/2.3.1", "claude-code"),
            ("OpenAI/JS 7.62.0", "openai-sdk-js"),
            ("ureq/3.5.2", "rust-ureq"),
            (
                "Mozilla/5.0 (X11; Linux x86_64) Gecko/20100101 Firefox/132.0",
                "script-rust",
            ),
        ] {
            let info = identify(&h(&[("user-agent", ua), ("x-ccr-client", "rust")]));
            assert_eq!(info.tool, Some(tool), "{ua}");
        }
    }

    #[test]
    fn recognizes_browser_metadata_and_legacy_user_agents() {
        for headers in [
            h(&[("sec-ch-ua", "\"HeadlessChrome\";v=\"159\", \"Not/A)Brand\";v=\"8\", \"Chromium\";v=\"159\"")]),
            h(&[("sec-fetch-site", "same-origin"), ("sec-fetch-mode", "cors")]),
            h(&[("user-agent", "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/159.0.8154.6 Safari/537.36")]),
            h(&[("user-agent", "Mozilla/5.0 (X11; Linux x86_64) Gecko/20100101 Firefox/132.0")]),
            h(&[("user-agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15")]),
        ] {
            let info = identify(&headers);
            assert_eq!(info.tool, Some("browser"));
            assert_eq!(info.version, None);
        }
    }

    #[test]
    fn branded_sdk_takes_precedence_over_generic_headers() {
        for (ua, lang, tool, version) in [
            (
                "OpenAI/Python 3.57.0",
                "python",
                "openai-sdk-python",
                "3.57.0",
            ),
            (
                "AsyncOpenAI/Python 3.57.0",
                "python",
                "openai-sdk-python",
                "3.57.0",
            ),
            ("OpenAI/JS 7.62.0", "js", "openai-sdk-js", "7.62.0"),
            ("Anthropic/JS 1.12.0", "js", "anthropic-sdk-js", "1.12.0"),
            (
                "AsyncAnthropic/Python 2.12.0",
                "python",
                "anthropic-sdk-python",
                "2.12.0",
            ),
        ] {
            let info = identify(&h(&[
                ("user-agent", ua),
                ("x-stainless-lang", lang),
                ("sec-fetch-site", "same-origin"),
                ("sec-fetch-mode", "cors"),
            ]));
            assert_eq!(info.tool, Some(tool), "{ua}");
            assert_eq!(info.version.as_deref(), Some(version), "{ua}");
        }
    }

    #[test]
    fn generic_signals_do_not_guess_the_client() {
        for headers in [
            h(&[("x-stainless-lang", "python")]),
            h(&[("x-stainless-lang", "js")]),
            h(&[("sec-fetch-mode", "cors")]),
            h(&[("user-agent", "Mozilla/5.0 (compatible; MyCrawler/1.0)")]),
            h(&[
                ("user-agent", "my-curl/1.0"),
                ("origin", "https://example.com"),
            ]),
        ] {
            assert_eq!(identify(&headers).tool, None, "{headers:?}");
        }
    }

    #[test]
    fn known_app_takes_precedence_over_generic_signals() {
        let info = identify(&h(&[
            ("user-agent", "OpenAI/JS 7.62.0"),
            ("x-app", "cli"),
            ("sec-fetch-site", "same-origin"),
            ("sec-fetch-mode", "cors"),
        ]));
        assert_eq!(info.tool, Some("claude-code"));
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
        assert_eq!(SUPPORTED_TOOLS.len(), 44);
    }

    #[test]
    fn entry_kind_as_str_matches_url_suffix() {
        assert_eq!(RequestEntryKind::Messages.as_str(), "messages");
        assert_eq!(RequestEntryKind::Responses.as_str(), "responses");
        // 前端详情弹窗渲染 `/v1/{entry_kind}`, 所以带斜杠
        assert_eq!(RequestEntryKind::ChatCompletions.as_str(), "chat/completions");
    }
}
