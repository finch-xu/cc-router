//! 通知插件 (`type: "notifier"`) 的格式与校验 (设计 §7)。模板渲染与 HTTP 请求组装在 [`crate::notifier_template`]。
//!
//! 两个核心安全边界:
//! 1. **网络约束**: URL 的 host 只能是字面量 (`https://`) 或来自 `type: "base_url"` 参数 (用户自己填)。
//!    因此「会连哪些域名」可以静态推导, 作者没法把 URL 拼到未声明的域名。
//! 2. **字段白名单**: 模板里只允许引用公开事件字段表与已声明参数——永远没有 API Key、提示词、请求头、IP、会话 ID。

use std::collections::BTreeMap;
use std::net::IpAddr;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::check::{Issue, IssueCode};
use crate::notifier_template::{extract_refs, parse_template};

pub const NOTIFIER_API: u64 = 1;

/// 公开的事件名表 (只增不删, 设计 §7.3)。
pub const EVENTS: &[&str] = &[
    "subscription.auth_failed",
    "subscription.cooldown",
    "subscription.recovered",
    "subscription.quota_reached",
    "subscription.quota_warning",
    "subscription.balance_low",
    "requests.all_failed",
    "proxy.stopped",
    "update.available",
    "usage.daily_summary",
    "test",
];

/// 允许在模板里引用的事件字段 (只增不删, 绝不含 Key / 提示词 / 请求头 / IP / 会话 ID)。
pub const EVENT_FIELDS: &[&str] = &[
    "event.kind",
    "event.title",
    "event.text",
    "time",
    "subscription.name",
    "provider.name",
    "reason",
    "period",
    "used",
    "limit",
    "percent",
    "remaining",
    "unit",
    "virtual_model",
    "count",
    "version",
    "requests",
    "tokens",
    "costUsd",
    "costCny",
    "topSubscriptions",
];

/// 请求模板 (URL / headers / body) 里额外可用的预渲染字段 (`templates` 渲染出的标题与正文)。
const REQUEST_EXTRA_FIELDS: &[&str] = &["title", "body"];

