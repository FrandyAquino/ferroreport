//! The part of Groovy that report expressions use: lists, maps and ranges, closures given to
//! `findAll`, `collect`, `sum` and the like, string helpers (`padLeft`, `tokenize`...), and the
//! operators Groovy adds (`**`, `<=>`, `in`, `list + list`, `text * 3`). Checked against
//! JasperReports 6.20.6 with its Groovy 4.0.8. Like the rest of the language, it can only
//! compute values: there is no way to reach classes, files or the network.

use super::eval::{eval, EvalEnv, Scope};
use super::parser::{BinOp, Expr};
use crate::error::{Error, Result};
use crate::value::{JavaType, Value};
use rust_decimal::Decimal;
use std::cmp::Ordering;
use std::sync::Arc;

/// Most items a list made by an expression may have (`(1..n)`, `"x" * n`, `list * n`).
const MAX_ITEMS: usize = 1_000_000;

/// Most closure calls one expression may make, nested ones included: a template cannot keep
/// a render busy with `(1..999999).collect { (1..999999).sum() }`.
const MAX_CALLS: u64 = 5_000_000;

thread_local! {
    /// Closure calls made by the expression being evaluated, and how deep in collection
    /// methods it is (the count starts over at the outermost one).
    static CALLS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Counts the collection methods running, so the budget belongs to the outermost.
struct Running;

impl Running {
    fn start() -> Running {
        DEPTH.with(|d| {
            if d.get() == 0 {
                CALLS.with(|c| c.set(0));
            }
            d.set(d.get() + 1);
        });
        Running
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// The scope inside a closure: its parameters, then whatever was visible outside it.
struct Frame<'a> {
    outer: &'a dyn Scope,
    names: &'a [Box<str>],
    values: Vec<Value>,
}

impl Scope for Frame<'_> {
    fn field(&self, name: &str) -> Result<Value> {
        self.outer.field(name)
    }
    fn param(&self, name: &str) -> Result<Value> {
        self.outer.param(name)
    }
    fn variable(&self, name: &str) -> Result<Value> {
        self.outer.variable(name)
    }
    fn local(&self, name: &str) -> Option<Value> {
        match self.names.iter().position(|n| &**n == name) {
            Some(i) => Some(self.values.get(i).cloned().unwrap_or_default()),
            None => self.outer.local(name),
        }
    }
}

/// A closure argument, ready to call.
struct Closure<'a> {
    params: &'a [Box<str>],
    body: &'a Expr,
    scope: &'a dyn Scope,
    env: &'a EvalEnv<'a>,
}

impl Closure<'_> {
    fn call(&self, args: Vec<Value>) -> Result<Value> {
        let spent = CALLS.with(|c| {
            c.set(c.get() + 1);
            c.get()
        });
        if spent > MAX_CALLS {
            return Err(fail(format!("the expression called its closures more than {MAX_CALLS} times")));
        }
        // A closure of one parameter given a map entry (key, value) gets the entry.
        // One taking several parameters given one list (`withIndex()` pairs) spreads it.
        let values = match args.as_slice() {
            [k, v] if self.params.len() == 1 => vec![entry(k, v)],
            [Value::List(parts)] if self.params.len() > 1 && parts.len() == self.params.len() => parts.as_ref().clone(),
            _ => args,
        };
        eval(self.body, &Frame { outer: self.scope, names: self.params, values }, self.env)
    }

    fn test(&self, args: Vec<Value>) -> Result<bool> {
        Ok(groovy_truth(&self.call(args)?))
    }
}

/// A map entry as a closure sees it: `it.key`, `it.value`.
fn entry(k: &Value, v: &Value) -> Value {
    Value::Map(Arc::new(vec![(Value::str("key"), k.clone()), (Value::str("value"), v.clone())]))
}

/// Groovy truth: null, false, 0, an empty text, list or map are false.
pub fn groovy_truth(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Str(s) => !s.is_empty(),
        Value::List(items) => !items.is_empty(),
        Value::Map(entries) => !entries.is_empty(),
        Value::Json(j) => match j.as_ref() {
            serde_json::Value::Array(a) => !a.is_empty(),
            serde_json::Value::Object(o) => !o.is_empty(),
            serde_json::Value::Null => false,
            _ => true,
        },
        v if v.is_numeric() => v.as_f64() != Some(0.0),
        _ => true,
    }
}

/// The items of a list-like value: a Groovy list, nested JSON rows, a map's entries.
fn items(v: &Value) -> Option<Vec<Value>> {
    match v {
        Value::List(items) => Some(items.as_ref().clone()),
        Value::Json(j) => match j.as_ref() {
            serde_json::Value::Array(a) => Some(a.iter().map(json).collect()),
            _ => None,
        },
        _ => None,
    }
}

