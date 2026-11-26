//! 规则插件 (`type: "rules"`) 的格式与校验 (设计 §6)。求值在 [`crate::rules_engine`]。
//!
//! 规则 = 条件 + 动作, 动作只能从封闭的动作表里选 (Rust 实现, 插件只填参数)。两个钩子:
//! `route` (选虚拟模型之前: 改道 / 拒绝) 与 `request` (改 Anthropic 形态的请求体)。

use serde::Serialize;
use serde_json::{Map, Value};

use crate::check::{Issue, IssueCode};

/// 当前最高的规则格式版本。2 新增工具条件 (`tool_type` / `tool_offered` / `tool_forced` / `prev_tool`);
/// 1 仍然接受, 但用了 2 才有的条件要声明 2 —— 旧版 app 因此提示「需要更新」而不是「未知条件」。
pub const RULES_API: u64 = 2;
/// 每个条件键最早出现在哪个 `rulesApi` (没列出的都是 1)。
const WHEN_KEYS_SINCE: &[(&str, u64)] = &[("tool_type", 2), ("tool_offered", 2), ("tool_forced", 2), ("prev_tool", 2)];
/// 规则能改道到的虚拟模型 (`jev` 不在其中: `/v1/systemone` 不经过规则)。
pub const VIRTUAL_MODELS: &[&str] = &["fable", "opus", "sonnet", "haiku", "fallback"];
pub const PURPOSES: &[&str] = &["main", "agent", "compact", "guard", "title", "fetch", "search", "probe", "background", "unknown"];
pub const ENTRIES: &[&str] = &["messages", "responses", "chat_completions"];
/// `set_param` / `clamp_param` 能动的请求参数。
pub const PARAM_PATHS: &[&str] = &["temperature", "top_p", "top_k", "max_tokens", "thinking.budget_tokens"];
pub const REDACT_PRESETS: &[&str] = &["api_keys", "email", "phone_cn", "id_cn"];

