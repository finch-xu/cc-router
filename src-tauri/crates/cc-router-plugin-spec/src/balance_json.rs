//! `balance_discovery.parser: json` 用的 JSON 路径子集。
//!
//! 语法: 以 `$` 开头, 后接任意个 `.字段` 或 `[下标]`; 字段名只含字母、数字、`_`、`-`。
//! 不支持通配、过滤、递归下降——余额接口的字段位置是固定的, 用不着。

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seg {
    Field(String),
    Index(usize),
}

pub fn parse_path(path: &str) -> Result<Vec<Seg>, String> {
    let rest = path.strip_prefix('$').ok_or_else(|| format!("path must start with $: {path}"))?;
    let mut segs = Vec::new();
    let b = rest.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'.' => {
                let start = i + 1;
                let mut j = start;
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] == b'-') {
                    j += 1;
                }
                if j == start {
                    return Err(format!("empty field name in {path}"));
                }
                segs.push(Seg::Field(rest[start..j].to_string()));
                i = j;
            }
            b'[' => {
                let close = rest[i..].find(']').ok_or_else(|| format!("unclosed [ in {path}"))? + i;
                let idx: usize = rest[i + 1..close].parse().map_err(|_| format!("bad index in {path}"))?;
                segs.push(Seg::Index(idx));
                i = close + 1;
            }
            _ => return Err(format!("unexpected character in {path}")),
        }
    }
    if segs.len() > 16 {
        return Err(format!("path too deep: {path}"));
    }
    Ok(segs)
}

pub fn eval<'a>(v: &'a Value, segs: &[Seg]) -> Option<&'a Value> {
    segs.iter().try_fold(v, |cur, s| match s {
        Seg::Field(k) => cur.get(k),
        Seg::Index(n) => cur.get(*n),
    })
}

/// JSON 数字或数字字符串。
pub fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|n: &f64| n.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn paths_parse_and_evaluate() {
        let v = json!({ "data": { "quota": "12.5", "list": [ { "n": 3 } ] } });
        assert_eq!(number(eval(&v, &parse_path("$.data.quota").unwrap()).unwrap()), Some(12.5));
        assert_eq!(number(eval(&v, &parse_path("$.data.list[0].n").unwrap()).unwrap()), Some(3.0));
        assert!(eval(&v, &parse_path("$.data.missing").unwrap()).is_none());
        assert_eq!(parse_path("$").unwrap(), vec![]);
    }

    #[test]
    fn bad_paths_are_rejected() {
        for p in ["data.quota", "$.", "$.a[", "$.a[x]", "$.a b", "$..a"] {
            assert!(parse_path(p).is_err(), "{p}");
        }
        assert_eq!(number(&json!("abc")), None);
        assert_eq!(number(&json!(true)), None);
    }
}