fn json(j: &serde_json::Value) -> Value {
    crate::data::json_to_value(j, JavaType::Object).unwrap_or(Value::Null)
}

fn list(items: Vec<Value>) -> Value {
    Value::List(Arc::new(items))
}

fn fail(message: impl std::fmt::Display) -> Error {
    Error::eval(message.to_string())
}

/// Evaluates a method call that Groovy (and not Java) understands; `None` when it is not one,
/// so the ordinary methods are tried.
pub fn method(
    recv: &Value,
    name: &str,
    args: &[Expr],
    property: bool,
    scope: &dyn Scope,
    env: &EvalEnv,
) -> Option<Result<Value>> {
    // `map.key`, `json.name`: an entry.
    if property {
        match recv {
            Value::Map(entries) => {
                return Some(Ok(entries
                    .iter()
                    .find(|(k, _)| k.to_java_string() == name)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()))
            }
            Value::Json(j) if j.is_object() => return Some(Ok(j.get(name).map(json).unwrap_or_default())),
            _ => {}
        }
    }
    let closure = |i: usize| -> Option<Closure> {
        match args.get(i) {
            Some(Expr::Closure { params, body }) => Some(Closure { params, body, scope, env }),
            _ => None,
        }
    };
    // The arguments that are values (a closure stands in as null: the method runs it).
    let values = || -> Result<Vec<Value>> {
        args.iter()
            .map(|a| match a {
                Expr::Closure { .. } => Ok(Value::Null),
                other => eval(other, scope, env),
            })
            .collect()
    };
    let _running = Running::start();
    if let Value::Map(entries) = recv {
        return map_method(entries, name, closure(0), values).transpose();
    }
    if let Some(list) = items(recv) {
        return list_method(list, name, args, closure, values, env).transpose();
    }
    if let Value::Str(s) = recv {
        return string_method(s, name, values, env).transpose();
    }
    if recv.is_numeric() {
        return number_method(recv, name, values).transpose();
    }
    None
}

