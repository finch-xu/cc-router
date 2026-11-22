//! 规则求值 (设计 §6.5)。纯函数: 输入是已解析的规则插件 + 用户参数 + 请求事实, 输出是决定与改写。
//!
//! 纪律: 一个插件求值出错 (参数缺失、模板解析失败) → 跳过它的全部改动、记下错误, **请求照常发出**;
//! `reject` 是唯一能让请求失败的动作, 而且是作者明写的。

use regex::{Regex, RegexBuilder};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::rules_plugin::{param_refs, Action, Hook, RedactScope, Rule, RulesPlugin, TimeWindow, When, VIRTUAL_MODELS};

/// 已启用的规则插件 + 解析好的用户参数 (含默认值)。
#[derive(Debug, Clone)]
pub struct LoadedRules {
    pub plugin_id: String,
    pub version: String,
    pub plugin: RulesPlugin,
    pub params: Map<String, Value>,
}

/// 请求的事实 (与 body 无关的部分由调用方给; body 相关的由 [`BodyFacts::of`] 算)。
#[derive(Debug, Clone)]
pub struct Facts<'a> {
    pub purpose: &'a str,
    pub purpose_guessed: bool,
    /// 虚拟模型短名: fable / opus / sonnet / haiku / fallback
    pub virtual_model: &'a str,
    /// 客户端 id; 未识别为 "unknown"
    pub client_tool: &'a str,
    /// messages / responses / chat_completions
    pub entry: &'a str,
    /// 客户端请求里原始的 model
    pub model: &'a str,
    /// 本地时间: 1 = 周一 … 7 = 周日
    pub weekday: u8,
    /// 本地时间: 0 点起的分钟数
    pub minute: u16,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BodyFacts {
    pub stream: bool,
    pub has_tools: bool,
    pub has_images: bool,
    pub thinking: bool,
    /// system + messages 的文本字符数 ÷ 4 (粗估, 文档写明)
    pub approx_input_tokens: u64,
}

impl BodyFacts {
    pub fn of(body: &Value) -> Self {
        let has_images = body
            .get("messages")
            .and_then(Value::as_array)
            .is_some_and(|msgs| msgs.iter().any(|m| blocks(m.get("content")).any(|b| b.get("type").and_then(Value::as_str) == Some("image"))));
        let thinking = body
            .get("thinking")
            .and_then(|t| t.get("type"))
            .and_then(Value::as_str)
            .is_some_and(|t| t != "disabled");
        let mut chars = 0usize;
        visit_text(body.get("system"), &mut |s| chars += s.chars().count());
        if let Some(msgs) = body.get("messages").and_then(Value::as_array) {
            for m in msgs {
                visit_text(m.get("content"), &mut |s| chars += s.chars().count());
            }
        }
        Self {
            stream: body.get("stream").and_then(Value::as_bool).unwrap_or(false),
            has_tools: body.get("tools").and_then(Value::as_array).is_some_and(|t| !t.is_empty()),
            has_images,
            thinking,
            approx_input_tokens: (chars / 4) as u64,
        }
    }
}

fn blocks(content: Option<&Value>) -> impl Iterator<Item = &Value> {
    content.and_then(Value::as_array).into_iter().flatten()
}

