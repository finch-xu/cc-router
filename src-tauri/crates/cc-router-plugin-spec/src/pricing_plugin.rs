//! 计价插件 (`type: "pricing"`) 的格式、校验与匹配 (设计 §5)。
//!
//! app 不带任何价格: 价格全部来自社区计价插件, 费用在**展示时**按当前价格表现算。
//!
//! ```json
//! { "pricingApi": 1, "currency": "USD", "per": 1000000,
//!   "prices": [ { "provider": "deepseek", "model": "deepseek-chat", "input": 0.27, "output": 1.1 } ] }
//! ```
//!
//! 匹配键是 (厂商 id, 真实模型名)。厂商 id 是公开承诺「只增不删」的: 内置 yaml 的 id、`custom-*` 来源
//! 标记、`plugin:<插件 id>`; `*` 匹配任意厂商。模型名可以以 `*` 结尾做前缀匹配。

use serde::Deserialize;
use serde_json::Value;

use crate::check::{Issue, IssueCode, PricingCapabilities};

pub const PRICING_API: u64 = 1;
pub const CURRENCIES: &[&str] = &["USD", "CNY"];
const MAX_RULES: usize = 5000;
const MAX_PRICE: f64 = 1_000_000.0;

/// 每 `per` 个 token 的价格。缓存读 / 写没写时按输入价算 (作者应当写全)。
#[derive(Debug, Clone, PartialEq)]
pub struct PriceRule {
    pub provider: String,
    pub model: String,
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PricingTable {
    pub currency: String,
    pub per: f64,
    pub prices: Vec<PriceRule>,
}

#[derive(Deserialize)]
struct RawRule {
    provider: Option<Value>,
    model: Option<Value>,
    input: Option<Value>,
    output: Option<Value>,
    cache_read: Option<Value>,
    cache_write: Option<Value>,
}

fn provider_ok(p: &str) -> bool {
    if p == "*" {
        return true;
    }
    let id = p.strip_prefix("plugin:").unwrap_or(p);
    !id.is_empty() && id.len() <= 60 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn model_ok(m: &str) -> bool {
    let body = m.strip_suffix('*').unwrap_or(m);
    !m.is_empty() && m.len() <= 200 && !m.chars().any(char::is_whitespace) && !body.contains('*')
}

pub fn parse(entry: &str) -> Result<(PricingTable, PricingCapabilities), Vec<Issue>> {
    let v: Value = serde_json::from_str(entry)
        .map_err(|e| vec![Issue::new("", IssueCode::InvalidValue).detail(format!("not valid JSON: {e}"))])?;
    let Some(obj) = v.as_object() else {
        return Err(vec![Issue::new("", IssueCode::InvalidType).detail("top level must be an object")]);
    };
    let mut issues = Vec::new();
    match obj.get("pricingApi").and_then(Value::as_u64) {
        Some(PRICING_API) => {}
        Some(n) if n > PRICING_API => issues.push(Issue::new("pricingApi", IssueCode::NeedsNewerApp).detail(n.to_string())),
        Some(_) => issues.push(Issue::new("pricingApi", IssueCode::UnsupportedVersion)),
        None => issues.push(Issue::new("pricingApi", IssueCode::Missing)),
    }
    let currency = obj.get("currency").and_then(Value::as_str).unwrap_or("");
    if !CURRENCIES.contains(&currency) {
        issues.push(Issue::new("currency", IssueCode::InvalidValue).detail("USD or CNY"));
    }
    let per = match obj.get("per") {
        None => 1_000_000.0,
        Some(p) => match p.as_f64() {
            Some(n) if n.is_finite() && n >= 1.0 => n,
            _ => {
                issues.push(Issue::new("per", IssueCode::InvalidValue).detail("a number >= 1"));
                1_000_000.0
            }
        },
    };
    let raw = match obj.get("prices").and_then(Value::as_array) {
        Some(a) if !a.is_empty() => a,
        Some(_) => {
            issues.push(Issue::new("prices", IssueCode::Missing).detail("at least one price"));
            return Err(issues);
        }
        None => {
            issues.push(Issue::new("prices", IssueCode::Missing));
            return Err(issues);
        }
    };
    if raw.len() > MAX_RULES {
        issues.push(Issue::new("prices", IssueCode::TooMany).detail(format!("max {MAX_RULES}")));
        return Err(issues);
    }

    let mut prices: Vec<PriceRule> = Vec::with_capacity(raw.len());
    for (i, item) in raw.iter().enumerate() {
        let at = |k: &str| format!("prices[{i}].{k}");
        let Ok(r) = serde_json::from_value::<RawRule>(item.clone()) else {
            issues.push(Issue::new(format!("prices[{i}]"), IssueCode::InvalidType));
            continue;
        };
        let provider = r.provider.as_ref().and_then(Value::as_str).unwrap_or("");
        if !provider_ok(provider) {
            issues.push(Issue::new(at("provider"), IssueCode::InvalidValue).detail("provider id, plugin:<id> or *"));
        }
        let model = r.model.as_ref().and_then(Value::as_str).unwrap_or("");
        if !model_ok(model) {
            issues.push(Issue::new(at("model"), IssueCode::InvalidValue).detail("model name; * only at the end"));
        }
        let mut price = |k: &str, v: &Option<Value>, required: bool| -> Option<f64> {
            match v {
                None if !required => None,
                None => {
                    issues.push(Issue::new(at(k), IssueCode::Missing));
                    None
                }
                Some(v) => match v.as_f64() {
                    Some(n) if n.is_finite() && (0.0..=MAX_PRICE).contains(&n) => Some(n),
                    _ => {
                        issues.push(Issue::new(at(k), IssueCode::InvalidValue).detail("a number between 0 and 1000000"));
                        None
                    }
                },
            }
        };
        let input = price("input", &r.input, true);
        let output = price("output", &r.output, true);
        let cache_read = price("cache_read", &r.cache_read, false);
        let cache_write = price("cache_write", &r.cache_write, false);
        if let (Some(input), Some(output)) = (input, output) {
            if prices.iter().any(|p| p.provider == provider && p.model == model) {
                issues.push(Issue::new(format!("prices[{i}]"), IssueCode::Duplicate).detail(format!("{provider} / {model}")));
                continue;
            }
            prices.push(PriceRule {
                provider: provider.to_string(),
                model: model.to_string(),
                input,
                output,
                cache_read: cache_read.unwrap_or(input),
                cache_write: cache_write.unwrap_or(input),
            });
        }
    }
    if !issues.is_empty() {
        return Err(issues);
    }
    let mut providers: Vec<String> = prices.iter().map(|p| p.provider.clone()).collect();
    providers.sort();
    providers.dedup();
    let caps = PricingCapabilities { currency: currency.to_string(), rules: prices.len(), providers };
    Ok((PricingTable { currency: currency.to_string(), per, prices }, caps))
}

impl PricingTable {
    /// 匹配顺序: 精确厂商 + 精确模型 > 精确厂商 + 最长前缀 > `*` + 精确模型 > `*` + 最长前缀。
    pub fn lookup(&self, provider: &str, model: &str) -> Option<&PriceRule> {
        let score = |r: &PriceRule| -> Option<(u8, usize)> {
            let provider_rank = if r.provider == provider {
                2
            } else if r.provider == "*" {
                0
            } else {
                return None;
            };
            if r.model == model {
                Some((provider_rank + 1, usize::MAX))
            } else if let Some(prefix) = r.model.strip_suffix('*') {
                model.starts_with(prefix).then_some((provider_rank, prefix.len()))
            } else {
                None
            }
        };
        self.prices.iter().filter_map(|r| score(r).map(|s| (s, r))).max_by_key(|(s, _)| *s).map(|(_, r)| r)
    }

    /// 按这条价格算费用, 单位是该币种的百万分之一 (micros)。`input` 是**不含缓存**的输入 token。
    pub fn cost_micros(&self, rule: &PriceRule, input: u64, output: u64, cache_read: u64, cache_write: u64) -> i64 {
        let units = input as f64 * rule.input
            + output as f64 * rule.output
            + cache_read as f64 * rule.cache_read
            + cache_write as f64 * rule.cache_write;
        (units / self.per * 1_000_000.0).round() as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(prices: &str) -> PricingTable {
        parse(&format!(r#"{{"pricingApi":1,"currency":"USD","prices":[{prices}]}}"#)).unwrap().0
    }

    #[test]
    fn lookup_prefers_exact_provider_then_exact_model_then_longest_prefix() {
        let t = table(
            r#"{"provider":"*","model":"claude-*","input":1,"output":1},
               {"provider":"*","model":"claude-sonnet-*","input":2,"output":2},
               {"provider":"*","model":"claude-sonnet-4-5","input":3,"output":3},
               {"provider":"openrouter","model":"claude-*","input":4,"output":4}"#,
        );
        assert_eq!(t.lookup("anthropic", "claude-sonnet-4-5").unwrap().input, 3.0);
        assert_eq!(t.lookup("anthropic", "claude-sonnet-4-6").unwrap().input, 2.0);
        assert_eq!(t.lookup("anthropic", "claude-haiku-4").unwrap().input, 1.0);
        assert_eq!(t.lookup("openrouter", "claude-sonnet-4-5").unwrap().input, 4.0, "精确厂商优先于 *");
        assert!(t.lookup("anthropic", "gpt-5").is_none());
    }

    #[test]
    fn cost_is_per_million_in_micros_and_cache_defaults_to_input() {
        let t = table(r#"{"provider":"deepseek","model":"deepseek-chat","input":0.27,"output":1.1,"cache_read":0.07}"#);
        let r = t.lookup("deepseek", "deepseek-chat").unwrap();
        assert_eq!(r.cache_write, 0.27);
        // 1M input + 1M output + 1M cache read + 0 cache write = 0.27 + 1.1 + 0.07 = $1.44
        assert_eq!(t.cost_micros(r, 1_000_000, 1_000_000, 1_000_000, 0), 1_440_000);
        assert_eq!(t.cost_micros(r, 1000, 0, 0, 0), 270);
    }

    fn codes(src: &str) -> Vec<IssueCode> {
        parse(src).unwrap_err().into_iter().map(|i| i.code).collect()
    }

    #[test]
    fn bad_tables_are_rejected() {
        assert_eq!(codes(r#"{"pricingApi":2,"currency":"USD","prices":[{"provider":"*","model":"a","input":1,"output":1}]}"#), vec![IssueCode::NeedsNewerApp]);
        assert_eq!(codes(r#"{"pricingApi":1,"currency":"EUR","prices":[{"provider":"*","model":"a","input":1,"output":1}]}"#), vec![IssueCode::InvalidValue]);
        assert_eq!(codes(r#"{"pricingApi":1,"currency":"USD","prices":[]}"#), vec![IssueCode::Missing]);
        assert_eq!(codes(r#"{"pricingApi":1,"currency":"USD","prices":[{"provider":"Bad Id","model":"a","input":1,"output":1}]}"#), vec![IssueCode::InvalidValue]);
        assert_eq!(codes(r#"{"pricingApi":1,"currency":"USD","prices":[{"provider":"*","model":"a*b","input":1,"output":1}]}"#), vec![IssueCode::InvalidValue]);
        assert_eq!(codes(r#"{"pricingApi":1,"currency":"USD","prices":[{"provider":"*","model":"a","input":-1,"output":1}]}"#), vec![IssueCode::InvalidValue]);
        assert_eq!(codes(r#"{"pricingApi":1,"currency":"USD","prices":[{"provider":"*","model":"a","output":1}]}"#), vec![IssueCode::Missing]);
        assert_eq!(
            codes(r#"{"pricingApi":1,"currency":"USD","prices":[{"provider":"*","model":"a","input":1,"output":1},{"provider":"*","model":"a","input":2,"output":2}]}"#),
            vec![IssueCode::Duplicate]
        );
        assert!(parse("not json").is_err());
        // 多字节结尾的模型名不 panic
        assert!(parse(r#"{"pricingApi":1,"currency":"USD","prices":[{"provider":"*","model":"模型","input":1,"output":1}]}"#).is_ok());
    }

    #[test]
    fn capabilities_list_currency_rules_and_providers() {
        let (_, caps) = parse(
            r#"{"pricingApi":1,"currency":"CNY","per":1000,"prices":[
                {"provider":"plugin:relay","model":"x","input":1,"output":1},
                {"provider":"deepseek","model":"y*","input":1,"output":1},
                {"provider":"deepseek","model":"z","input":1,"output":1}]}"#,
        )
        .unwrap();
        assert_eq!(caps.currency, "CNY");
        assert_eq!(caps.rules, 3);
        assert_eq!(caps.providers, vec!["deepseek", "plugin:relay"]);
    }
}