const MAX_RULES: usize = 100;
const MAX_ACTIONS: usize = 20;
const MAX_PATTERNS: usize = 20;
const MAX_TEXT: usize = 4096;
const MAX_PARAMS: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Hook {
    Route,
    Request,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ParamKind {
    String,
    Number,
    Bool,
    Enum { values: Vec<String> },
    VirtualModel,
}

/// 用户在插件设置里填的参数。`label` 原样保留 (字符串或分语言对象), 由界面本地化。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParamDecl {
    pub id: String,
    #[serde(flatten)]
    pub kind: ParamKind,
    pub label: Value,
    /// 有默认值即可选
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeWindow {
    /// 1 = 周一 … 7 = 周日
    pub days: [bool; 8],
    pub start: u16,
    pub end: u16,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct When {
    pub purpose: Option<Vec<String>>,
    pub purpose_guessed: Option<bool>,
    pub virtual_model: Option<Vec<String>>,
    pub client_tool: Option<Vec<String>>,
    pub entry: Option<Vec<String>>,
    /// 支持末尾 `*` 前缀匹配
    pub model_requested: Option<Vec<String>>,
    pub stream: Option<bool>,
    pub has_tools: Option<bool>,
    pub has_images: Option<bool>,
    pub thinking: Option<bool>,
    pub approx_input_tokens: Option<(Option<u64>, Option<u64>)>,
    pub time: Option<Vec<TimeWindow>>,
    /// 引用一个布尔参数: 用户打开它时规则才生效 (让作者把可选功能做成开关)
    pub param: Option<String>,
    /// `tools[]` 里任一工具的 `type` 匹配 (没写 type 的普通工具算 `custom`); 支持末尾 `*`
    pub tool_type: Option<Vec<String>>,
    /// `tools[]` 里任一工具的名字匹配; 支持末尾 `*`
    pub tool_offered: Option<Vec<String>>,
    /// `tool_choice` 指定的工具名匹配; 支持末尾 `*`
    pub tool_forced: Option<Vec<String>>,
    /// 上一步模型调用的工具 (本请求回传的是它们的结果) 任一匹配; 支持末尾 `*`
    pub prev_tool: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactScope {
    UserText,
    AllText,
}

/// 字符串可以含 `{{params.<id>}}`, 求值时替换成用户参数。
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    UseVirtualModel(String),
    Reject(String),
    SystemAppend(String),
    SetParam { path: String, value: Value },
    ClampParam { path: String, min: Option<f64>, max: Option<f64> },
    RemoveTools(Vec<String>),
    Redact { presets: Vec<String>, patterns: Vec<String>, scope: RedactScope, replacement: String },
}

impl Action {
    pub fn name(&self) -> &'static str {
        match self {
            Self::UseVirtualModel(_) => "use_virtual_model",
            Self::Reject(_) => "reject",
            Self::SystemAppend(_) => "system_append",
            Self::SetParam { .. } => "set_param",
            Self::ClampParam { .. } => "clamp_param",
            Self::RemoveTools(_) => "remove_tools",
            Self::Redact { .. } => "redact",
        }
    }

    fn allowed_in(&self, hook: Hook) -> bool {
        match self {
            Self::Reject(_) => true,
            Self::UseVirtualModel(_) => hook == Hook::Route,
            _ => hook == Hook::Request,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub id: String,
    pub hook: Hook,
    pub when: When,
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RulesPlugin {
    pub params: Vec<ParamDecl>,
    pub rules: Vec<Rule>,
}

/// 能力摘要: 用了哪些钩子 / 动作、可能改道到哪些虚拟模型、有哪些参数。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RulesCapabilities {
    pub hooks: Vec<Hook>,
    pub actions: Vec<&'static str>,
    /// 写死的虚拟模型名, 或 `{{params.x}}` (由用户参数决定)
    pub redirects_to: Vec<String>,
    pub rules: usize,
    pub params: Vec<ParamDecl>,
}

// Value 里有 f64, 不能 derive Eq; 能力摘要只拿来比较与展示, 手写 Eq 足够。
impl Eq for RulesCapabilities {}

fn is_ident(s: &str) -> bool {
    !s.is_empty() && s.len() <= 40 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// 字符串里引用的参数 id (`{{params.<id>}}`)。
pub fn param_refs(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find("{{params.") {
        let after = &rest[i + 9..];
        let Some(end) = after.find("}}") else { break };
        out.push(after[..end].to_string());
        rest = &after[end + 2..];
    }
    out
}

fn parse_hhmm(s: &str) -> Option<u16> {
    let (h, m) = s.split_once(':')?;
    if h.len() != 2 || m.len() != 2 {
        return None;
    }
    let (h, m): (u16, u16) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

struct Ctx<'a> {
    issues: &'a mut Vec<Issue>,
    params: &'a [ParamDecl],
    /// 插件声明的 `rulesApi` (缺失 / 非法时按当前版本, 那个错误另外报)
    api: u64,
}

impl Ctx<'_> {
    fn bad(&mut self, path: impl Into<String>, detail: impl Into<String>) {
        self.issues.push(Issue::new(path, IssueCode::InvalidValue).detail(detail));
    }

    /// 字符串字段: 检查长度与参数引用。
    fn text(&mut self, path: &str, v: &Value, max: usize) -> Option<String> {
        let Some(s) = v.as_str() else {
            self.issues.push(Issue::new(path, IssueCode::InvalidType).detail("string"));
            return None;
        };
        if s.is_empty() || s.len() > max {
            self.bad(path, format!("1..{max} characters"));
            return None;
        }
        for r in param_refs(s) {
            if !self.params.iter().any(|p| p.id == r) {
                self.bad(path, format!("undeclared param {r}"));
            }
        }
        Some(s.to_string())
    }

    fn string_list(&mut self, path: &str, v: &Value, allowed: Option<&[&str]>) -> Option<Vec<String>> {
        let Some(arr) = v.as_array().filter(|a| !a.is_empty() && a.len() <= 50) else {
            self.issues.push(Issue::new(path, IssueCode::InvalidType).detail("non-empty array (max 50)"));
            return None;
        };
        let mut out = Vec::new();
        for (i, item) in arr.iter().enumerate() {
            match item.as_str() {
                Some(s) if allowed.is_none_or(|a| a.contains(&s)) && !s.is_empty() && s.len() <= 200 => out.push(s.to_string()),
                _ => self.bad(format!("{path}[{i}]"), allowed.map(|a| a.join(" / ")).unwrap_or_else(|| "string".into())),
            }
        }
        Some(out)
    }

    fn bool(&mut self, path: &str, v: &Value) -> Option<bool> {
        let b = v.as_bool();
        if b.is_none() {
            self.issues.push(Issue::new(path, IssueCode::InvalidType).detail("bool"));
        }
        b
    }
}

pub fn parse(entry: &str) -> Result<(RulesPlugin, RulesCapabilities), Vec<Issue>> {
    let v: Value = serde_json::from_str(entry)
        .map_err(|e| vec![Issue::new("", IssueCode::InvalidValue).detail(format!("not valid JSON: {e}"))])?;
    let Some(obj) = v.as_object() else {
        return Err(vec![Issue::new("", IssueCode::InvalidType).detail("top level must be an object")]);
    };
    let mut issues = Vec::new();
    let api = obj.get("rulesApi").and_then(Value::as_u64);
    match api {
        Some(1..=RULES_API) => {}
        Some(n) if n > RULES_API => issues.push(Issue::new("rulesApi", IssueCode::NeedsNewerApp).detail(n.to_string())),
        Some(_) => issues.push(Issue::new("rulesApi", IssueCode::UnsupportedVersion)),
        None => issues.push(Issue::new("rulesApi", IssueCode::Missing)),
    }

    let params = parse_params(obj.get("params"), &mut issues);
    let api = api.filter(|n| (1..=RULES_API).contains(n)).unwrap_or(RULES_API);
    let mut ctx = Ctx { issues: &mut issues, params: &params, api };

    let mut rules = Vec::new();
    match obj.get("rules").and_then(Value::as_array) {
        Some(a) if !a.is_empty() && a.len() <= MAX_RULES => {
            for (i, r) in a.iter().enumerate() {
                if let Some(rule) = parse_rule(&format!("rules[{i}]"), r, &mut ctx) {
                    if rules.iter().any(|x: &Rule| x.id == rule.id) {
                        ctx.issues.push(Issue::new(format!("rules[{i}].id"), IssueCode::Duplicate).detail(rule.id.clone()));
                    } else {
                        rules.push(rule);
                    }
                }
            }
        }
        Some(a) if a.len() > MAX_RULES => ctx.issues.push(Issue::new("rules", IssueCode::TooMany).detail(format!("max {MAX_RULES}"))),
        _ => ctx.issues.push(Issue::new("rules", IssueCode::Missing).detail("at least one rule")),
    }
    if !issues.is_empty() {
        return Err(issues);
    }

    let mut hooks = Vec::new();
    let mut actions: Vec<&'static str> = Vec::new();
    let mut redirects: Vec<String> = Vec::new();
    for r in &rules {
        if !hooks.contains(&r.hook) {
            hooks.push(r.hook);
        }
        for a in &r.actions {
            if !actions.contains(&a.name()) {
                actions.push(a.name());
            }
            if let Action::UseVirtualModel(t) = a {
                if !redirects.contains(t) {
                    redirects.push(t.clone());
                }
            }
        }
    }
    actions.sort_unstable();
    let caps = RulesCapabilities { hooks, actions, redirects_to: redirects, rules: rules.len(), params: params.clone() };
    Ok((RulesPlugin { params, rules }, caps))
}

fn parse_params(v: Option<&Value>, issues: &mut Vec<Issue>) -> Vec<ParamDecl> {
    let Some(v) = v else { return Vec::new() };
    let Some(arr) = v.as_array() else {
        issues.push(Issue::new("params", IssueCode::InvalidType).detail("array"));
        return Vec::new();
    };
    if arr.len() > MAX_PARAMS {
        issues.push(Issue::new("params", IssueCode::TooMany).detail(format!("max {MAX_PARAMS}")));
        return Vec::new();
    }
    let mut out: Vec<ParamDecl> = Vec::new();
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
            Some("string") => ParamKind::String,
            Some("number") => ParamKind::Number,
            Some("bool") => ParamKind::Bool,
            Some("virtual_model") => ParamKind::VirtualModel,
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
                ParamKind::Enum { values }
            }
            _ => {
                issues.push(Issue::new(at("type"), IssueCode::InvalidValue).detail("string / number / bool / enum / virtual_model"));
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
        let default = o.get("default").cloned();
        if let Some(d) = &default {
            if let Err(e) = check_param_value(&kind, d) {
                issues.push(Issue::new(at("default"), IssueCode::InvalidValue).detail(e));
                continue;
            }
        }
        out.push(ParamDecl { id: id.to_string(), kind, label, default });
    }
    out
}

/// 参数值是否符合声明的类型。
pub fn check_param_value(kind: &ParamKind, v: &Value) -> Result<(), String> {
    let ok = match kind {
        ParamKind::String => v.as_str().is_some_and(|s| s.len() <= MAX_TEXT),
        ParamKind::Number => v.as_f64().is_some_and(f64::is_finite),
        ParamKind::Bool => v.is_boolean(),
        ParamKind::Enum { values } => v.as_str().is_some_and(|s| values.iter().any(|x| x == s)),
        ParamKind::VirtualModel => v.as_str().is_some_and(|s| VIRTUAL_MODELS.contains(&s)),
    };
    if ok {
        Ok(())
    } else {
        Err(match kind {
            ParamKind::String => "string (max 4096)".into(),
            ParamKind::Number => "number".into(),
            ParamKind::Bool => "bool".into(),
            ParamKind::Enum { values } => values.join(" / "),
            ParamKind::VirtualModel => VIRTUAL_MODELS.join(" / "),
        })
    }
}

/// 用户参数 + 默认值 → 求值用的参数表。缺必填参数 / 类型不对 → 问题列表 (路径是参数 id)。
pub fn resolve_params(decls: &[ParamDecl], user: &Map<String, Value>) -> Result<Map<String, Value>, Vec<Issue>> {
    let mut out = Map::new();
    let mut issues = Vec::new();
    for d in decls {
        match user.get(&d.id).or(d.default.as_ref()) {
            Some(v) => match check_param_value(&d.kind, v) {
                Ok(()) => {
                    out.insert(d.id.clone(), v.clone());
                }
                Err(e) => issues.push(Issue::new(d.id.clone(), IssueCode::InvalidValue).detail(e)),
            },
            None => issues.push(Issue::new(d.id.clone(), IssueCode::Missing)),
        }
    }
    if issues.is_empty() {
        Ok(out)
    } else {
        Err(issues)
    }
}

fn parse_rule(path: &str, v: &Value, ctx: &mut Ctx) -> Option<Rule> {
    let Some(o) = v.as_object() else {
        ctx.issues.push(Issue::new(path, IssueCode::InvalidType));
        return None;
    };
    let id = o.get("id").and_then(Value::as_str).unwrap_or("");
    if !is_ident(id) {
        ctx.bad(format!("{path}.id"), "[a-z0-9_-]{1,40}");
        return None;
    }
    let hook = match o.get("hook").and_then(Value::as_str) {
        Some("route") => Hook::Route,
        Some("request") => Hook::Request,
        _ => {
            ctx.bad(format!("{path}.hook"), "route / request");
            return None;
        }
    };
    let when = match o.get("when") {
        None => When::default(),
        Some(w) => parse_when(&format!("{path}.when"), w, ctx)?,
    };
    let Some(actions_v) = o.get("do").and_then(Value::as_array).filter(|a| !a.is_empty()) else {
        ctx.issues.push(Issue::new(format!("{path}.do"), IssueCode::Missing).detail("at least one action"));
        return None;
    };
    if actions_v.len() > MAX_ACTIONS {
        ctx.issues.push(Issue::new(format!("{path}.do"), IssueCode::TooMany).detail(format!("max {MAX_ACTIONS}")));
        return None;
    }
    let mut actions = Vec::new();
    for (i, a) in actions_v.iter().enumerate() {
        let at = format!("{path}.do[{i}]");
        if let Some(action) = parse_action(&at, a, ctx) {
            if !action.allowed_in(hook) {
                ctx.bad(at, format!("{} is not allowed in the {} hook", action.name(), if hook == Hook::Route { "route" } else { "request" }));
                continue;
            }
            actions.push(action);
        }
    }
    Some(Rule { id: id.to_string(), hook, when, actions })
}

const WHEN_KEYS: &[&str] = &[
    "purpose", "purpose_guessed", "virtual_model", "client_tool", "entry", "model_requested", "stream", "has_tools",
    "has_images", "thinking", "approx_input_tokens", "time", "param", "tool_type", "tool_offered", "tool_forced", "prev_tool",
];

fn parse_when(path: &str, v: &Value, ctx: &mut Ctx) -> Option<When> {
    let Some(o) = v.as_object() else {
        ctx.issues.push(Issue::new(path, IssueCode::InvalidType));
        return None;
    };
    let mut w = When::default();
    for (k, val) in o {
        let at = format!("{path}.{k}");
        if let Some((_, since)) = WHEN_KEYS_SINCE.iter().find(|(name, _)| name == k) {
            if ctx.api < *since {
                ctx.bad(&at, format!("requires \"rulesApi\": {since}"));
                continue;
            }
        }
        match k.as_str() {
            "tool_type" => w.tool_type = ctx.string_list(&at, val, None),
            "tool_offered" => w.tool_offered = ctx.string_list(&at, val, None),
            "tool_forced" => w.tool_forced = ctx.string_list(&at, val, None),
            "prev_tool" => w.prev_tool = ctx.string_list(&at, val, None),
            "purpose" => {
                w.purpose = ctx.string_list(&at, val, Some(PURPOSES));
                // `search` 是 rulesApi 2 才有的取值, 旧版 app 会当成非法值
                if ctx.api < 2 && w.purpose.as_ref().is_some_and(|l| l.iter().any(|x| x == "search")) {
                    ctx.bad(&at, "\"search\" requires \"rulesApi\": 2");
                }
            }
            "virtual_model" => w.virtual_model = ctx.string_list(&at, val, Some(VIRTUAL_MODELS)),
            "entry" => w.entry = ctx.string_list(&at, val, Some(ENTRIES)),
            "client_tool" => w.client_tool = ctx.string_list(&at, val, None),
            "model_requested" => w.model_requested = ctx.string_list(&at, val, None),
            "purpose_guessed" => w.purpose_guessed = ctx.bool(&at, val),
            "stream" => w.stream = ctx.bool(&at, val),
            "has_tools" => w.has_tools = ctx.bool(&at, val),
            "has_images" => w.has_images = ctx.bool(&at, val),
            "thinking" => w.thinking = ctx.bool(&at, val),
            "param" => match val.as_str() {
                Some(id) if ctx.params.iter().any(|p| p.id == id && p.kind == ParamKind::Bool) => w.param = Some(id.to_string()),
                _ => ctx.bad(at, "the id of a declared bool param"),
            },
            "approx_input_tokens" => {
                let gt = val.get("gt").and_then(Value::as_u64);
                let lt = val.get("lt").and_then(Value::as_u64);
                if gt.is_none() && lt.is_none() {
                    ctx.bad(at, "{ gt?: n, lt?: n }");
                } else {
                    w.approx_input_tokens = Some((gt, lt));
                }
            }
            "time" => {
                let Some(arr) = val.as_array().filter(|a| !a.is_empty() && a.len() <= 20) else {
                    ctx.bad(at, "non-empty array (max 20)");
                    continue;
                };
                let mut windows = Vec::new();
                for (i, t) in arr.iter().enumerate() {
                    let start = t.get("start").and_then(Value::as_str).and_then(parse_hhmm);
                    let end = t.get("end").and_then(Value::as_str).and_then(parse_hhmm);
                    let mut days = [false; 8];
                    let day_list = t.get("days").and_then(Value::as_array);
                    let days_ok = day_list.is_some_and(|d| {
                        !d.is_empty()
                            && d.iter().all(|x| match x.as_u64() {
                                Some(n @ 1..=7) => {
                                    days[n as usize] = true;
                                    true
                                }
                                _ => false,
                            })
                    });
                    match (start, end, days_ok) {
                        (Some(s), Some(e), true) if s != e => windows.push(TimeWindow { days, start: s, end: e }),
                        _ => ctx.bad(format!("{at}[{i}]"), "{ days: [1..7], start: \"HH:MM\", end: \"HH:MM\" } with start != end"),
                    }
                }
                w.time = Some(windows);
            }
            _ => ctx.issues.push(Issue::new(at, IssueCode::UnknownKey).detail(format!("one of {}", WHEN_KEYS.join(", ")))),
        }
    }
    Some(w)
}

fn parse_action(path: &str, v: &Value, ctx: &mut Ctx) -> Option<Action> {
    let Some(o) = v.as_object().filter(|o| o.len() == 1) else {
        ctx.bad(path, "an object with exactly one action key");
        return None;
    };
    let (name, arg) = o.iter().next()?;
    let at = format!("{path}.{name}");
    match name.as_str() {
        "use_virtual_model" => {
            let t = ctx.text(&at, arg, 100)?;
            if param_refs(&t).is_empty() && !VIRTUAL_MODELS.contains(&t.as_str()) {
                ctx.bad(&at, VIRTUAL_MODELS.join(" / "));
                return None;
            }
            Some(Action::UseVirtualModel(t))
        }
        "reject" => {
            let msg = arg.get("message").map(|m| ctx.text(&format!("{at}.message"), m, 500)).unwrap_or_else(|| {
                ctx.issues.push(Issue::new(format!("{at}.message"), IssueCode::Missing));
                None
            })?;
            Some(Action::Reject(msg))
        }
        "system_append" => {
            let text = arg.get("text").map(|t| ctx.text(&format!("{at}.text"), t, MAX_TEXT)).unwrap_or_else(|| {
                ctx.issues.push(Issue::new(format!("{at}.text"), IssueCode::Missing));
                None
            })?;
            Some(Action::SystemAppend(text))
        }
        "set_param" => {
            let path_v = arg.get("path").and_then(Value::as_str).unwrap_or("");
            if !PARAM_PATHS.contains(&path_v) {
                ctx.bad(format!("{at}.path"), PARAM_PATHS.join(" / "));
                return None;
            }
            let value = match arg.get("value") {
                Some(Value::Number(n)) if n.as_f64().is_some_and(|f| f.is_finite() && f >= 0.0) => Value::Number(n.clone()),
                Some(s @ Value::String(_)) => Value::String(ctx.text(&format!("{at}.value"), s, 100)?),
                _ => {
                    ctx.bad(format!("{at}.value"), "a non-negative number or {{params.x}}");
                    return None;
                }
            };
            Some(Action::SetParam { path: path_v.to_string(), value })
        }
        "clamp_param" => {
            let path_v = arg.get("path").and_then(Value::as_str).unwrap_or("");
            if !PARAM_PATHS.contains(&path_v) {
                ctx.bad(format!("{at}.path"), PARAM_PATHS.join(" / "));
                return None;
            }
            let min = arg.get("min").and_then(Value::as_f64);
            let max = arg.get("max").and_then(Value::as_f64);
            if min.is_none() && max.is_none() || matches!((min, max), (Some(a), Some(b)) if a > b) {
                ctx.bad(&at, "min and/or max, min <= max");
                return None;
            }
            Some(Action::ClampParam { path: path_v.to_string(), min, max })
        }
        "remove_tools" => {
            let names = ctx.string_list(&format!("{at}.names"), arg.get("names").unwrap_or(&Value::Null), None)?;
            Some(Action::RemoveTools(names))
        }
        "redact" => {
            let presets = match arg.get("preset") {
                Some(p) => ctx.string_list(&format!("{at}.preset"), p, Some(REDACT_PRESETS))?,
                None => Vec::new(),
            };
            let patterns = match arg.get("patterns") {
                Some(p) => {
                    let list = ctx.string_list(&format!("{at}.patterns"), p, None)?;
                    if list.len() > MAX_PATTERNS {
                        ctx.issues.push(Issue::new(format!("{at}.patterns"), IssueCode::TooMany).detail(format!("max {MAX_PATTERNS}")));
                        return None;
                    }
                    for (i, pat) in list.iter().enumerate() {
                        if let Err(e) = crate::rules_engine::compile_pattern(pat) {
                            ctx.bad(format!("{at}.patterns[{i}]"), e);
                        }
                    }
                    list
                }
                None => Vec::new(),
            };
            if presets.is_empty() && patterns.is_empty() {
                ctx.bad(&at, "preset and/or patterns");
                return None;
            }
            let scope = match arg.get("scope").and_then(Value::as_str) {
                None | Some("user_text") => RedactScope::UserText,
                Some("all_text") => RedactScope::AllText,
                Some(_) => {
                    ctx.bad(format!("{at}.scope"), "user_text / all_text");
                    return None;
                }
            };
            let replacement = match arg.get("replacement") {
                None => "[REDACTED]".to_string(),
                Some(r) => r.as_str().filter(|s| s.len() <= 100).map(str::to_string).unwrap_or_else(|| {
                    ctx.bad(format!("{at}.replacement"), "string (max 100)");
                    String::new()
                }),
            };
            Some(Action::Redact { presets, patterns, scope, replacement })
        }
        other => {
            ctx.issues.push(Issue::new(at, IssueCode::UnknownKey).detail(format!("unknown action {other}")));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(src: &str) -> Vec<(String, IssueCode)> {
        parse(src).unwrap_err().into_iter().map(|i| (i.path, i.code)).collect()
    }

    fn rule(r: &str) -> String {
        format!(r#"{{"rulesApi":1,"rules":[{r}]}}"#)
    }

    #[test]
    fn hook_action_compatibility_and_unknowns_are_checked() {
        let c = codes(&rule(r#"{"id":"a","hook":"request","do":[{"use_virtual_model":"haiku"}]}"#));
        assert_eq!(c[0].1, IssueCode::InvalidValue, "改道只能在 route 钩子");
        let c = codes(&rule(r#"{"id":"a","hook":"route","do":[{"system_append":{"text":"x"}}]}"#));
        assert_eq!(c[0].1, IssueCode::InvalidValue);
        let c = codes(&rule(r#"{"id":"a","hook":"route","do":[{"teleport":{}}]}"#));
        assert_eq!(c[0].1, IssueCode::UnknownKey, "未知动作让整个插件失败");
        let c = codes(&rule(r#"{"id":"a","hook":"route","when":{"weather":"sunny"},"do":[{"reject":{"message":"x"}}]}"#));
        assert_eq!(c[0].1, IssueCode::UnknownKey);
        let c = codes(&rule(r#"{"id":"a","hook":"route","do":[{"use_virtual_model":"jev"}]}"#));
        assert_eq!(c[0].1, IssueCode::InvalidValue, "不能改道到 jev");
    }

    #[test]
    fn params_are_declared_typed_and_resolved() {
        let c = codes(&rule(r#"{"id":"a","hook":"route","do":[{"use_virtual_model":"{{params.nope}}"}]}"#));
        assert!(c[0].0.ends_with("use_virtual_model"));
        let (p, caps) = parse(
            r#"{"rulesApi":1,"params":[{"id":"vm","type":"virtual_model","label":"VM"},{"id":"n","type":"number","label":"N","default":3}],
                "rules":[{"id":"a","hook":"route","do":[{"use_virtual_model":"{{params.vm}}"}]}]}"#,
        )
        .unwrap();
        assert_eq!(caps.redirects_to, vec!["{{params.vm}}"]);
        assert!(resolve_params(&p.params, &Map::new()).is_err(), "vm 必填");
        let user: Map<String, Value> = serde_json::from_str(r#"{"vm":"haiku"}"#).unwrap();
        let resolved = resolve_params(&p.params, &user).unwrap();
        assert_eq!(resolved["n"], 3, "默认值补上");
        let bad: Map<String, Value> = serde_json::from_str(r#"{"vm":"jev"}"#).unwrap();
        assert!(resolve_params(&p.params, &bad).is_err());
    }

    #[test]
    fn bad_values_are_rejected() {
        assert!(parse(&rule(r#"{"id":"a","hook":"request","do":[{"set_param":{"path":"model","value":1}}]}"#)).is_err());
        assert!(parse(&rule(r#"{"id":"a","hook":"request","do":[{"clamp_param":{"path":"top_p","min":0.9,"max":0.1}}]}"#)).is_err());
        assert!(parse(&rule(r#"{"id":"a","hook":"request","do":[{"redact":{"patterns":["(unclosed"]}}]}"#)).is_err());
        assert!(parse(&rule(r#"{"id":"a","hook":"route","when":{"time":[{"days":[8],"start":"25:00","end":"01:00"}]},"do":[{"reject":{"message":"x"}}]}"#)).is_err());
        assert!(parse(&rule(r#"{"id":"Bad Id","hook":"route","do":[{"reject":{"message":"x"}}]}"#)).is_err());
        assert!(parse(r#"{"rulesApi":3,"rules":[{"id":"a","hook":"route","do":[{"reject":{"message":"x"}}]}]}"#).is_err());
        assert!(parse(r#"{"rulesApi":0,"rules":[{"id":"a","hook":"route","do":[{"reject":{"message":"x"}}]}]}"#).is_err());
        let dup = format!(r#"{{"rulesApi":1,"rules":[{0},{0}]}}"#, r#"{"id":"a","hook":"route","do":[{"reject":{"message":"x"}}]}"#);
        assert!(parse(&dup).is_err());
    }

    #[test]
    fn tool_conditions_need_rules_api_2() {
        let r = r#"{"id":"a","hook":"route","when":{"tool_type":["web_search_*"],"tool_offered":["mcp__*"],"tool_forced":["x"],"prev_tool":["Read"]},"do":[{"use_virtual_model":"haiku"}]}"#;
        let c = codes(&rule(r));
        assert_eq!(c.len(), 4, "rulesApi 1 下四个工具条件都报错: {c:?}");
        assert!(c.iter().all(|(_, code)| *code == IssueCode::InvalidValue));
        let (p, _) = parse(&format!(r#"{{"rulesApi":2,"rules":[{r}]}}"#)).unwrap();
        let w = &p.rules[0].when;
        assert_eq!(w.tool_type.as_deref(), Some(&["web_search_*".to_string()][..]));
        assert_eq!(w.prev_tool.as_deref(), Some(&["Read".to_string()][..]));
        let search = r#"{"id":"a","hook":"route","when":{"purpose":["search"]},"do":[{"use_virtual_model":"haiku"}]}"#;
        assert!(parse(&rule(search)).is_err(), "search 用途同样要求 rulesApi 2");
        assert!(parse(&format!(r#"{{"rulesApi":2,"rules":[{search}]}}"#)).is_ok());
    }
}