/// 只读遍历文本: 字符串内容、text 块、tool_result 里的文本。
fn visit_text(v: Option<&Value>, f: &mut impl FnMut(&str)) {
    match v {
        Some(Value::String(s)) => f(s),
        Some(Value::Array(items)) => {
            for b in items {
                match b.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(s) = b.get("text").and_then(Value::as_str) {
                            f(s)
                        }
                    }
                    Some("tool_result") => visit_text(b.get("content"), f),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// 一个插件这次做了什么 (写进请求日志)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Applied {
    pub plugin_id: String,
    pub version: String,
    /// 命中并执行了动作的规则 id
    pub rules: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteDecision {
    Continue,
    /// 改道到这个虚拟模型 (短名)
    UseVirtualModel(String),
    Reject(String),
}

fn time_matches(windows: &[TimeWindow], weekday: u8, minute: u16) -> bool {
    windows.iter().any(|w| {
        let day = |d: u8| w.days.get(d as usize).copied().unwrap_or(false);
        if w.start < w.end {
            day(weekday) && (w.start..w.end).contains(&minute)
        } else {
            // 跨午夜: 前半段属于当天, 后半段属于前一天开始的窗口
            let prev = if weekday == 1 { 7 } else { weekday - 1 };
            (day(weekday) && minute >= w.start) || (day(prev) && minute < w.end)
        }
    })
}

fn glob_match(patterns: &[String], s: &str) -> bool {
    patterns.iter().any(|p| match p.strip_suffix('*') {
        Some(prefix) => s.starts_with(prefix),
        None => p == s,
    })
}

fn matches(w: &When, f: &Facts, b: &BodyFacts, params: &Map<String, Value>) -> bool {
    let in_list = |list: &Option<Vec<String>>, v: &str| list.as_ref().is_none_or(|l| l.iter().any(|x| x == v));
    let flag = |want: Option<bool>, have: bool| want.is_none_or(|x| x == have);
    in_list(&w.purpose, f.purpose)
        && flag(w.purpose_guessed, f.purpose_guessed)
        && in_list(&w.virtual_model, f.virtual_model)
        && in_list(&w.client_tool, f.client_tool)
        && in_list(&w.entry, f.entry)
        && w.model_requested.as_ref().is_none_or(|p| glob_match(p, f.model))
        && flag(w.stream, b.stream)
        && flag(w.has_tools, b.has_tools)
        && flag(w.has_images, b.has_images)
        && flag(w.thinking, b.thinking)
        && w.approx_input_tokens.is_none_or(|(gt, lt)| {
            gt.is_none_or(|g| b.approx_input_tokens > g) && lt.is_none_or(|l| b.approx_input_tokens < l)
        })
        && w.time.as_ref().is_none_or(|t| time_matches(t, f.weekday, f.minute))
        && w.param.as_ref().is_none_or(|id| params.get(id).and_then(Value::as_bool).unwrap_or(false))
}

/// 试运行时只能从请求日志列推出来的部分 body 事实; 拿不到的字段为 `None` (命中前标记为「无法判断」)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PartialBodyFacts {
    pub stream: Option<bool>,
    pub has_tools: Option<bool>,
    pub has_images: Option<bool>,
    pub thinking: Option<bool>,
    pub approx_input_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleDryRunOutcome {
    NotMatched,
    Matched,
    Unknown { fields: Vec<&'static str> },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DryRunEval {
    pub matched_rules: Vec<String>,
    pub unknown_rules: Vec<String>,
    pub unknown_fields: Vec<String>,
    pub redirect_to: Option<String>,
    pub rejected: bool,
}

/// 针对历史请求日志的规则试运行匹配:
/// - 只要任一已知条件不满足 → `NotMatched`;
/// - 所有已知条件都满足但有依赖 body 的条件拿不到值 → `Unknown { fields }`;
/// - 全部满足 → `Matched`。
pub fn dry_run_rule(w: &When, f: &Facts, b: &PartialBodyFacts, params: &Map<String, Value>) -> RuleDryRunOutcome {
    let in_list = |list: &Option<Vec<String>>, v: &str| list.as_ref().is_none_or(|l| l.iter().any(|x| x == v));
    let flag = |want: Option<bool>, have: bool| want.is_none_or(|x| x == have);
    if !(in_list(&w.purpose, f.purpose)
        && flag(w.purpose_guessed, f.purpose_guessed)
        && in_list(&w.virtual_model, f.virtual_model)
        && in_list(&w.client_tool, f.client_tool)
        && in_list(&w.entry, f.entry)
        && w.model_requested.as_ref().is_none_or(|p| glob_match(p, f.model))
        && w.time.as_ref().is_none_or(|t| time_matches(t, f.weekday, f.minute))
        && w.param.as_ref().is_none_or(|id| params.get(id).and_then(Value::as_bool).unwrap_or(false)))
    {
        return RuleDryRunOutcome::NotMatched;
    }

    let mut unknown = Vec::new();
    let mut check_bool = |want: Option<bool>, have: Option<bool>, name: &'static str| -> bool {
        let Some(expected) = want else { return true };
        match have {
            Some(actual) => actual == expected,
            None => {
                unknown.push(name);
                true
            }
        }
    };
    if !check_bool(w.stream, b.stream, "stream") {
        return RuleDryRunOutcome::NotMatched;
    }
    if !check_bool(w.has_tools, b.has_tools, "has_tools") {
        return RuleDryRunOutcome::NotMatched;
    }
    if !check_bool(w.has_images, b.has_images, "has_images") {
        return RuleDryRunOutcome::NotMatched;
    }
    if !check_bool(w.thinking, b.thinking, "thinking") {
        return RuleDryRunOutcome::NotMatched;
    }
    if let Some((gt, lt)) = w.approx_input_tokens {
        match b.approx_input_tokens {
            Some(actual) => {
                if !(gt.is_none_or(|g| actual > g) && lt.is_none_or(|l| actual < l)) {
                    return RuleDryRunOutcome::NotMatched;
                }
            }
            None => unknown.push("approx_input_tokens"),
        }
    }

    if unknown.is_empty() {
        RuleDryRunOutcome::Matched
    } else {
        RuleDryRunOutcome::Unknown { fields: unknown }
    }
}

/// 用历史请求日志的一行事实试运行单个规则插件 (不产生任何副作用)。
pub fn dry_run_plugin(plugin: &LoadedRules, facts: &Facts, body: &PartialBodyFacts) -> DryRunEval {
    let mut out = DryRunEval::default();
    let push_unknown_fields = |fields: Vec<&'static str>, dst: &mut Vec<String>| {
        for f in fields {
            if !dst.iter().any(|x| x == f) {
                dst.push(f.to_string());
            }
        }
    };

    for rule in plugin.plugin.rules.iter().filter(|r| r.hook == Hook::Route) {
        match dry_run_rule(&rule.when, facts, body, &plugin.params) {
            RuleDryRunOutcome::NotMatched => {}
            RuleDryRunOutcome::Unknown { fields } => {
                out.unknown_rules.push(rule.id.clone());
                push_unknown_fields(fields, &mut out.unknown_fields);
            }
            RuleDryRunOutcome::Matched => {
                match route_actions(rule, &plugin.params) {
                    Ok(RouteDecision::UseVirtualModel(vm)) => {
                        out.matched_rules.push(rule.id.clone());
                        out.redirect_to = Some(vm);
                        break;
                    }
                    Ok(RouteDecision::Reject(_)) => {
                        out.matched_rules.push(rule.id.clone());
                        out.rejected = true;
                        return out;
                    }
                    _ => {}
                }
            }
        }
    }

    let req_facts = Facts {
        virtual_model: out.redirect_to.as_deref().unwrap_or(facts.virtual_model),
        ..facts.clone()
    };
    for rule in plugin.plugin.rules.iter().filter(|r| r.hook == Hook::Request) {
        match dry_run_rule(&rule.when, &req_facts, body, &plugin.params) {
            RuleDryRunOutcome::NotMatched => {}
            RuleDryRunOutcome::Unknown { fields } => {
                out.unknown_rules.push(rule.id.clone());
                push_unknown_fields(fields, &mut out.unknown_fields);
            }
            RuleDryRunOutcome::Matched => {
                out.matched_rules.push(rule.id.clone());
                if rule.actions.iter().any(|a| matches!(a, Action::Reject(_))) {
                    out.rejected = true;
                    break;
                }
            }
        }
    }

    out
}

/// 把字符串里的 `{{params.x}}` 换成参数值。引用了不存在的参数 → Err。
fn render(s: &str, params: &Map<String, Value>) -> Result<String, String> {
    let mut out = s.to_string();
    for r in param_refs(s) {
        let v = params.get(&r).ok_or_else(|| format!("param {r} is not set"))?;
        let text = match v {
            Value::String(x) => x.clone(),
            other => other.to_string(),
        };
        out = out.replace(&format!("{{{{params.{r}}}}}"), &text);
    }
    Ok(out)
}

/// 规则可用的正则: 限制编译后的大小, 防止病态表达式吃内存 (regex crate 本身是线性时间)。
pub fn compile_pattern(p: &str) -> Result<Regex, String> {
    RegexBuilder::new(p).size_limit(1 << 20).dfa_size_limit(1 << 20).build().map_err(|e| e.to_string())
}

fn preset_pattern(name: &str) -> &'static str {
    match name {
        "api_keys" => r"\b(?:sk-ant-[A-Za-z0-9_\-]{20,}|sk-[A-Za-z0-9_\-]{20,}|AKIA[0-9A-Z]{16}|ghp_[A-Za-z0-9]{36}|github_pat_[A-Za-z0-9_]{40,}|xox[baprs]-[A-Za-z0-9\-]{10,}|AIza[0-9A-Za-z_\-]{35})\b",
        "email" => r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}",
        "phone_cn" => r"\b1[3-9]\d{9}\b",
        "id_cn" => r"\b\d{17}[\dXx]\b",
        _ => r"$^",
    }
}