fn list_method<'a>(
    list: Vec<Value>,
    name: &str,
    args: &[Expr],
    closure: impl Fn(usize) -> Option<Closure<'a>>,
    values: impl Fn() -> Result<Vec<Value>>,
    env: &EvalEnv,
) -> Result<Option<Value>> {
    let with = closure(args.len().saturating_sub(1));
    let arg = |i: usize| -> Result<Value> { values().map(|v| v.get(i).cloned().unwrap_or_default()) };
    Ok(Some(match (name, &with) {
        ("size", None) => Value::Int(list.len() as i32),
        ("isEmpty", None) => Value::Bool(list.is_empty()),
        ("first", None) => list.first().cloned().ok_or_else(|| fail("NoSuchElementException: first() on an empty list"))?,
        ("last", None) => list.last().cloned().ok_or_else(|| fail("NoSuchElementException: last() on an empty list"))?,
        ("get" | "getAt", None) => at(&list, &arg(0)?)?,
        ("contains", None) => {
            let x = arg(0)?;
            Value::Bool(list.iter().any(|v| v.java_equals(&x)))
        }
        ("indexOf", None) => {
            let x = arg(0)?;
            Value::Int(list.iter().position(|v| v.java_equals(&x)).map(|i| i as i32).unwrap_or(-1))
        }
        ("join", None) => {
            let sep = if args.is_empty() { String::new() } else { arg(0)?.to_java_string() };
            Value::str(list.iter().map(Value::to_java_string).collect::<Vec<_>>().join(&sep))
        }
        ("reverse", None) => list.into_iter().rev().collect::<Vec<_>>().into_value(),
        ("toList" | "asList", None) => list.into_value(),
        ("withIndex", None) => {
            list.into_iter().enumerate().map(|(i, v)| list_value(vec![v, Value::Int(i as i32)])).collect::<Vec<_>>().into_value()
        }
        ("unique", None) => {
            let mut out: Vec<Value> = Vec::new();
            for v in list {
                if !out.iter().any(|o| o.java_equals(&v)) {
                    out.push(v);
                }
            }
            out.into_value()
        }
        ("flatten", None) => {
            fn flat(v: Value, out: &mut Vec<Value>) {
                match items(&v) {
                    Some(inner) => inner.into_iter().for_each(|i| flat(i, out)),
                    None => out.push(v),
                }
            }
            let mut out = Vec::new();
            list.into_iter().for_each(|v| flat(v, &mut out));
            out.into_value()
        }
        ("take" | "drop", None) => {
            let n = arg(0)?.as_i64().unwrap_or(0).clamp(0, list.len() as i64) as usize;
            if name == "take" { list[..n].to_vec() } else { list[n..].to_vec() }.into_value()
        }
        ("subList", None) => {
            let (a, b) = (arg(0)?.as_i64().unwrap_or(0), arg(1)?.as_i64().unwrap_or(0));
            if a < 0 || b < a || b as usize > list.len() {
                return Err(fail("IndexOutOfBoundsException in subList"));
            }
            list[a as usize..b as usize].to_vec().into_value()
        }
        ("plus", None) => plus_list(list, arg(0)?)?,
        ("minus", None) => minus_list(list, arg(0)?),
        ("sum", _) => {
            let mut total: Option<Value> =
                if args.len() > 1 || (args.len() == 1 && with.is_none()) { Some(arg(0)?) } else { None };
            for v in list {
                let v = match &with {
                    Some(c) => c.call(vec![v])?,
                    None => v,
                };
                total = Some(match total {
                    None => v,
                    Some(t) => super::eval::binary(BinOp::Add, t, v, env)?,
                });
            }
            total.unwrap_or_default()
        }
        ("max" | "min", _) => {
            let mut best: Option<(Value, Value)> = None;
            for v in list {
                let key = match &with {
                    Some(c) if c.params.len() < 2 => c.call(vec![v.clone()])?,
                    _ => v.clone(),
                };
                let better = match &best {
                    None => true,
                    Some((_, k)) => {
                        let ord = match &with {
                            Some(c) if c.params.len() == 2 => order(&c.call(vec![v.clone(), k.clone()])?),
                            _ => key.compare(k).unwrap_or(Ordering::Equal),
                        };
                        if name == "max" {
                            ord == Ordering::Greater
                        } else {
                            ord == Ordering::Less
                        }
                    }
                };
                if better {
                    best = Some((v, key));
                }
            }
            best.map(|(v, _)| v).unwrap_or_default()
        }
        ("sort", _) => {
            let mut keyed: Vec<(Value, Value)> = Vec::with_capacity(list.len());
            for v in list {
                let key = match &with {
                    Some(c) if c.params.len() < 2 => c.call(vec![v.clone()])?,
                    _ => v.clone(),
                };
                keyed.push((v, key));
            }
            let mut error = None;
            let keyed = merge_sort(keyed, &mut |a, b| match &with {
                Some(c) if c.params.len() == 2 => match c.call(vec![a.0.clone(), b.0.clone()]) {
                    Ok(r) => order(&r),
                    Err(e) => {
                        error.get_or_insert(e);
                        Ordering::Equal
                    }
                },
                _ => compare_nulls_first(&a.1, &b.1),
            });
            if let Some(e) = error {
                return Err(e);
            }
            keyed.into_iter().map(|(v, _)| v).collect::<Vec<_>>().into_value()
        }
        ("count", _) => {
            let mut n = 0;
            match &with {
                Some(c) => {
                    for v in list {
                        n += c.test(vec![v])? as i32;
                    }
                }
                None => {
                    let x = arg(0)?;
                    n = list.iter().filter(|v| v.java_equals(&x)).count() as i32;
                }
            }
            Value::Int(n)
        }
        ("inject", Some(c)) => {
            let mut acc = if args.len() > 1 { arg(0)? } else { list.first().cloned().unwrap_or_default() };
            let rest = if args.len() > 1 { &list[..] } else { list.get(1..).unwrap_or(&[]) };
            for v in rest {
                acc = c.call(vec![acc, v.clone()])?;
            }
            acc
        }
        ("findAll" | "grep", Some(c)) => {
            let mut out = Vec::new();
            for v in list {
                if c.test(vec![v.clone()])? {
                    out.push(v);
                }
            }
            out.into_value()
        }
        ("findAll", None) => list.into_iter().filter(groovy_truth).collect::<Vec<_>>().into_value(),
        ("find", Some(c)) => {
            for v in list {
                if c.test(vec![v.clone()])? {
                    return Ok(Some(v));
                }
            }
            Value::Null
        }
        ("findIndexOf", Some(c)) => {
            for (i, v) in list.into_iter().enumerate() {
                if c.test(vec![v])? {
                    return Ok(Some(Value::Int(i as i32)));
                }
            }
            Value::Int(-1)
        }
        ("any" | "every", Some(c)) => {
            let any = name == "any";
            for v in list {
                if c.test(vec![v])? == any {
                    return Ok(Some(Value::Bool(any)));
                }
            }
            Value::Bool(!any)
        }
        ("any", None) => Value::Bool(list.iter().any(groovy_truth)),
        ("every", None) => Value::Bool(list.iter().all(groovy_truth)),
        ("collect", Some(c)) => list.into_iter().map(|v| c.call(vec![v])).collect::<Result<Vec<_>>>()?.into_value(),
        ("collect", None) => list.into_value(),
        ("each", Some(c)) => {
            for v in &list {
                c.call(vec![v.clone()])?;
            }
            list.into_value()
        }
        ("eachWithIndex" | "collectWithIndex", Some(c)) => {
            let mut out = Vec::new();
            for (i, v) in list.iter().enumerate() {
                out.push(c.call(vec![v.clone(), Value::Int(i as i32)])?);
            }
            if name == "eachWithIndex" {
                list.into_value()
            } else {
                out.into_value()
            }
        }
        ("groupBy", Some(c)) => {
            let mut groups: Vec<(Value, Vec<Value>)> = Vec::new();
            for v in list {
                let key = c.call(vec![v.clone()])?;
                match groups.iter_mut().find(|(k, _)| k.java_equals(&key)) {
                    Some((_, members)) => members.push(v),
                    None => groups.push((key, vec![v])),
                }
            }
            Value::Map(Arc::new(groups.into_iter().map(|(k, v)| (k, list_value(v))).collect()))
        }
        ("collectEntries", Some(c)) => {
            let mut out = Vec::new();
            for v in list {
                match c.call(vec![v])? {
                    Value::List(pair) if pair.len() == 2 => out.push((pair[0].clone(), pair[1].clone())),
                    Value::Map(m) => out.extend(m.iter().cloned()),
                    other => return Err(fail(format!("collectEntries: {} is not a [key, value] pair", other.type_name()))),
                }
            }
            Value::Map(Arc::new(out))
        }
        _ => return Ok(None),
    }))
}

