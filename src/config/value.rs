//! Reading settings out of a category (or any table of pastel.toml), which
//! are kept as JSON values.
use serde_json::Value;

/// Merge `b` into `a`: tables merge key by key, at any depth; any other
/// value in `b` replaces the one in `a`.
pub fn deep_merge(a: &mut Value, b: Value) {
    let (Value::Object(a), Value::Object(b)) = (a, b) else {
        return;
    };
    for (k, v) in b {
        match (a.get_mut(&k), v) {
            (Some(old @ Value::Object(_)), v @ Value::Object(_)) => deep_merge(old, v),
            (_, v) => {
                a.insert(k, v);
            }
        }
    }
}

/// A setting is "set" when present and not null, false, 0, "" or empty.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Numeric value of a setting (numbers or numeric strings), if present.
pub fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// `v[key]` as a number, or `default` when missing.
pub fn num(v: &Value, key: &str, default: f64) -> f64 {
    as_f64(&v[key]).unwrap_or(default)
}

/// `v[key]` as a string, when it is a non-empty string.
pub fn string<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v[key].as_str().filter(|s| !s.is_empty())
}