/// `route` 钩子: 第一条改道 / 拒绝生效, 之后的 route 规则不再看。
pub fn apply_route(plugins: &[LoadedRules], facts: &Facts, body: &Value) -> (RouteDecision, Vec<Applied>) {
    let bf = BodyFacts::of(body);
    let mut log = Vec::new();
    for p in plugins {
        let mut applied = Applied { plugin_id: p.plugin_id.clone(), version: p.version.clone(), rules: Vec::new(), error: None };
        let mut decision = RouteDecision::Continue;
        for rule in p.plugin.rules.iter().filter(|r| r.hook == Hook::Route) {
            if !matches(&rule.when, facts, &bf, &p.params) {
                continue;
            }
            match route_actions(rule, &p.params) {
                Ok(RouteDecision::Continue) => {}
                Ok(d) => {
                    applied.rules.push(rule.id.clone());
                    decision = d;
                    break;
                }
                Err(e) => {
                    applied.error = Some(format!("{}: {e}", rule.id));
                    applied.rules.clear();
                    decision = RouteDecision::Continue;
                    break;
                }
            }
        }
        let done = decision != RouteDecision::Continue;
        if !applied.rules.is_empty() || applied.error.is_some() {
            log.push(applied);
        }
        if done {
            return (decision, log);
        }
    }
    (RouteDecision::Continue, log)
}

