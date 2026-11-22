//! 通知插件的 Rust 模板引擎子集与 HTTP 请求渲染 (设计 §7.2)。
//!
//! 语法: `{{path}}` 与 `{{#if path}}...{{else}}...{{/if}}` (可嵌套)。
//! 按位置自动转义:
//! - URL 路径 / 查询段: 对替换出来的参数值做 percent-encode (`base_url` 开头除外)
//! - 请求头: 去掉 CR / LF; 渲染后为空的头自动省略 (配合 `{{#if params.token}}`)
//! - JSON body: 只对字符串叶子做模板替换, 结构本身保持合法 JSON (天然转义)
//! - Form body: 返回 `(key, value)` 列表, 由调用方按 `application/x-www-form-urlencoded` 编码

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::notifier_plugin::{
    parse_url_scheme_host, validate_base_url, HttpMethod, NotifierPlugin, RequestBodySpec,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Literal(String),
    Var(String),
    If {
        cond: String,
        then_branch: Vec<Node>,
        else_branch: Vec<Node>,
    },
}

const MAX_DEPTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    Eof,
    Else,
    EndIf,
}

/// 解析模板文本为 AST。不闭合、未知块标签、非法标识符均返回 Err。
pub fn parse_template(src: &str) -> Result<Vec<Node>, String> {
    let mut rest = src;
    let (nodes, stop) = parse_block(&mut rest, 0, false)?;
    if stop != Stop::Eof || !rest.is_empty() {
        return Err("unexpected closing tag".into());
    }
    Ok(nodes)
}

fn is_path(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 80
        && s.split('.').all(|seg| {
            !seg.is_empty() && seg.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
}

fn parse_block(rest: &mut &str, depth: usize, allow_else: bool) -> Result<(Vec<Node>, Stop), String> {
    if depth > MAX_DEPTH {
        return Err("template nesting too deep".into());
    }
    let mut out = Vec::new();
    while !rest.is_empty() {
        let Some(open) = rest.find("{{") else {
            out.push(Node::Literal((*rest).to_string()));
            *rest = "";
            break;
        };
        if open > 0 {
            out.push(Node::Literal(rest[..open].to_string()));
        }
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            return Err("unclosed {{".into());
        };
        let tag = after[..close].trim();
        *rest = &after[close + 2..];

        if let Some(cond) = tag.strip_prefix("#if ") {
            let cond = cond.trim();
            if !is_path(cond) {
                return Err(format!("invalid #if path: {cond}"));
            }
            let (then_branch, stop) = parse_block(rest, depth + 1, true)?;
            let else_branch = match stop {
                Stop::EndIf => Vec::new(),
                Stop::Else => {
                    let (branch, stop2) = parse_block(rest, depth + 1, false)?;
                    if stop2 != Stop::EndIf {
                        return Err("missing {{/if}}".into());
                    }
                    branch
                }
                Stop::Eof => return Err("missing {{/if}}".into()),
            };
            out.push(Node::If { cond: cond.to_string(), then_branch, else_branch });
        } else if tag == "else" {
            if !allow_else {
                return Err("unexpected {{else}}".into());
            }
            return Ok((out, Stop::Else));
        } else if tag == "/if" {
            if depth == 0 {
                return Err("unexpected {{/if}}".into());
            }
            return Ok((out, Stop::EndIf));
        } else if tag.starts_with('#') || tag.starts_with('/') {
            return Err(format!("unsupported block tag: {tag}"));
        } else {
            if !is_path(tag) {
                return Err(format!("invalid template path: {tag}"));
            }
            out.push(Node::Var(tag.to_string()));
        }
    }
    Ok((out, Stop::Eof))
}

/// 抽取模板中引用到的全部路径 (供校验期做字段白名单检查与能力摘要)。
pub fn extract_refs(nodes: &[Node]) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(nodes: &[Node], out: &mut Vec<String>) {
        for n in nodes {
            match n {
                Node::Literal(_) => {}
                Node::Var(p) => out.push(p.clone()),
                Node::If { cond, then_branch, else_branch } => {
                    out.push(cond.clone());
                    walk(then_branch, out);
                    walk(else_branch, out);
                }
            }
        }
    }
    walk(nodes, &mut out);
    out
}

fn lookup<'a>(ctx: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = ctx;
    for seg in path.split('.') {
        cur = cur.as_object()?.get(seg)?;
    }
    Some(cur)
}

fn is_truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|x| x != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

fn value_to_text(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| match x {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(", "),
        Some(Value::Object(_)) => String::new(),
    }
}