fn map_method<'a>(
    entries: &[(Value, Value)],
    name: &str,
    with: Option<Closure<'a>>,
    values: impl Fn() -> Result<Vec<Value>>,
) -> Result<Option<Value>> {
    let arg = |i: usize| -> Result<Value> { values().map(|v| v.get(i).cloned().unwrap_or_default()) };
    let get = |k: &Value| entries.iter().find(|(key, _)| key.java_equals(k)).map(|(_, v)| v.clone());
    Ok(Some(match (name, &with) {
        ("size", None) => Value::Int(entries.len() as i32),
        ("isEmpty", None) => Value::Bool(entries.is_empty()),
        ("get" | "getAt", None) => get(&arg(0)?).unwrap_or_default(),
        ("containsKey", None) => Value::Bool(get(&arg(0)?).is_some()),
        ("containsValue", None) => {
            let x = arg(0)?;
            Value::Bool(entries.iter().any(|(_, v)| v.java_equals(&x)))
        }
        ("keySet" | "keys", None) => list_value(entries.iter().map(|(k, _)| k.clone()).collect()),
        ("values", None) => list_value(entries.iter().map(|(_, v)| v.clone()).collect()),
        ("collect", Some(c)) => {
            list_value(entries.iter().map(|(k, v)| c.call(vec![k.clone(), v.clone()])).collect::<Result<Vec<_>>>()?)
        }
        ("each", Some(c)) => {
            for (k, v) in entries {
                c.call(vec![k.clone(), v.clone()])?;
            }
            Value::Map(Arc::new(entries.to_vec()))
        }
        ("findAll", Some(c)) => {
            let mut out = Vec::new();
            for (k, v) in entries {
                if c.test(vec![k.clone(), v.clone()])? {
                    out.push((k.clone(), v.clone()));
                }
            }
            Value::Map(Arc::new(out))
        }
        ("find", Some(c)) => {
            for (k, v) in entries {
                if c.test(vec![k.clone(), v.clone()])? {
                    return Ok(Some(entry(k, v)));
                }
            }
            Value::Null
        }
        ("any" | "every", Some(c)) => {
            let any = name == "any";
            for (k, v) in entries {
                if c.test(vec![k.clone(), v.clone()])? == any {
                    return Ok(Some(Value::Bool(any)));
                }
            }
            Value::Bool(!any)
        }
        _ => return Ok(None),
    }))
}