fn route_actions(rule: &Rule, params: &Map<String, Value>) -> Result<RouteDecision, String> {
    for a in &rule.actions {
        match a {
            Action::Reject(msg) => return Ok(RouteDecision::Reject(render(msg, params)?)),
            Action::UseVirtualModel(t) => {
                let vm = render(t, params)?;
                if !VIRTUAL_MODELS.contains(&vm.as_str()) {
                    return Err(format!("{vm} is not a virtual model"));
                }
                return Ok(RouteDecision::UseVirtualModel(vm));
            }
            _ => {}
        }
    }
    Ok(RouteDecision::Continue)
}

/// `request` 钩子: 每个插件在副本上改, 全部成功才生效。返回 Some(message) 表示被 `reject`。
pub fn apply_request(plugins: &[LoadedRules], facts: &Facts, body: &mut Value) -> (Option<String>, Vec<Applied>) {
    let mut log = Vec::new();
    for p in plugins {
        let bf = BodyFacts::of(body);
        let mut draft = body.clone();
        let mut applied = Applied { plugin_id: p.plugin_id.clone(), version: p.version.clone(), rules: Vec::new(), error: None };
        let mut reject = None;
        for rule in p.plugin.rules.iter().filter(|r| r.hook == Hook::Request) {
            if !matches(&rule.when, facts, &bf, &p.params) {
                continue;
            }
            match request_actions(rule, &p.params, &mut draft) {
                Ok(None) => applied.rules.push(rule.id.clone()),
                Ok(Some(msg)) => {
                    applied.rules.push(rule.id.clone());
                    reject = Some(msg);
                    break;
                }
                Err(e) => {
                    applied.error = Some(format!("{}: {e}", rule.id));
                    break;
                }
            }
        }
        if applied.error.is_some() {
            applied.rules.clear();
            log.push(applied);
            continue;
        }
        if !applied.rules.is_empty() {
            *body = draft;
            log.push(applied);
        }
        if reject.is_some() {
            return (reject, log);
        }
    }
    (None, log)
}