/// 渲染模板, `escape` 控制每个变量值的转义方式。
pub fn render_nodes(nodes: &[Node], ctx: &Value, escape: impl Fn(&str) -> String + Copy) -> String {
    let mut out = String::new();
    for n in nodes {
        match n {
            Node::Literal(s) => out.push_str(s),
            Node::Var(p) => out.push_str(&escape(&value_to_text(lookup(ctx, p)))),
            Node::If { cond, then_branch, else_branch } => {
                let branch = if is_truthy(lookup(ctx, cond)) { then_branch } else { else_branch };
                out.push_str(&render_nodes(branch, ctx, escape));
            }
        }
    }
    out
}

pub fn render_plain(tpl: &str, ctx: &Value) -> Result<String, String> {
    Ok(render_nodes(&parse_template(tpl)?, ctx, |s| s.to_string()))
}

/// RFC 3986 unreserved characters stay unencoded; everything else is percent-encoded (UTF-8 bytes).
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn strip_crlf(s: &str) -> String {
    s.chars().filter(|c| *c != '\r' && *c != '\n').collect()
}

/// 渲染完成的待发 HTTP 请求 (无外部依赖, 调用方直接喂给 reqwest)。
#[derive(Debug, Clone, PartialEq)]
pub enum RenderedBody {
    Text(String),
    Json(Value),
    Form(Vec<(String, String)>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderedRequest {
    pub method: HttpMethod,
    pub url: String,
    pub host: String,
    pub headers: BTreeMap<String, String>,
    pub body: Option<RenderedBody>,
}

/// 把插件定义 + 已解析的用户参数 + 事件上下文渲染成最终 HTTP 请求。
pub fn build_request(
    plugin: &NotifierPlugin,
    params: &Map<String, Value>,
    event_name: &str,
    event_ctx: &Map<String, Value>,
) -> Result<RenderedRequest, String> {
    let mut base_ctx = event_ctx.clone();
    base_ctx.insert("params".into(), Value::Object(params.clone()));
    let base_val = Value::Object(base_ctx.clone());

    let (title_tpl, body_tpl) = plugin
        .templates
        .get(event_name)
        .or_else(|| plugin.templates.get("default"))
        .map(|t| (t.title.as_str(), t.body.as_str()))
        .unwrap_or(("{{event.title}}", "{{event.text}}"));

    let title = render_plain(title_tpl, &base_val)?;
    let body_text = render_plain(body_tpl, &base_val)?;
    base_ctx.insert("title".into(), Value::String(title));
    base_ctx.insert("body".into(), Value::String(body_text));
    let full_ctx = Value::Object(base_ctx);

    // 1. URL 渲染: 开头若是 `base_url` 参数, 原样拼接已校验的 base_url (去末尾 `/`), 其余变量全部 percent-encode。
    let url = if let Some(pid) = &plugin.request.base_url_param {
        let prefix = format!("{{{{params.{pid}}}}}");
        let tail = plugin.request.url.strip_prefix(&prefix).ok_or("URL missing base_url prefix")?;
        let base_raw = params.get(pid).and_then(Value::as_str).ok_or_else(|| format!("param {pid} is not set"))?;
        validate_base_url(base_raw)?;
        let base_clean = base_raw.trim_end_matches('/');
        let tail_rendered = render_nodes(&parse_template(tail)?, &full_ctx, percent_encode);
        format!("{base_clean}{tail_rendered}")
    } else {
        render_nodes(&parse_template(&plugin.request.url)?, &full_ctx, percent_encode)
    };

    let Some((_, host)) = parse_url_scheme_host(&url) else {
        return Err(format!("rendered URL is invalid: {url}"));
    };

    // 2. Headers: 去 CR/LF, 渲染后为空串的自动省略
    let mut headers = BTreeMap::new();
    for (k, tpl) in &plugin.request.headers {
        let val = strip_crlf(&render_plain(tpl, &full_ctx)?);
        if !val.trim().is_empty() {
            headers.insert(k.clone(), val);
        }
    }

    // 3. Body
    let body = match &plugin.request.body {
        None => None,
        Some(RequestBodySpec::Text(tpl)) => Some(RenderedBody::Text(render_plain(tpl, &full_ctx)?)),
        Some(RequestBodySpec::Json(tpl)) => Some(RenderedBody::Json(render_json(tpl, &full_ctx)?)),
        Some(RequestBodySpec::Form(fields)) => {
            let mut pairs = Vec::with_capacity(fields.len());
            for (k, tpl) in fields {
                pairs.push((k.clone(), render_plain(tpl, &full_ctx)?));
            }
            Some(RenderedBody::Form(pairs))
        }
    };

    Ok(RenderedRequest { method: plugin.request.method, url, host, headers, body })
}

fn render_json(v: &Value, ctx: &Value) -> Result<Value, String> {
    match v {
        Value::String(s) => Ok(Value::String(render_plain(s, ctx)?)),
        Value::Array(arr) => {
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                out.push(render_json(item, ctx)?);
            }
            Ok(Value::Array(out))
        }
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, val) in map {
                out.insert(k.clone(), render_json(val, ctx)?);
            }
            Ok(Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifier_plugin::parse;
    use serde_json::json;

    #[test]
    fn template_if_else_and_escaping() {
        let ctx = json!({ "a": "hello", "b": "", "params": { "tok": "x\r\ny" } });
        assert_eq!(render_plain("{{#if a}}Y:{{a}}{{else}}N{{/if}}", &ctx).unwrap(), "Y:hello");
        assert_eq!(render_plain("{{#if b}}Y{{else}}N{{/if}}", &ctx).unwrap(), "N");
        assert_eq!(percent_encode("a b/c?d=1"), "a%20b%2Fc%3Fd%3D1");
        assert!(parse_template("{{#if a}}no close").is_err());
        assert!(parse_template("{{else}}").is_err());
    }

    #[test]
    fn rendered_request_escapes_every_position_and_respects_hosts() {
        let spec = r#"{
            "notifierApi": 1,
            "params": [
                { "id": "server", "type": "base_url", "label": "Server", "default": "https://ntfy.sh" },
                { "id": "topic", "type": "string", "label": "Topic" },
                { "id": "token", "type": "secret", "label": "Token", "optional": true }
            ],
            "events": ["subscription.auth_failed"],
            "request": {
                "method": "POST",
                "url": "{{params.server}}/{{params.topic}}",
                "headers": {
                    "Authorization": "{{#if params.token}}Bearer {{params.token}}{{/if}}",
                    "Title": "{{title}}"
                },
                "body": { "type": "json", "template": { "msg": "{{body}}", "nested": ["{{title}}"] } }
            },
            "templates": {
                "subscription.auth_failed": { "title": "Fail\r\nInject: {{subscription.name}}", "body": "Quote \"here\" & {{reason}}" }
            }
        }"#;
        let (plugin, caps) = parse(spec).unwrap();
        let params: Map<String, Value> = serde_json::from_str(r#"{"server":"https://ntfy.sh/","topic":"my/topic"}"#).unwrap();
        let ctx: Map<String, Value> = serde_json::from_str(r#"{"subscription":{"name":"SubA"},"reason":"HTTP 401"}"#).unwrap();
        let req = build_request(&plugin, &params, "subscription.auth_failed", &ctx).unwrap();

        // 1. 域名与能力摘要一致; topic 中的 `/` 被 percent-encode
        assert_eq!(req.host, "ntfy.sh");
        assert!(caps.hosts.contains(&req.host));
        assert_eq!(req.url, "https://ntfy.sh/my%2Ftopic");
        // 2. 空可选 header 被省略; CR/LF 从 Title 剥离
        assert!(!req.headers.contains_key("Authorization"));
        assert_eq!(req.headers["Title"], "FailInject: SubA");
        // 3. JSON 叶子天然转义
        assert_eq!(
            req.body,
            Some(RenderedBody::Json(json!({ "msg": "Quote \"here\" & HTTP 401", "nested": ["Fail\r\nInject: SubA"] })))
        );
    }

    #[test]
    fn literal_host_cannot_be_altered_by_params() {
        let spec = r#"{
            "notifierApi": 1,
            "params": [{ "id": "token", "type": "secret", "label": "Token" }],
            "events": ["test"],
            "request": { "url": "https://api.telegram.org/bot{{params.token}}/sendMessage" }
        }"#;
        let (plugin, caps) = parse(spec).unwrap();
        // 即使参数含 `@evil.com/` 也因为 percent-encode 无法改变 host
        let params: Map<String, Value> = serde_json::from_str(r#"{"token":"123@evil.com/x"}"#).unwrap();
        let req = build_request(&plugin, &params, "test", &Map::new()).unwrap();
        assert_eq!(req.host, "api.telegram.org");
        assert_eq!(caps.hosts, vec!["api.telegram.org"]);
        assert!(req.url.contains("123%40evil.com%2Fx"));
    }
}