fn string_method(s: &str, name: &str, values: impl Fn() -> Result<Vec<Value>>, env: &EvalEnv) -> Result<Option<Value>> {
    let chars: Vec<char> = s.chars().collect();
    let arg = |i: usize| -> Result<Value> { values().map(|v| v.get(i).cloned().unwrap_or_default()) };
    let count = |i: usize| -> Result<usize> { Ok(arg(i)?.as_i64().unwrap_or(0).max(0) as usize) };
    let pad = |i: usize| -> Result<String> {
        let v = values()?;
        Ok(v.get(i).map(Value::to_java_string).unwrap_or_else(|| " ".into()))
    };
    let fill = |n: usize, with: &str| -> String { with.chars().cycle().take(n).collect() };
    Ok(Some(match name {
        "size" => Value::Int(s.encode_utf16().count() as i32),
        "reverse" => Value::str(chars.iter().rev().collect::<String>()),
        "capitalize" => Value::str(match chars.split_first() {
            Some((first, rest)) => first.to_uppercase().chain(rest.iter().copied()).collect::<String>(),
            None => String::new(),
        }),
        "uncapitalize" => Value::str(match chars.split_first() {
            Some((first, rest)) => first.to_lowercase().chain(rest.iter().copied()).collect::<String>(),
            None => String::new(),
        }),
        "padLeft" | "padRight" | "center" => {
            let width = count(0)?;
            if width > 64 << 20 {
                return Err(fail(format!("{name}: a width over 64 million characters")));
            }
            let with = pad(1)?;
            if width <= chars.len() || with.is_empty() {
                return Ok(Some(Value::str(s)));
            }
            let missing = width - chars.len();
            Value::str(match name {
                "padLeft" => format!("{}{s}", fill(missing, &with)),
                "padRight" => format!("{s}{}", fill(missing, &with)),
                _ => {
                    let left = missing / 2;
                    format!("{}{s}{}", fill(left, &with), fill(missing - left, &with))
                }
            })
        }
        "take" => Value::str(chars.iter().take(count(0)?).collect::<String>()),
        "drop" => Value::str(chars.iter().skip(count(0)?).collect::<String>()),
        "count" => {
            let needle = arg(0)?.to_java_string();
            Value::Int(if needle.is_empty() { chars.len() as i32 + 1 } else { s.matches(&needle).count() as i32 })
        }
        "tokenize" => {
            let delims = if values()?.is_empty() { " \t\n\r\u{c}".to_string() } else { arg(0)?.to_java_string() };
            list_value(s.split(|c| delims.contains(c)).filter(|t| !t.is_empty()).map(Value::str).collect())
        }
        "split" => {
            let parts = java_split(s, &arg(0)?.to_java_string())?;
            list_value(parts.into_iter().map(Value::str).collect())
        }
        "toList" | "chars" => list_value(chars.iter().map(|c| Value::str(c.to_string())).collect()),
        "getAt" => index(Value::str(s), arg(0)?)?,
        "plus" => Value::str(format!("{s}{}", arg(0)?.to_java_string())),
        "minus" => minus_text(s, &arg(0)?),
        "multiply" => repeat_text(s, &arg(0)?)?,
        "toInteger" | "toLong" | "toDouble" | "toFloat" | "toBigDecimal" | "toBigInteger" => {
            let ty = match name {
                "toInteger" => JavaType::Integer,
                "toLong" | "toBigInteger" => JavaType::Long,
                "toDouble" => JavaType::Double,
                "toFloat" => JavaType::Float,
                _ => JavaType::BigDecimal,
            };
            as_type(Value::str(s), ty, env)?
        }
        "isNumber" | "isBigDecimal" | "isDouble" | "isFloat" => {
            Value::Bool(s.trim().parse::<f64>().is_ok() && !s.trim().is_empty())
        }
        "isInteger" | "isLong" | "isBigInteger" => Value::Bool(s.trim().parse::<i64>().is_ok()),
        _ => return Ok(None),
    }))
}