const MAX_PARAMS: usize = 20;
const MAX_HEADERS: usize = 20;
const MAX_FORM_FIELDS: usize = 30;
const MAX_TEXT: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NotifierParamKind {
    String,
    Number,
    Bool,
    Enum { values: Vec<String> },
    /// 用户自填的服务器地址: 允许 `https://` 或指向回环 / 私有网段的 `http://`
    BaseUrl,
    /// 敏感凭据 (令牌 / Webhook 密钥): 存库明文同 api_key, 永不经 list_plugins 回传明文
    Secret,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NotifierParamDecl {
    pub id: String,
    #[serde(flatten)]
    pub kind: NotifierParamKind,
    pub label: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

impl Eq for NotifierParamDecl {}

impl NotifierParamDecl {
    pub fn is_required(&self) -> bool {
        !self.optional && self.default.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RequestBodySpec {
    Text(String),
    Json(Value),
    Form(BTreeMap<String, String>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct RequestSpec {
    pub method: HttpMethod,
    pub url: String,
    /// 当 URL 以 `{{params.<id>}}` 开头时记录这个 `base_url` 参数 id, 否则 None (字面量 host)。
    pub base_url_param: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub body: Option<RequestBodySpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventTemplate {
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NotifierPlugin {
    pub params: Vec<NotifierParamDecl>,
    pub events: Vec<String>,
    pub request: RequestSpec,
    pub templates: BTreeMap<String, EventTemplate>,
}

/// 能力摘要: 会连哪些域名、哪些由用户 `base_url` 参数决定、订阅的事件、引用的事件字段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NotifierCapabilities {
    pub hosts: Vec<String>,
    pub base_url_params: Vec<String>,
    pub events: Vec<String>,
    pub fields: Vec<String>,
    pub params: Vec<NotifierParamDecl>,
}

fn is_ident(s: &str) -> bool {
    !s.is_empty() && s.len() <= 40 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// 提取 URL 的 `(scheme, host_lower)` (不带端口、方括号)。拒绝含 userinfo (`@`) 或空白 / 控制字符。
pub fn parse_url_scheme_host(url: &str) -> Option<(&str, String)> {
    if url.bytes().any(|b| b <= 0x20 || b == 0x7f) {
        return None;
    }
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let host = if let Some(after) = authority.strip_prefix('[') {
        let (ip, tail) = after.split_once(']')?;
        if !tail.is_empty() && !tail.starts_with(':') {
            return None;
        }
        ip
    } else {
        authority.split(':').next()?
    };
    if host.is_empty() {
        return None;
    }
    Some((scheme, host.to_ascii_lowercase()))
}

/// `base_url` 允许指向回环或 RFC1918 私有网段的 `http://` (自建 ntfy / Gotify / 内网服务)。
pub fn is_loopback_or_private_host(host: &str) -> bool {
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        Ok(IpAddr::V6(v6)) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00,
        Err(_) => false,
    }
}

/// 校验一个 `base_url` 参数值 (不含 query / fragment, 仅 `https://` 或本机/私网 `http://`)。
pub fn validate_base_url(url: &str) -> Result<String, String> {
    if url.is_empty() || url.len() > 500 {
        return Err("URL must be 1..500 characters".into());
    }
    if url.contains(['?', '#']) {
        return Err("base_url cannot contain query or fragment".into());
    }
    let Some((scheme, host)) = parse_url_scheme_host(url) else {
        return Err("not a valid http(s) URL".into());
    };
    if host.contains('{') || host.contains('}') {
        return Err("host cannot contain placeholders".into());
    }
    match scheme {
        "https" => Ok(host),
        "http" if is_loopback_or_private_host(&host) => Ok(host),
        "http" => Err("http:// is only allowed for localhost or private network addresses".into()),
        _ => Err("only https:// (or local/private http://) is allowed".into()),
    }
}

pub fn check_notifier_param_value(kind: &NotifierParamKind, v: &Value) -> Result<(), String> {
    match kind {
        NotifierParamKind::String | NotifierParamKind::Secret => {
            if v.as_str().is_some_and(|s| !s.is_empty() && s.len() <= MAX_TEXT) {
                Ok(())
            } else {
                Err("non-empty string (max 4096)".into())
            }
        }
        NotifierParamKind::Number => {
            if v.as_f64().is_some_and(f64::is_finite) {
                Ok(())
            } else {
                Err("number".into())
            }
        }
        NotifierParamKind::Bool => {
            if v.is_boolean() {
                Ok(())
            } else {
                Err("bool".into())
            }
        }
        NotifierParamKind::Enum { values } => {
            if v.as_str().is_some_and(|s| values.iter().any(|x| x == s)) {
                Ok(())
            } else {
                Err(values.join(" / "))
            }
        }
        NotifierParamKind::BaseUrl => {
            let Some(s) = v.as_str() else {
                return Err("URL string".into());
            };
            validate_base_url(s).map(|_| ())
        }
    }
}

/// 用户参数 + 默认值 → 投递用的参数表。必填参数缺失或类型不对 → 错误列表。
pub fn resolve_notifier_params(decls: &[NotifierParamDecl], user: &Map<String, Value>) -> Result<Map<String, Value>, Vec<Issue>> {
    let mut out = Map::new();
    let mut issues = Vec::new();
    for d in decls {
        match user.get(&d.id).or(d.default.as_ref()) {
            Some(v) => match check_notifier_param_value(&d.kind, v) {
                Ok(()) => {
                    out.insert(d.id.clone(), v.clone());
                }
                Err(e) => issues.push(Issue::new(d.id.clone(), IssueCode::InvalidValue).detail(e)),
            },
            None if d.optional => {}
            None => issues.push(Issue::new(d.id.clone(), IssueCode::Missing)),
        }
    }
    if issues.is_empty() {
        Ok(out)
    } else {
        Err(issues)
    }
}

pub fn parse(entry: &str) -> Result<(NotifierPlugin, NotifierCapabilities), Vec<Issue>> {
    let v: Value = serde_json::from_str(entry)
        .map_err(|e| vec![Issue::new("", IssueCode::InvalidValue).detail(format!("not valid JSON: {e}"))])?;
    let Some(obj) = v.as_object() else {
        return Err(vec![Issue::new("", IssueCode::InvalidType).detail("top level must be an object")]);
    };
    let mut issues = Vec::new();
    match obj.get("notifierApi").and_then(Value::as_u64) {
        Some(NOTIFIER_API) => {}
        Some(n) if n > NOTIFIER_API => issues.push(Issue::new("notifierApi", IssueCode::NeedsNewerApp).detail(n.to_string())),
        Some(_) => issues.push(Issue::new("notifierApi", IssueCode::UnsupportedVersion)),
        None => issues.push(Issue::new("notifierApi", IssueCode::Missing)),
    }

    let params = parse_params(obj.get("params"), &mut issues);

    // events
    let mut events = Vec::new();
    match obj.get("events").and_then(Value::as_array) {
        Some(arr) if !arr.is_empty() && arr.len() <= EVENTS.len() => {
            for (i, item) in arr.iter().enumerate() {
                match item.as_str() {
                    Some(e) if EVENTS.contains(&e) => {
                        if !events.iter().any(|x| x == e) {
                            events.push(e.to_string());
                        }
                    }
                    _ => issues.push(Issue::new(format!("events[{i}]"), IssueCode::InvalidValue).detail(EVENTS.join(" / "))),
                }
            }
        }
        _ => issues.push(Issue::new("events", IssueCode::Missing).detail("non-empty list of event names")),
    }

    let mut used_fields: Vec<String> = Vec::new();
    let mut hosts: Vec<String> = Vec::new();
    let mut base_url_params: Vec<String> = Vec::new();

    // templates (optional)
    let mut templates = BTreeMap::new();
    if let Some(tval) = obj.get("templates") {
        let Some(tobj) = tval.as_object() else {
            issues.push(Issue::new("templates", IssueCode::InvalidType).detail("object"));
            return Err(issues);
        };
        for (k, tv) in tobj {
            if k != "default" && !EVENTS.contains(&k.as_str()) {
                issues.push(Issue::new(format!("templates.{k}"), IssueCode::UnknownKey).detail("event name or default"));
                continue;
            }
            let Some(to) = tv.as_object() else {
                issues.push(Issue::new(format!("templates.{k}"), IssueCode::InvalidType));
                continue;
            };
            let title = check_tpl(&format!("templates.{k}.title"), to.get("title"), &params, false, &mut used_fields, &mut issues)
                .unwrap_or_else(|| "{{event.title}}".into());
            let body = check_tpl(&format!("templates.{k}.body"), to.get("body"), &params, false, &mut used_fields, &mut issues)
                .unwrap_or_else(|| "{{event.text}}".into());
            templates.insert(k.clone(), EventTemplate { title, body });
        }
    }

    // request (required)
    let request = parse_request(
        obj.get("request"),
        &params,
        &mut hosts,
        &mut base_url_params,
        &mut used_fields,
        &mut issues,
    );

    if !issues.is_empty() {
        return Err(issues);
    }
    let request = request.expect("issues empty guarantees request");

    hosts.sort();
    hosts.dedup();
    base_url_params.sort();
    base_url_params.dedup();
    used_fields.sort();
    used_fields.dedup();

    let caps = NotifierCapabilities {
        hosts,
        base_url_params,
        events: events.clone(),
        fields: used_fields,
        params: params.clone(),
    };
    Ok((NotifierPlugin { params, events, request, templates }, caps))
}

fn parse_params(v: Option<&Value>, issues: &mut Vec<Issue>) -> Vec<NotifierParamDecl> {
    let Some(v) = v else { return Vec::new() };
    let Some(arr) = v.as_array() else {
        issues.push(Issue::new("params", IssueCode::InvalidType).detail("array"));
        return Vec::new();
    };
    if arr.len() > MAX_PARAMS {
        issues.push(Issue::new("params", IssueCode::TooMany).detail(format!("max {MAX_PARAMS}")));
        return Vec::new();
    }
    let mut out: Vec<NotifierParamDecl> = Vec::new();
    for (i, p) in arr.iter().enumerate() {
        let at = |k: &str| format!("params[{i}].{k}");
        let Some(o) = p.as_object() else {
            issues.push(Issue::new(format!("params[{i}]"), IssueCode::InvalidType));
            continue;
        };
        let id = o.get("id").and_then(Value::as_str).unwrap_or("");
        if !is_ident(id) {
            issues.push(Issue::new(at("id"), IssueCode::InvalidValue).detail("[a-z0-9_-]{1,40}"));
            continue;
        }
        if out.iter().any(|d| d.id == id) {
            issues.push(Issue::new(at("id"), IssueCode::Duplicate).detail(id.to_string()));
            continue;
        }
        let kind = match o.get("type").and_then(Value::as_str) {
            Some("string") => NotifierParamKind::String,
            Some("number") => NotifierParamKind::Number,
            Some("bool") => NotifierParamKind::Bool,
            Some("base_url") => NotifierParamKind::BaseUrl,
            Some("secret") => NotifierParamKind::Secret,
            Some("enum") => {
                let values: Vec<String> = o
                    .get("values")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                if values.is_empty() || values.len() > 50 {
                    issues.push(Issue::new(at("values"), IssueCode::InvalidValue).detail("1..50 strings"));
                    continue;
                }
                NotifierParamKind::Enum { values }
            }
            _ => {
                issues.push(Issue::new(at("type"), IssueCode::InvalidValue).detail("string / number / bool / enum / base_url / secret"));
                continue;
            }
        };
        let label = match o.get("label") {
            Some(l @ Value::String(s)) if !s.is_empty() => l.clone(),
            Some(l @ Value::Object(m)) if !m.is_empty() && m.values().all(Value::is_string) => l.clone(),
            _ => {
                issues.push(Issue::new(at("label"), IssueCode::Missing));
                continue;
            }
        };
        let optional = o.get("optional").and_then(Value::as_bool).unwrap_or(false);
        let default = o.get("default").cloned();
        if let Some(d) = &default {
            if matches!(kind, NotifierParamKind::Secret) {
                issues.push(Issue::new(at("default"), IssueCode::InvalidValue).detail("secret params cannot have a default"));
                continue;
            }
            if let Err(e) = check_notifier_param_value(&kind, d) {
                issues.push(Issue::new(at("default"), IssueCode::InvalidValue).detail(e));
                continue;
            }
        }
        out.push(NotifierParamDecl { id: id.to_string(), kind, label, default, optional });
    }
    out
}

fn check_template_str(
    path: &str,
    s: &str,
    params: &[NotifierParamDecl],
    allow_request_extras: bool,
    used_fields: &mut Vec<String>,
    issues: &mut Vec<Issue>,
) {
    if s.len() > MAX_TEXT {
        issues.push(Issue::new(path, IssueCode::InvalidValue).detail(format!("max {MAX_TEXT} chars")));
        return;
    }
    let nodes = match parse_template(s) {
        Ok(n) => n,
        Err(e) => {
            issues.push(Issue::new(path, IssueCode::InvalidValue).detail(e));
            return;
        }
    };
    for r in extract_refs(&nodes) {
        if let Some(pid) = r.strip_prefix("params.") {
            if !params.iter().any(|p| p.id == pid) {
                issues.push(Issue::new(path, IssueCode::InvalidValue).detail(format!("undeclared param {pid}")));
            }
        } else if EVENT_FIELDS.contains(&r.as_str()) {
            used_fields.push(r);
        } else if allow_request_extras && REQUEST_EXTRA_FIELDS.contains(&r.as_str()) {
            // `title` / `body` come from templates (which already record any event fields they use)
        } else {
            issues.push(Issue::new(path, IssueCode::InvalidValue).detail(format!("disallowed field {r}")));
        }
    }
}

fn check_tpl(
    path: &str,
    v: Option<&Value>,
    params: &[NotifierParamDecl],
    allow_request_extras: bool,
    used_fields: &mut Vec<String>,
    issues: &mut Vec<Issue>,
) -> Option<String> {
    let Some(v) = v else { return None };
    let Some(s) = v.as_str() else {
        issues.push(Issue::new(path, IssueCode::InvalidType).detail("string"));
        return None;
    };
    check_template_str(path, s, params, allow_request_extras, used_fields, issues);
    Some(s.to_string())
}

fn parse_request(
    v: Option<&Value>,
    params: &[NotifierParamDecl],
    hosts: &mut Vec<String>,
    base_url_params: &mut Vec<String>,
    used_fields: &mut Vec<String>,
    issues: &mut Vec<Issue>,
) -> Option<RequestSpec> {
    let Some(obj) = v.and_then(Value::as_object) else {
        issues.push(Issue::new("request", IssueCode::Missing).detail("request object"));
        return None;
    };
    let method = match obj.get("method").and_then(Value::as_str) {
        None | Some("POST") => HttpMethod::Post,
        Some("PUT") => HttpMethod::Put,
        Some("GET") => HttpMethod::Get,
        Some(_) => {
            issues.push(Issue::new("request.method", IssueCode::InvalidValue).detail("GET / POST / PUT"));
            return None;
        }
    };
    let Some(url) = obj.get("url").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
        issues.push(Issue::new("request.url", IssueCode::Missing));
        return None;
    };
    check_template_str("request.url", url, params, true, used_fields, issues);

    // 校验 URL 的 host 约束: 只能是字面量 https:// host, 或以 `{{params.<base_url>}}` 开头。
    let base_url_param = check_url_host(url, params, hosts, base_url_params, issues);

    let mut headers = BTreeMap::new();
    if let Some(hv) = obj.get("headers") {
        let Some(ho) = hv.as_object() else {
            issues.push(Issue::new("request.headers", IssueCode::InvalidType).detail("object"));
            return None;
        };
        if ho.len() > MAX_HEADERS {
            issues.push(Issue::new("request.headers", IssueCode::TooMany).detail(format!("max {MAX_HEADERS}")));
        }
        for (k, val) in ho {
            if k.is_empty() || k.len() > 80 || !k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
                issues.push(Issue::new(format!("request.headers.{k}"), IssueCode::InvalidValue).detail("invalid header name"));
                continue;
            }
            if let Some(s) = check_tpl(&format!("request.headers.{k}"), Some(val), params, true, used_fields, issues) {
                headers.insert(k.clone(), s);
            }
        }
    }

    let body = if let Some(bv) = obj.get("body") {
        parse_body(bv, params, used_fields, issues)
    } else {
        None
    };

    Some(RequestSpec { method, url: url.to_string(), base_url_param, headers, body })
}

fn check_url_host(
    url: &str,
    params: &[NotifierParamDecl],
    hosts: &mut Vec<String>,
    base_url_params: &mut Vec<String>,
    issues: &mut Vec<Issue>,
) -> Option<String> {
    if let Some(rest) = url.strip_prefix("{{params.") {
        if let Some((pid, after)) = rest.split_once("}}") {
            let Some(decl) = params.iter().find(|p| p.id == pid) else {
                // undeclared param already reported by check_template_str
                return None;
            };
            if decl.kind != NotifierParamKind::BaseUrl {
                issues.push(Issue::new("request.url", IssueCode::HostPlaceholder).detail(format!("param {pid} must have type base_url")));
                return None;
            }
            // base_url 后面必须是空的、`/` 或 `?` (不能拼在 host 后面改变域名!)
            if !after.is_empty() && !after.starts_with('/') && !after.starts_with('?') {
                issues.push(Issue::new("request.url", IssueCode::HostPlaceholder).detail("base_url param must be followed by /, ?, or end of URL"));
                return None;
            }
            base_url_params.push(pid.to_string());
            if let Some(def) = decl.default.as_ref().and_then(Value::as_str) {
                if let Ok(h) = validate_base_url(def) {
                    hosts.push(h);
                }
            }
            return Some(pid.to_string());
        }
    }
    let Some((scheme, host)) = parse_url_scheme_host(url) else {
        issues.push(Issue::new("request.url", IssueCode::InvalidValue).detail("must be https://... or start with {{params.<base_url>}}"));
        return None;
    };
    if host.contains('{') || host.contains('}') {
        issues.push(Issue::new("request.url", IssueCode::HostPlaceholder).detail("host cannot contain placeholders"));
        return None;
    }
    if scheme != "https" {
        issues.push(Issue::new("request.url", IssueCode::InsecureUrl).detail("literal URL must use https://"));
        return None;
    }
    hosts.push(host);
    None
}

fn parse_body(
    v: &Value,
    params: &[NotifierParamDecl],
    used_fields: &mut Vec<String>,
    issues: &mut Vec<Issue>,
) -> Option<RequestBodySpec> {
    let Some(obj) = v.as_object() else {
        issues.push(Issue::new("request.body", IssueCode::InvalidType).detail("object"));
        return None;
    };
    match obj.get("type").and_then(Value::as_str) {
        Some("text") => {
            let tpl = check_tpl("request.body.template", obj.get("template"), params, true, used_fields, issues)?;
            Some(RequestBodySpec::Text(tpl))
        }
        Some("json") => {
            let Some(tpl) = obj.get("template") else {
                issues.push(Issue::new("request.body.template", IssueCode::Missing));
                return None;
            };
            check_json_leaves("request.body.template", tpl, params, used_fields, issues, 0);
            Some(RequestBodySpec::Json(tpl.clone()))
        }
        Some("form") => {
            let Some(fields) = obj.get("fields").and_then(Value::as_object) else {
                issues.push(Issue::new("request.body.fields", IssueCode::Missing).detail("object"));
                return None;
            };
            if fields.is_empty() || fields.len() > MAX_FORM_FIELDS {
                issues.push(Issue::new("request.body.fields", IssueCode::InvalidValue).detail(format!("1..{MAX_FORM_FIELDS} fields")));
                return None;
            }
            let mut out = BTreeMap::new();
            for (k, val) in fields {
                if let Some(s) = check_tpl(&format!("request.body.fields.{k}"), Some(val), params, true, used_fields, issues) {
                    out.insert(k.clone(), s);
                }
            }
            Some(RequestBodySpec::Form(out))
        }
        _ => {
            issues.push(Issue::new("request.body.type", IssueCode::InvalidValue).detail("text / json / form"));
            None
        }
    }
}

fn check_json_leaves(
    path: &str,
    v: &Value,
    params: &[NotifierParamDecl],
    used_fields: &mut Vec<String>,
    issues: &mut Vec<Issue>,
    depth: usize,
) {
    if depth > 12 {
        issues.push(Issue::new(path, IssueCode::InvalidValue).detail("JSON template nested too deeply"));
        return;
    }
    match v {
        Value::String(s) => check_template_str(path, s, params, true, used_fields, issues),
        Value::Array(arr) => {
            for (i, item) in arr.iter().enumerate() {
                check_json_leaves(&format!("{path}[{i}]"), item, params, used_fields, issues, depth + 1);
            }
        }
        Value::Object(map) => {
            for (k, val) in map {
                check_json_leaves(&format!("{path}.{k}"), val, params, used_fields, issues, depth + 1);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NTFY: &str = r#"{
        "notifierApi": 1,
        "params": [
            { "id": "server", "type": "base_url", "label": "Server", "default": "https://ntfy.sh" },
            { "id": "topic", "type": "string", "label": "Topic" },
            { "id": "token", "type": "secret", "label": "Token", "optional": true }
        ],
        "events": ["subscription.auth_failed", "proxy.stopped"],
        "request": {
            "method": "POST",
            "url": "{{params.server}}/{{params.topic}}",
            "headers": {
                "Authorization": "{{#if params.token}}Bearer {{params.token}}{{/if}}",
                "Title": "{{title}}"
            },
            "body": { "type": "text", "template": "{{body}}" }
        },
        "templates": {
            "subscription.auth_failed": { "title": "Auth failed: {{subscription.name}}", "body": "{{provider.name}}: {{reason}}" },
            "default": { "title": "{{event.title}}", "body": "{{event.text}}" }
        }
    }"#;

    #[test]
    fn valid_ntfy_derives_hosts_events_and_fields() {
        let (p, caps) = parse(NTFY).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(p.request.base_url_param.as_deref(), Some("server"));
        assert_eq!(caps.hosts, vec!["ntfy.sh"]);
        assert_eq!(caps.base_url_params, vec!["server"]);
        assert_eq!(caps.events, vec!["subscription.auth_failed", "proxy.stopped"]);
        assert!(caps.fields.contains(&"subscription.name".to_string()));
        assert!(caps.fields.contains(&"provider.name".to_string()));
        assert!(caps.fields.contains(&"event.title".to_string()));
    }

    #[test]
    fn rejects_host_placeholders_insecure_urls_and_secret_fields() {
        // host 含非 base_url 占位符
        let bad_host = r#"{
            "notifierApi": 1,
            "params": [{ "id": "h", "type": "string", "label": "H" }],
            "events": ["test"],
            "request": { "url": "https://{{params.h}}.example.com/hook" }
        }"#;
        assert!(parse(bad_host).unwrap_err().iter().any(|i| i.code == IssueCode::HostPlaceholder));

        // 明文 http 字面量拒绝
        let bad_http = r#"{
            "notifierApi": 1,
            "events": ["test"],
            "request": { "url": "http://hooks.example.com/x" }
        }"#;
        assert!(parse(bad_http).unwrap_err().iter().any(|i| i.code == IssueCode::InsecureUrl));

        // 企图在 base_url 后拼域名后缀
        let bad_suffix = r#"{
            "notifierApi": 1,
            "params": [{ "id": "s", "type": "base_url", "label": "S" }],
            "events": ["test"],
            "request": { "url": "{{params.s}}.evil.com/x" }
        }"#;
        assert!(parse(bad_suffix).unwrap_err().iter().any(|i| i.code == IssueCode::HostPlaceholder));

        // 引用不允许的字段 (如 api_key / prompt)
        let bad_field = r#"{
            "notifierApi": 1,
            "events": ["test"],
            "request": { "url": "https://hooks.example.com/x", "body": { "type": "text", "template": "{{subscription.api_key}}" } }
        }"#;
        assert!(parse(bad_field).is_err());

        // base_url 校验: 公网 http 拒绝, 私网 / 回环 http 允许
        assert!(validate_base_url("http://ntfy.sh").is_err());
        assert!(validate_base_url("http://192.168.1.10:8080").is_ok());
        assert!(validate_base_url("http://127.0.0.1:8080").is_ok());
        assert!(validate_base_url("https://ntfy.sh").is_ok());
    }
}