fn request_actions(rule: &Rule, params: &Map<String, Value>, body: &mut Value) -> Result<Option<String>, String> {
    for a in &rule.actions {
        match a {
            Action::Reject(msg) => return Ok(Some(render(msg, params)?)),
            Action::SystemAppend(t) => system_append(body, &render(t, params)?),
            Action::SetParam { path, value } => {
                let n = match value {
                    Value::String(t) => render(t, params)?.trim().parse::<f64>().map_err(|_| format!("{path}: not a number"))?,
                    v => v.as_f64().ok_or_else(|| format!("{path}: not a number"))?,
                };
                set_param(body, path, n);
            }
            Action::ClampParam { path, min, max } => clamp_param(body, path, *min, *max),
            Action::RemoveTools(names) => remove_tools(body, names),
            Action::Redact { presets, patterns, scope, replacement } => {
                let mut res: Vec<Regex> = Vec::new();
                for p in presets {
                    res.push(compile_pattern(preset_pattern(p))?);
                }
                for p in patterns {
                    res.push(compile_pattern(p)?);
                }
                redact(body, &res, *scope, replacement);
            }
            Action::UseVirtualModel(_) => {}
        }
    }
    Ok(None)
}

fn system_append(body: &mut Value, text: &str) {
    let Some(obj) = body.as_object_mut() else { return };
    match obj.get_mut("system") {
        Some(Value::String(s)) if !s.is_empty() => {
            s.push_str("\n\n");
            s.push_str(text);
        }
        Some(Value::Array(blocks)) => blocks.push(serde_json::json!({ "type": "text", "text": text })),
        _ => {
            obj.insert("system".into(), Value::String(text.to_string()));
        }
    }
}

fn is_integer_param(path: &str) -> bool {
    matches!(path, "top_k" | "max_tokens" | "thinking.budget_tokens")
}

fn num(path: &str, n: f64) -> Value {
    if is_integer_param(path) {
        Value::from(n.round().max(0.0) as u64)
    } else {
        serde_json::Number::from_f64(n).map(Value::Number).unwrap_or(Value::Null)
    }
}

/// `thinking.budget_tokens` 只在已开启思考时改 (凭空开思考会改变请求语义)。
fn param_slot<'a>(body: &'a mut Value, path: &str, create: bool) -> Option<&'a mut Value> {
    let obj = body.as_object_mut()?;
    if path == "thinking.budget_tokens" {
        let t = obj.get_mut("thinking")?.as_object_mut()?;
        if t.get("type").and_then(Value::as_str) != Some("enabled") {
            return None;
        }
        return if create { Some(t.entry("budget_tokens").or_insert(Value::Null)) } else { t.get_mut("budget_tokens") };
    }
    if create {
        Some(obj.entry(path.to_string()).or_insert(Value::Null))
    } else {
        obj.get_mut(path)
    }
}

fn set_param(body: &mut Value, path: &str, n: f64) {
    if let Some(slot) = param_slot(body, path, true) {
        *slot = num(path, n);
    }
}

fn clamp_param(body: &mut Value, path: &str, min: Option<f64>, max: Option<f64>) {
    if let Some(slot) = param_slot(body, path, false) {
        if let Some(cur) = slot.as_f64() {
            let mut v = cur;
            if let Some(lo) = min {
                v = v.max(lo);
            }
            if let Some(hi) = max {
                v = v.min(hi);
            }
            if v != cur {
                *slot = num(path, v);
            }
        }
    }
}