fn number_method(v: &Value, name: &str, values: impl Fn() -> Result<Vec<Value>>) -> Result<Option<Value>> {
    let arg = |i: usize| -> Result<Value> { values().map(|v| v.get(i).cloned().unwrap_or_default()) };
    Ok(Some(match name {
        // Math.round: a Float to an Integer, anything else to a Long.
        "round" if values()?.is_empty() => match v {
            Value::Float(f) => Value::Int((f + 0.5).floor() as i32),
            Value::Int(_) | Value::Long(_) => v.clone(),
            other => Value::Long((other.as_f64().unwrap_or(0.0) + 0.5).floor() as i64),
        },
        "round" => {
            let places = arg(0)?.as_i64().unwrap_or(0).clamp(0, 28) as u32;
            match v {
                Value::Double(f) => {
                    let k = 10f64.powi(places as i32);
                    Value::Double((f * k).round() / k)
                }
                Value::Float(f) => {
                    let k = 10f32.powi(places as i32);
                    Value::Float((f * k).round() / k)
                }
                other => Value::Decimal(
                    other
                        .as_decimal()
                        .unwrap_or_default()
                        .round_dp_with_strategy(places, rust_decimal::RoundingStrategy::MidpointAwayFromZero),
                ),
            }
        }
        "intdiv" => {
            let (a, b) = (v.as_i64().unwrap_or(0), arg(0)?.as_i64().unwrap_or(0));
            if b == 0 {
                return Err(fail("ArithmeticException: / by zero"));
            }
            match (v, arg(0)?) {
                (Value::Int(_), Value::Int(_)) => Value::Int((a / b) as i32),
                _ => Value::Long(a / b),
            }
        }
        "toInteger" => Value::Int(v.as_i64().unwrap_or(0) as i32),
        "toLong" => Value::Long(v.as_i64().unwrap_or(0)),
        "toDouble" => Value::Double(v.as_f64().unwrap_or(0.0)),
        "toFloat" => Value::Float(v.as_f64().unwrap_or(0.0) as f32),
        "toBigDecimal" => Value::Decimal(v.as_decimal().unwrap_or_default()),
        "power" => power(v.clone(), arg(0)?)?,
        _ => return Ok(None),
    }))
}

/// `target[index]`: an item of a list (from the end when negative), several for a range, an
/// entry of a map, a character or a part of a text.
pub fn index(target: Value, index: Value) -> Result<Value> {
    match (&target, &index) {
        (Value::Map(entries), k) => {
            Ok(entries.iter().find(|(key, _)| key.java_equals(k)).map(|(_, v)| v.clone()).unwrap_or_default())
        }
        (Value::Json(j), Value::Str(k)) if j.is_object() => Ok(j.get(&**k).map(json).unwrap_or_default()),
        (Value::Str(s), _) => {
            let chars: Vec<char> = s.chars().collect();
            let texts: Vec<Value> = chars.iter().map(|c| Value::str(c.to_string())).collect();
            match at(&texts, &index)? {
                Value::List(picked) => Ok(Value::str(picked.iter().map(Value::to_java_string).collect::<String>())),
                one => Ok(one),
            }
        }
        (Value::Null, _) => Err(fail(format!("{}index of null", super::eval::NULL_POINTER))),
        (t, _) => match items(t) {
            Some(list) => at(&list, &index),
            None => Err(fail(format!("cannot index {}", t.type_name()))),
        },
    }
}

/// An item of a list by position (negative counts from the end), or the items at a list of
/// positions (a range).
fn at(list: &[Value], index: &Value) -> Result<Value> {
    let one = |i: i64| -> Result<Value> {
        let k = if i < 0 { list.len() as i64 + i } else { i };
        if k < 0 || k >= list.len() as i64 {
            return Err(fail(format!("IndexOutOfBoundsException: index {i} for length {}", list.len())));
        }
        Ok(list[k as usize].clone())
    };
    match index {
        Value::List(positions) => {
            Ok(list_value(positions.iter().map(|p| one(p.as_i64().unwrap_or(0))).collect::<Result<Vec<_>>>()?))
        }
        v => one(v.as_i64().ok_or_else(|| fail(format!("{} is not a position", v.type_name())))?),
    }
}

/// `a..b`: the whole numbers from `a` to `b` (down when `b` is smaller).
pub fn range(from: Value, to: Value, exclusive: bool) -> Result<Value> {
    let (Some(a), Some(b)) = (from.as_i64(), to.as_i64()) else {
        return Err(fail(format!("a range needs whole numbers, not {} and {}", from.type_name(), to.type_name())));
    };
    let len = a.abs_diff(b) + 1 - exclusive as u64;
    if len as usize > MAX_ITEMS {
        return Err(fail(format!("a range of more than {MAX_ITEMS} numbers")));
    }
    let step = if b >= a { 1 } else { -1 };
    Ok(list_value((0..len as i64).map(|i| Value::Int((a + i * step) as i32)).collect()))
}