fn remove_tools(body: &mut Value, names: &[String]) {
    let Some(obj) = body.as_object_mut() else { return };
    if let Some(Value::Array(tools)) = obj.get_mut("tools") {
        tools.retain(|t| !t.get("name").and_then(Value::as_str).is_some_and(|n| glob_match(names, n)));
    }
    // tool_choice 指向被删的工具时一并去掉, 否则上游 400
    let chosen_removed = obj
        .get("tool_choice")
        .and_then(|c| c.get("name"))
        .and_then(Value::as_str)
        .is_some_and(|n| glob_match(names, n));
    if chosen_removed {
        obj.remove("tool_choice");
    }
}

fn redact_str(s: &mut String, res: &[Regex], replacement: &str) {
    for re in res {
        if re.is_match(s) {
            *s = re.replace_all(s, replacement).into_owned();
        }
    }
}

fn redact_content(v: &mut Value, res: &[Regex], replacement: &str) {
    match v {
        Value::String(s) => redact_str(s, res, replacement),
        Value::Array(items) => {
            for b in items {
                match b.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(Value::String(s)) = b.get_mut("text") {
                            redact_str(s, res, replacement);
                        }
                    }
                    Some("tool_result") => {
                        if let Some(c) = b.get_mut("content") {
                            redact_content(c, res, replacement);
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// 替换结果是确定的 (同样的输入每轮得到同样的输出), 不会破坏提示词缓存的前缀。
fn redact(body: &mut Value, res: &[Regex], scope: RedactScope, replacement: &str) {
    let Some(obj) = body.as_object_mut() else { return };
    if scope == RedactScope::AllText {
        if let Some(s) = obj.get_mut("system") {
            redact_content(s, res, replacement);
        }
    }
    if let Some(Value::Array(msgs)) = obj.get_mut("messages") {
        for m in msgs {
            let role = m.get("role").and_then(Value::as_str).unwrap_or("");
            if role == "user" || scope == RedactScope::AllText {
                if let Some(c) = m.get_mut("content") {
                    redact_content(c, res, replacement);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules_plugin::parse;
    use serde_json::json;

    fn load(rules: &str, params: Value) -> LoadedRules {
        let (plugin, _) = parse(rules).unwrap_or_else(|e| panic!("{e:?}"));
        LoadedRules { plugin_id: "p".into(), version: "1".into(), plugin, params: params.as_object().cloned().unwrap_or_default() }
    }

    fn facts<'a>(purpose: &'a str, vm: &'a str) -> Facts<'a> {
        Facts { purpose, purpose_guessed: true, virtual_model: vm, client_tool: "claude_code", entry: "messages", model: "model-opus", weekday: 3, minute: 600 }
    }

    const SIDE: &str = r#"{"rulesApi":1,
      "params":[{"id":"cheap","type":"virtual_model","label":"Cheap","default":"haiku"}],
      "rules":[{"id":"side","hook":"route","when":{"purpose":["title","compact"],"virtual_model":["opus","sonnet"]},
                "do":[{"use_virtual_model":"{{params.cheap}}"}]}]}"#;

    #[test]
    fn route_redirects_by_purpose_with_param() {
        let p = load(SIDE, json!({ "cheap": "haiku" }));
        let (d, log) = apply_route(&[p.clone()], &facts("title", "opus"), &json!({}));
        assert_eq!(d, RouteDecision::UseVirtualModel("haiku".into()));
        assert_eq!(log[0].rules, vec!["side"]);
        let (d, log) = apply_route(&[p], &facts("main", "opus"), &json!({}));
        assert_eq!(d, RouteDecision::Continue);
        assert!(log.is_empty());
    }

    #[test]
    fn missing_param_is_an_error_not_a_failure() {
        let p = load(SIDE, json!({}));
        let (d, log) = apply_route(&[p], &facts("title", "opus"), &json!({}));
        assert_eq!(d, RouteDecision::Continue, "出错不改道, 请求照常");
        assert!(log[0].error.as_deref().unwrap().contains("cheap"));
    }

    #[test]
    fn reject_in_route_and_request() {
        let p = load(r#"{"rulesApi":1,"rules":[{"id":"no-img","hook":"route","when":{"has_images":true},"do":[{"reject":{"message":"no images"}}]}]}"#, json!({}));
        let body = json!({ "messages": [{ "role": "user", "content": [{ "type": "image" }] }] });
        assert_eq!(apply_route(&[p], &facts("main", "opus"), &body).0, RouteDecision::Reject("no images".into()));
    }

    #[test]
    fn request_actions_rewrite_the_body() {
        let p = load(
            r#"{"rulesApi":1,"rules":[{"id":"r","hook":"request","when":{},"do":[
                {"system_append":{"text":"Be brief."}},
                {"set_param":{"path":"temperature","value":0.2}},
                {"clamp_param":{"path":"max_tokens","max":1000}},
                {"clamp_param":{"path":"thinking.budget_tokens","max":2000}},
                {"remove_tools":{"names":["WebSearch","mcp__*"]}},
                {"redact":{"preset":["api_keys","email"]}}]}]}"#,
            json!({}),
        );
        let mut body = json!({
            "system": [{ "type": "text", "text": "sys" }],
            "max_tokens": 32000,
            "thinking": { "type": "enabled", "budget_tokens": 10000 },
            "tools": [{ "name": "Bash" }, { "name": "WebSearch" }, { "name": "mcp__x" }],
            "tool_choice": { "type": "tool", "name": "WebSearch" },
            "messages": [
                { "role": "user", "content": "key sk-ant-abcdefghijklmnopqrstuvwxyz mail a@b.com" },
                { "role": "assistant", "content": [{ "type": "text", "text": "a@b.com stays" }] }
            ]
        });
        let (reject, log) = apply_request(&[p], &facts("main", "opus"), &mut body);
        assert!(reject.is_none());
        assert_eq!(log[0].rules, vec!["r"]);
        assert_eq!(body["system"][1]["text"], "Be brief.");
        assert_eq!(body["temperature"], 0.2);
        assert_eq!(body["max_tokens"], 1000);
        assert_eq!(body["thinking"]["budget_tokens"], 2000);
        assert_eq!(body["tools"], json!([{ "name": "Bash" }]));
        assert!(body.get("tool_choice").is_none());
        assert_eq!(body["messages"][0]["content"], "key [REDACTED] mail [REDACTED]");
        assert_eq!(body["messages"][1]["content"][0]["text"], "a@b.com stays", "user_text 不动 assistant");
    }

    #[test]
    fn budget_is_not_created_when_thinking_is_off() {
        let p = load(r#"{"rulesApi":1,"rules":[{"id":"r","hook":"request","do":[{"set_param":{"path":"thinking.budget_tokens","value":1000}}]}]}"#, json!({}));
        let mut body = json!({ "messages": [] });
        apply_request(&[p], &facts("main", "opus"), &mut body);
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn a_failing_plugin_leaves_the_body_untouched() {
        let p = load(
            r#"{"rulesApi":1,"params":[{"id":"t","type":"number","label":"T"}],"rules":[{"id":"r","hook":"request","do":[
                {"system_append":{"text":"x"}},{"set_param":{"path":"temperature","value":"{{params.t}}"}}]}]}"#,
            json!({}),
        );
        let mut body = json!({ "messages": [] });
        let (_, log) = apply_request(&[p], &facts("main", "opus"), &mut body);
        assert!(body.get("system").is_none(), "前一个动作的改动也被丢弃");
        assert!(log[0].error.is_some());
    }

    #[test]
    fn conditions_time_tokens_and_model_globs() {
        let w = parse(
            r#"{"rulesApi":1,"rules":[{"id":"r","hook":"route","when":{
                "time":[{"days":[3],"start":"22:00","end":"02:00"}],
                "model_requested":["claude-*"],"approx_input_tokens":{"gt":1}},
              "do":[{"use_virtual_model":"haiku"}]}]}"#,
        )
        .unwrap()
        .0
        .rules[0]
            .when
            .clone();
        let big = BodyFacts { approx_input_tokens: 10, ..Default::default() };
        let mut f = facts("main", "opus");
        f.model = "claude-sonnet-4-5";
        f.minute = 23 * 60;
        let none = Map::new();
        assert!(matches(&w, &f, &big, &none));
        f.weekday = 4;
        f.minute = 60; // 周四 01:00 属于周三 22:00 开始的窗口
        assert!(matches(&w, &f, &big, &none));
        f.minute = 3 * 60;
        assert!(!matches(&w, &f, &big, &none));
        f.weekday = 3;
        f.minute = 23 * 60;
        assert!(!matches(&w, &f, &BodyFacts::default(), &none), "token 下限");
        f.model = "gpt-5";
        assert!(!matches(&w, &f, &big, &none));
    }

    #[test]
    fn bool_param_switches_a_rule_on_and_off() {
        let src = r#"{"rulesApi":1,"params":[{"id":"on","type":"bool","label":"On","default":false}],
            "rules":[{"id":"r","hook":"request","when":{"param":"on"},"do":[{"system_append":{"text":"x"}}]}]}"#;
        let mut body = json!({ "messages": [] });
        apply_request(&[load(src, json!({ "on": false }))], &facts("main", "opus"), &mut body);
        assert!(body.get("system").is_none());
        apply_request(&[load(src, json!({ "on": true }))], &facts("main", "opus"), &mut body);
        assert_eq!(body["system"], "x");
        assert!(parse(&src.replace(r#""type":"bool""#, r#""type":"number""#).replace("false", "1")).is_err(), "只能引用布尔参数");
    }

    #[test]
    fn body_facts() {
        let b = BodyFacts::of(&json!({
            "stream": true, "tools": [{ "name": "x" }], "thinking": { "type": "enabled" },
            "system": "abcd", "messages": [{ "role": "user", "content": [{ "type": "text", "text": "efgh" }, { "type": "image" }] }]
        }));
        assert_eq!(b, BodyFacts { stream: true, has_tools: true, has_images: true, thinking: true, approx_input_tokens: 2 });
    }

    #[test]
    fn dry_run_distinguishes_matched_not_matched_and_unknown_body_conditions() {
        let src = r#"{
            "rulesApi": 1,
            "params": [{ "id": "cheap", "type": "virtual_model", "label": "Cheap", "default": "haiku" }],
            "rules": [
                { "id": "side", "hook": "route", "when": { "purpose": ["title"], "virtual_model": ["opus"] },
                  "do": [{ "use_virtual_model": "{{params.cheap}}" }] },
                { "id": "img-guard", "hook": "request", "when": { "virtual_model": ["haiku"], "has_images": true },
                  "do": [{ "reject": { "message": "no images" } }] },
                { "id": "long-req", "hook": "request", "when": { "approx_input_tokens": { "gt": 50000 } },
                  "do": [{ "system_append": { "text": "short" } }] }
            ]
        }"#;
        let p = load(src, json!({ "cheap": "haiku" }));
        let partial = PartialBodyFacts { stream: Some(true), has_tools: Some(false), ..Default::default() };

        let ev = dry_run_plugin(&p, &facts("title", "opus"), &partial);
        assert_eq!(ev.matched_rules, vec!["side"]);
        assert_eq!(ev.redirect_to.as_deref(), Some("haiku"));
        assert_eq!(ev.unknown_rules, vec!["img-guard", "long-req"]);
        assert_eq!(ev.unknown_fields, vec!["has_images", "approx_input_tokens"]);
        assert!(!ev.rejected);

        // 已知条件 (virtual_model = opus, 但 side 未命中) 不满足 → img-guard 是 NotMatched 而不是 Unknown
        let ev2 = dry_run_plugin(&p, &facts("main", "opus"), &partial);
        assert!(ev2.matched_rules.is_empty());
        assert_eq!(ev2.unknown_rules, vec!["long-req"]);
        assert_eq!(ev2.unknown_fields, vec!["approx_input_tokens"]);
    }
}