/// The operators Groovy adds or changes: `**`, `<=>`, `in`, and `+`, `-`, `*` on lists, maps
/// and texts. `None` leaves the operation to the ordinary rules.
pub fn binary(op: BinOp, l: &Value, r: &Value) -> Option<Result<Value>> {
    Some(match (op, l, r) {
        (BinOp::Pow, a, b) => power(a.clone(), b.clone()),
        (BinOp::Cmp, a, b) => Ok(Value::Int(match compare_nulls_first(a, b) {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        })),
        (BinOp::In, x, container) => Ok(Value::Bool(match container {
            Value::Map(entries) => entries.iter().any(|(k, _)| k.java_equals(x)),
            Value::Str(s) => s.contains(&*x.to_java_string()),
            c => items(c).is_some_and(|list| list.iter().any(|v| v.java_equals(x))),
        })),
        (BinOp::Add, Value::Map(a), Value::Map(b)) => {
            let mut out = a.as_ref().clone();
            for (k, v) in b.iter() {
                match out.iter_mut().find(|(key, _)| key.java_equals(k)) {
                    Some(slot) => slot.1 = v.clone(),
                    None => out.push((k.clone(), v.clone())),
                }
            }
            Ok(Value::Map(Arc::new(out)))
        }
        (BinOp::Add, Value::List(a), b) => plus_list(a.as_ref().clone(), b.clone()),
        (BinOp::Sub, Value::List(a), b) => Ok(minus_list(a.as_ref().clone(), b.clone())),
        (BinOp::Sub, Value::Str(s), b) => Ok(minus_text(s, b)),
        (BinOp::Mul, Value::Str(s), n) if n.is_numeric() => repeat_text(s, n),
        (BinOp::Mul, Value::List(a), n) if n.is_numeric() => {
            let times = n.as_i64().unwrap_or(0).max(0) as usize;
            if a.len().saturating_mul(times) > MAX_ITEMS {
                return Some(Err(fail(format!("a list of more than {MAX_ITEMS} items"))));
            }
            Ok(list_value(a.iter().cloned().cycle().take(a.len() * times).collect()))
        }
        _ => return None,
    })
}

fn plus_list(mut list: Vec<Value>, other: Value) -> Result<Value> {
    match items(&other) {
        Some(more) => list.extend(more),
        None => list.push(other),
    }
    if list.len() > MAX_ITEMS {
        return Err(fail(format!("a list of more than {MAX_ITEMS} items")));
    }
    Ok(list_value(list))
}

fn minus_list(list: Vec<Value>, other: Value) -> Value {
    let remove = items(&other).unwrap_or_else(|| vec![other]);
    list_value(list.into_iter().filter(|v| !remove.iter().any(|r| r.java_equals(v))).collect())
}

/// `text - part`: the text without the first occurrence of the part.
fn minus_text(s: &str, part: &Value) -> Value {
    let part = part.to_java_string();
    Value::str(match s.find(&part) {
        Some(at) if !part.is_empty() => format!("{}{}", &s[..at], &s[at + part.len()..]),
        _ => s.to_string(),
    })
}

fn repeat_text(s: &str, n: &Value) -> Result<Value> {
    let times = n.as_i64().unwrap_or(0).max(0) as usize;
    if s.len().saturating_mul(times) > 64 << 20 {
        return Err(fail("the text would be larger than 64 MB"));
    }
    Ok(Value::str(s.repeat(times)))
}

/// `a ** b`: whole numbers stay whole (Integer while it fits, then Long), a BigDecimal raised
/// to a whole power stays a BigDecimal, anything else is a Double.
fn power(a: Value, b: Value) -> Result<Value> {
    if a.is_null() || b.is_null() {
        return Err(fail(format!("{}** on a null value", super::eval::NULL_POINTER)));
    }
    let whole_exponent = matches!(b, Value::Int(_) | Value::Long(_)) && b.as_i64().is_some_and(|e| e >= 0);
    match (&a, whole_exponent) {
        (Value::Int(_) | Value::Long(_), true) => {
            let (base, exp) = (a.as_i64().unwrap_or(0), b.as_i64().unwrap_or(0));
            match u32::try_from(exp).ok().and_then(|e| base.checked_pow(e)) {
                Some(p) if i32::try_from(p).is_ok() && matches!(a, Value::Int(_)) => Ok(Value::Int(p as i32)),
                Some(p) => Ok(Value::Long(p)),
                None => Ok(Value::Double((base as f64).powf(exp as f64))),
            }
        }
        (Value::Decimal(d), true) => {
            let exp = b.as_i64().unwrap_or(0);
            let mut out = Decimal::ONE;
            for _ in 0..exp.min(64) {
                out = out.checked_mul(*d).ok_or_else(|| fail("ArithmeticException: the power is too large"))?;
            }
            Ok(Value::Decimal(out))
        }
        _ => Ok(Value::Double(a.as_f64().unwrap_or(0.0).powf(b.as_f64().unwrap_or(0.0)))),
    }
}

/// `value as Type`.
pub fn as_type(v: Value, ty: JavaType, env: &EvalEnv) -> Result<Value> {
    let _ = env;
    Ok(match (ty, v) {
        (_, Value::Null) => Value::Null,
        (JavaType::String, v) => Value::str(v.to_java_string()),
        (JavaType::Boolean, v) => Value::Bool(groovy_truth(&v)),
        (JavaType::List, Value::Str(s)) => list_value(s.chars().map(|c| Value::str(c.to_string())).collect()),
        (JavaType::List, v) => list_value(items(&v).unwrap_or_else(|| vec![v])),
        (
            JavaType::Integer
            | JavaType::Long
            | JavaType::Short
            | JavaType::Byte
            | JavaType::Double
            | JavaType::Float
            | JavaType::BigDecimal
            | JavaType::BigInteger
            | JavaType::Number,
            Value::Str(s),
        ) => {
            let t = s.trim();
            let bad = || fail(format!("NumberFormatException: For input string: \"{s}\""));
            match ty {
                JavaType::Integer | JavaType::Short | JavaType::Byte => Value::Int(t.parse().map_err(|_| bad())?),
                JavaType::Long | JavaType::BigInteger => Value::Long(t.parse().map_err(|_| bad())?),
                JavaType::Double => Value::Double(t.parse().map_err(|_| bad())?),
                JavaType::Float => Value::Float(t.parse().map_err(|_| bad())?),
                _ => Value::Decimal(t.parse().map_err(|_| bad())?),
            }
        }
        (ty, v) => ty.coerce(v),
    })
}

/// `String.split(regex)`, for the patterns reports use: a literal text, an escaped character
/// (`\\.`, `\\|`) and white space (`\\s+`, `\\s*,\\s*`). Trailing empty parts are dropped.
fn java_split(s: &str, pattern: &str) -> Result<Vec<String>> {
    let meta = |c: char| ".$|()[]{}^?*+\\".contains(c);
    let mut parts: Vec<String> = if !pattern.chars().any(meta) {
        if pattern.is_empty() {
            s.chars().map(String::from).collect()
        } else {
            s.split(pattern).map(String::from).collect()
        }
    } else if pattern.len() == 2 && pattern.starts_with('\\') {
        s.split(&pattern[1..]).map(String::from).collect()
    } else if pattern == "\\s+" || pattern == " +" {
        let mut out: Vec<String> = s.split(char::is_whitespace).filter(|p| !p.is_empty()).map(String::from).collect();
        if s.starts_with(char::is_whitespace) {
            out.insert(0, String::new());
        }
        out
    } else if let Some(sep) = pattern.strip_prefix("\\s*").and_then(|p| p.strip_suffix("\\s*")).filter(|p| !p.chars().any(meta)) {
        s.split(sep).map(|p| p.trim().to_string()).collect()
    } else {
        return Err(fail(format!(
            "split(\"{pattern}\"): regular expressions other than a literal text or \\\\s+ are not supported"
        )));
    };
    while parts.len() > 1 && parts.last().is_some_and(String::is_empty) {
        parts.pop();
    }
    Ok(parts)
}

/// A stable merge sort that, unlike the standard one, accepts a comparison that is not a total
/// order (a template's closure can answer anything) without panicking.
pub(crate) fn merge_sort<T: Clone>(items: Vec<T>, compare: &mut dyn FnMut(&T, &T) -> Ordering) -> Vec<T> {
    if items.len() <= 1 {
        return items;
    }
    let mut right = items;
    let left: Vec<T> = right.drain(..right.len() / 2).collect();
    let (left, right) = (merge_sort(left, compare), merge_sort(right, compare));
    let mut out = Vec::with_capacity(left.len() + right.len());
    let (mut i, mut j) = (0, 0);
    while i < left.len() && j < right.len() {
        if compare(&right[j], &left[i]) == Ordering::Less {
            out.push(right[j].clone());
            j += 1;
        } else {
            out.push(left[i].clone());
            i += 1;
        }
    }
    out.extend_from_slice(&left[i..]);
    out.extend_from_slice(&right[j..]);
    out
}

/// A closure's answer as a comparator's: a number's sign.
fn order(v: &Value) -> Ordering {
    match v.as_f64() {
        Some(x) if x < 0.0 => Ordering::Less,
        Some(x) if x > 0.0 => Ordering::Greater,
        _ => Ordering::Equal,
    }
}

/// Groovy's ordering: null first, then the natural order.
fn compare_nulls_first(a: &Value, b: &Value) -> Ordering {
    match (a.is_null(), b.is_null()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => a.compare(b).unwrap_or(Ordering::Equal),
    }
}

fn list_value(items: Vec<Value>) -> Value {
    list(items)
}

trait IntoValue {
    fn into_value(self) -> Value;
}

impl IntoValue for Vec<Value> {
    fn into_value(self) -> Value {
        list(self)
    }
}
