//! Expression evaluator.
//!
//! Only a fixed allow-list of operations exists: there is no reflection, no class loading,
//! no file or network access. That is what makes it safe to render untrusted templates.

use super::parser::{BinOp, Expr, UnOp};
use crate::error::{Error, Result};
use crate::format::date::format_date;
use crate::format::{Locale, NumberPattern};
use crate::value::{JavaType, Value};
use chrono::NaiveDateTime;
use rust_decimal::{Decimal, RoundingStrategy};
use std::cmp::Ordering;

/// Where `$F{}`, `$P{}` and `$V{}` references are resolved.
pub trait Scope {
    fn field(&self, name: &str) -> Result<Value>;
    fn param(&self, name: &str) -> Result<Value>;
    fn variable(&self, name: &str) -> Result<Value>;
    /// A closure's parameter (`it`, `x`): none outside closures.
    fn local(&self, _name: &str) -> Option<Value> {
        None
    }
}

/// Evaluation settings that are fixed for a whole report run.
pub struct EvalEnv<'a> {
    pub locale: &'a Locale,
    /// Groovy semantics: integer division yields BigDecimal, `+` works on BigDecimal.
    pub groovy: bool,
    /// Value returned by `new Date()`, fixed per run so output is reproducible.
    pub now: NaiveDateTime,
}

/// How the message of a Java `NullPointerException` starts: a method called on null, null in
/// arithmetic or a comparison, null used as a boolean. JasperReports catches those around a
/// whole expression and gives null (`net.sf.jasperreports.evaluator.ignore.npe`, true by
/// default); the filler does the same with [`is_null_pointer`].
pub(crate) const NULL_POINTER: &str = "NullPointerException: ";

/// Whether `e` is what Java would throw as a `NullPointerException`.
pub(crate) fn is_null_pointer(e: &Error) -> bool {
    matches!(e, Error::Eval(m) if m.starts_with(NULL_POINTER))
}

pub fn eval(expr: &Expr, scope: &dyn Scope, env: &EvalEnv) -> Result<Value> {
    match expr {
        Expr::Lit(v) => Ok(v.clone()),
        Expr::Field(n) => scope.field(n),
        Expr::Param(n) => scope.param(n),
        Expr::Var(n) => scope.variable(n),
        Expr::Unary(op, e) => {
            let v = eval(e, scope, env)?;
            match op {
                UnOp::Not => match v {
                    Value::Bool(b) => Ok(Value::Bool(!b)),
                    Value::Null if env.groovy => Ok(Value::Bool(true)),
                    other => Err(Error::eval(format!("cannot apply ! to {}", other.type_name()))),
                },
                UnOp::Plus => Ok(v),
                UnOp::Neg => match v {
                    Value::Int(n) => Ok(Value::Int(n.wrapping_neg())),
                    Value::Long(n) => Ok(Value::Long(n.wrapping_neg())),
                    Value::Float(f) => Ok(Value::Float(-f)),
                    Value::Double(f) => Ok(Value::Double(-f)),
                    Value::Decimal(d) => Ok(Value::Decimal(-d)),
                    Value::Null => Err(Error::eval(format!("{NULL_POINTER}cannot negate null"))),
                    other => Err(Error::eval(format!("cannot negate {}", other.type_name()))),
                },
            }
        }
        Expr::And(a, b) => {
            if truthy(&eval(a, scope, env)?, env)? {
                Ok(Value::Bool(truthy(&eval(b, scope, env)?, env)?))
            } else {
                Ok(Value::Bool(false))
            }
        }
        Expr::Or(a, b) => {
            if truthy(&eval(a, scope, env)?, env)? {
                Ok(Value::Bool(true))
            } else {
                Ok(Value::Bool(truthy(&eval(b, scope, env)?, env)?))
            }
        }
        Expr::Cond(c, t, f) => {
            if truthy(&eval(c, scope, env)?, env)? {
                eval(t, scope, env)
            } else {
                eval(f, scope, env)
            }
        }
        Expr::Elvis(a, b) => {
            let v = eval(a, scope, env)?;
            if groovy_truth(&v) {
                Ok(v)
            } else {
                eval(b, scope, env)
            }
        }
        Expr::Binary(op, a, b) => {
            let l = eval(a, scope, env)?;
            let r = eval(b, scope, env)?;
            binary(*op, l, r, env)
        }
        Expr::Cast(ty, e) => {
            let v = eval(e, scope, env)?;
            Ok(match ty {
                JavaType::String | JavaType::Object | JavaType::Number | JavaType::Boolean | JavaType::Date => v,
                other => other.coerce(v),
            })
        }
        Expr::Method { target, name, args, safe, property } => {
            // `new SimpleDateFormat(p).format(x)` / `new DecimalFormat(p).format(x)`
            if let Expr::New { class, args: ctor } = target.as_ref() {
                if &**name == "format" && matches!(&**class, "SimpleDateFormat" | "DecimalFormat") {
                    let pattern = ctor.first().map(|e| eval(e, scope, env)).transpose()?.unwrap_or(Value::Null);
                    let value = args.first().map(|e| eval(e, scope, env)).transpose()?.unwrap_or(Value::Null);
                    return format_with(class, &pattern.to_java_string(), &value, env.locale);
                }
            }
            // `java.util.Base64.getDecoder().decode(s)`
            if let Expr::Static { class, name: getter, .. } = target.as_ref() {
                if &**class == "Base64" && &**getter == "getDecoder" && &**name == "decode" {
                    let v = args.first().map(|e| eval(e, scope, env)).transpose()?.unwrap_or(Value::Null);
                    return decode_bytes(v);
                }
            }
            // `x.getClass().getSimpleName()` / `.getName()`: the Java class of a value.
            if let Expr::Method { target: inner, name: get_class, args: none, .. } = target.as_ref() {
                if &**get_class == "getClass"
                    && none.is_empty()
                    && matches!(&**name, "getSimpleName" | "getName" | "getCanonicalName")
                {
                    let v = eval(inner, scope, env)?;
                    if v.is_null() {
                        return Err(Error::eval(format!("{NULL_POINTER}getClass() on null")));
                    }
                    let full = java_class(&v);
                    return Ok(Value::str(if &**name == "getSimpleName" {
                        full.rsplit('.').next().unwrap_or(full)
                    } else {
                        full
                    }));
                }
            }
            let recv = eval(target, scope, env)?;
            if *safe && recv.is_null() {
                return Ok(Value::Null);
            }
            if let Some(result) = super::groovy::method(&recv, name, args, *property, scope, env) {
                return result;
            }
            let argv = args.iter().map(|a| eval(a, scope, env)).collect::<Result<Vec<_>>>()?;
            match method(recv.clone(), name, argv, env) {
                // Groovy reads `date.time` with the getter, getTime().
                Err(e) if *property => {
                    let mut getter = String::from("get");
                    let mut chars = name.chars();
                    getter.extend(chars.next().map(|c| c.to_ascii_uppercase()));
                    getter.extend(chars);
                    method(recv, &getter, Vec::new(), env).map_err(|_| e)
                }
                other => other,
            }
        }
        Expr::List(items) => {
            Ok(Value::List(std::sync::Arc::new(items.iter().map(|e| eval(e, scope, env)).collect::<Result<Vec<_>>>()?)))
        }
        Expr::Map(entries) => Ok(Value::Map(std::sync::Arc::new(
            entries.iter().map(|(k, v)| Ok((eval(k, scope, env)?, eval(v, scope, env)?))).collect::<Result<Vec<_>>>()?,
        ))),
        Expr::Range(a, b, exclusive) => super::groovy::range(eval(a, scope, env)?, eval(b, scope, env)?, *exclusive),
        Expr::Closure { .. } => {
            Err(Error::eval("a closure can only be given to a method that runs it, such as findAll or collect"))
        }
        Expr::Local(name) => scope.local(name).ok_or_else(|| Error::eval(format!("unknown variable `{name}`"))),
        Expr::Index(target, index) => super::groovy::index(eval(target, scope, env)?, eval(index, scope, env)?),
        Expr::As(ty, e) => super::groovy::as_type(eval(e, scope, env)?, *ty, env),
        Expr::GString(parts) => {
            let mut out = String::new();
            for p in parts {
                out.push_str(&eval(p, scope, env)?.to_java_string());
            }
            Ok(Value::str(out))
        }
        Expr::Static { class, name, args } => {
            let argv = args.iter().map(|a| eval(a, scope, env)).collect::<Result<Vec<_>>>()?;
            match (&**class, &**name) {
                ("", "msg") => {
                    let pattern = argv.first().map(|v| v.to_java_string()).unwrap_or_default();
                    Ok(Value::str(crate::format::message::message_format(&pattern, argv.get(1..).unwrap_or(&[]), env.locale)))
                }
                ("", "str") => resource(&argv.first().map(|v| v.to_java_string()).unwrap_or_default(), scope),
                ("", f) => super::functions::call(f, argv, env),
                (c, f) if super::functions::is_function_class(c) => super::functions::call(f, argv, env),
                _ => static_call(class, name, argv, env),
            }
        }
        Expr::StaticField { class, name } => static_field(class, name),
        Expr::New { class, args } => {
            let argv = args.iter().map(|a| eval(a, scope, env)).collect::<Result<Vec<_>>>()?;
            construct(class, argv, env)
        }
    }
}

/// The Java class a value would have.
fn java_class(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "java.lang.Boolean",
        Value::Int(_) => "java.lang.Integer",
        Value::Long(_) => "java.lang.Long",
        Value::Float(_) => "java.lang.Float",
        Value::Double(_) => "java.lang.Double",
        Value::Decimal(_) => "java.math.BigDecimal",
        Value::Str(_) => "java.lang.String",
        Value::Date(_) => "java.util.Date",
        Value::Bytes(_) => "[B",
        Value::Json(_) | Value::List(_) => "java.util.ArrayList",
        Value::Map(_) => "java.util.LinkedHashMap",
        Value::DateRange(r) if r.start == r.end => "net.sf.jasperreports.types.date.FixedDate",
        Value::DateRange(_) => "net.sf.jasperreports.types.date.RelativeDateRange",
    }
}

fn decode_bytes(v: Value) -> Result<Value> {
    match v {
        Value::Str(s) => Ok(Value::Bytes(crate::image::decode_base64(&s)?.into())),
        Value::Bytes(b) => Ok(Value::Bytes(b)),
        Value::Null => Ok(Value::Null),
        other => Err(Error::eval(format!("cannot base64-decode {}", other.type_name()))),
    }
}

use super::groovy::groovy_truth;

/// Converts a condition value to a boolean (Java requires Boolean; Groovy is lenient).
pub fn truthy(v: &Value, env: &EvalEnv) -> Result<bool> {
    match v {
        Value::Bool(b) => Ok(*b),
        _ if env.groovy => Ok(groovy_truth(v)),
        Value::Null => Err(Error::eval(format!("{NULL_POINTER}null used as a boolean condition"))),
        other => Err(Error::eval(format!("{} used as a boolean condition", other.type_name()))),
    }
}

fn format_with(class: &str, pattern: &str, value: &Value, locale: &Locale) -> Result<Value> {
    let out = match (class, value) {
        ("SimpleDateFormat", Value::Date(d)) => {
            format_date(pattern, d, locale).ok_or_else(|| Error::eval(format!("unsupported date pattern `{pattern}`")))?
        }
        ("DecimalFormat", v) if v.is_numeric() => {
            NumberPattern::parse(pattern, locale).format(v, locale).ok_or_else(|| Error::eval("cannot format number"))?
        }
        (_, Value::Null) => return Err(Error::eval(format!("{class}.format(null)"))),
        (_, v) => return Err(Error::eval(format!("{class}.format() cannot format {}", v.type_name()))),
    };
    Ok(Value::str(out))
}

#[derive(PartialEq, PartialOrd, Clone, Copy)]
enum Rank {
    Int,
    Long,
    Float,
    Double,
    Decimal,
}

fn rank(v: &Value) -> Option<Rank> {
    Some(match v {
        Value::Int(_) => Rank::Int,
        Value::Long(_) => Rank::Long,
        Value::Float(_) => Rank::Float,
        Value::Double(_) => Rank::Double,
        Value::Decimal(_) => Rank::Decimal,
        _ => return None,
    })
}

fn arithmetic(op: BinOp, l: &Value, r: &Value, env: &EvalEnv) -> Result<Value> {
    let (Some(rl), Some(rr)) = (rank(l), rank(r)) else {
        return Err(Error::eval(format!("operator {op:?} not applicable to {} and {}", l.type_name(), r.type_name())));
    };
    let mut rk = if rl > rr { rl } else { rr };
    // Groovy: integer division produces BigDecimal.
    if env.groovy && op == BinOp::Div && rk <= Rank::Long {
        rk = Rank::Decimal;
    }
    let div0 = || Error::eval("ArithmeticException: / by zero");
    Ok(match rk {
        Rank::Int => {
            let (a, b) = (l.as_i64().unwrap_or(0) as i32, r.as_i64().unwrap_or(0) as i32);
            Value::Int(match op {
                BinOp::Add => a.wrapping_add(b),
                BinOp::Sub => a.wrapping_sub(b),
                BinOp::Mul => a.wrapping_mul(b),
                BinOp::Div => a.checked_div(b).ok_or_else(div0)?,
                BinOp::Rem => a.checked_rem(b).ok_or_else(div0)?,
                _ => unreachable!(),
            })
        }
        Rank::Long => {
            let (a, b) = (l.as_i64().unwrap_or(0), r.as_i64().unwrap_or(0));
            Value::Long(match op {
                BinOp::Add => a.wrapping_add(b),
                BinOp::Sub => a.wrapping_sub(b),
                BinOp::Mul => a.wrapping_mul(b),
                BinOp::Div => a.checked_div(b).ok_or_else(div0)?,
                BinOp::Rem => a.checked_rem(b).ok_or_else(div0)?,
                _ => unreachable!(),
            })
        }
        Rank::Float => {
            let (a, b) = (l.as_f64().unwrap_or(0.0) as f32, r.as_f64().unwrap_or(0.0) as f32);
            Value::Float(match op {
                BinOp::Add => a + b,
                BinOp::Sub => a - b,
                BinOp::Mul => a * b,
                BinOp::Div => a / b,
                BinOp::Rem => a % b,
                _ => unreachable!(),
            })
        }
        Rank::Double => {
            let (a, b) = (l.as_f64().unwrap_or(0.0), r.as_f64().unwrap_or(0.0));
            Value::Double(match op {
                BinOp::Add => a + b,
                BinOp::Sub => a - b,
                BinOp::Mul => a * b,
                BinOp::Div => a / b,
                BinOp::Rem => a % b,
                _ => unreachable!(),
            })
        }
        // Plain Java cannot apply operators to BigDecimal; Groovy can, and templates written for
        // either language rely on it, so it is always accepted.
        Rank::Decimal => {
            let (a, b) = (l.as_decimal().unwrap_or_default(), r.as_decimal().unwrap_or_default());
            let overflow = || Error::eval("ArithmeticException: the result has more than 28 digits");
            Value::Decimal(match op {
                BinOp::Add => a.checked_add(b).ok_or_else(overflow)?,
                BinOp::Sub => a.checked_sub(b).ok_or_else(overflow)?,
                BinOp::Mul => a.checked_mul(b).ok_or_else(overflow)?,
                BinOp::Div => {
                    if b.is_zero() {
                        return Err(div0());
                    }
                    groovy_divide(a, b).ok_or_else(overflow)?
                }
                BinOp::Rem => a.checked_rem(b).ok_or_else(div0)?,
                _ => unreachable!(),
            })
        }
    })
}

/// Groovy's BigDecimal division: an exact quotient keeps the scale BigDecimal.divide prefers
/// (1234.50 / 3 is 411.50, 6 / 2 is 3); one that does not end is rounded half up to 10
/// decimals.
fn groovy_divide(a: Decimal, b: Decimal) -> Option<Decimal> {
    let q = a.checked_div(b)?;
    if q.checked_mul(b) == Some(a) {
        let q = q.normalize();
        let preferred = a.scale() as i64 - b.scale() as i64;
        let scale = (q.scale() as i64).max(preferred).clamp(0, 28) as u32;
        let mut r = q;
        r.rescale(scale);
        return Some(r);
    }
    let mut r = q.round_dp_with_strategy(10, RoundingStrategy::MidpointAwayFromZero);
    r.rescale(10);
    Some(r)
}

pub(crate) fn binary(op: BinOp, l: Value, r: Value, env: &EvalEnv) -> Result<Value> {
    if let Some(result) = super::groovy::binary(op, &l, &r) {
        return result;
    }
    match op {
        BinOp::Pow | BinOp::Cmp | BinOp::In => Err(Error::eval(format!("operator {op:?} not applicable"))),
        BinOp::Add if matches!(l, Value::Str(_)) || matches!(r, Value::Str(_)) => {
            Ok(Value::str(format!("{}{}", l.to_java_string(), r.to_java_string())))
        }
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
            if l.is_null() || r.is_null() {
                return Err(Error::eval(format!("{NULL_POINTER}arithmetic on a null value")));
            }
            arithmetic(op, &l, &r, env)
        }
        BinOp::Eq => Ok(Value::Bool(l.java_equals(&r))),
        BinOp::Ne => Ok(Value::Bool(!l.java_equals(&r))),
        // Java unboxes both sides: null throws.
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge if !env.groovy && (l.is_null() || r.is_null()) => {
            let sym = match op {
                BinOp::Lt => "<",
                BinOp::Le => "<=",
                BinOp::Gt => ">",
                _ => ">=",
            };
            Err(Error::eval(format!("{NULL_POINTER}null compared with {sym}")))
        }
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let ord =
                l.compare(&r).ok_or_else(|| Error::eval(format!("cannot compare {} with {}", l.type_name(), r.type_name())))?;
            Ok(Value::Bool(match op {
                BinOp::Lt => ord == Ordering::Less,
                BinOp::Le => ord != Ordering::Greater,
                BinOp::Gt => ord == Ordering::Greater,
                _ => ord != Ordering::Less,
            }))
        }
    }
}

fn arg(args: &[Value], i: usize, what: &str) -> Result<Value> {
    args.get(i).cloned().ok_or_else(|| Error::eval(format!("{what}: missing argument {}", i + 1)))
}

fn int_arg(args: &[Value], i: usize, what: &str) -> Result<i64> {
    arg(args, i, what)?.as_i64().ok_or_else(|| Error::eval(format!("{what}: argument {} must be a number", i + 1)))
}

fn rounding(v: &Value) -> RoundingStrategy {
    let s = v.to_java_string();
    match s.rsplit('.').next().unwrap_or(&s) {
        "HALF_EVEN" | "6" => RoundingStrategy::MidpointNearestEven,
        "HALF_DOWN" | "5" => RoundingStrategy::MidpointTowardZero,
        "DOWN" | "1" => RoundingStrategy::ToZero,
        "UP" | "0" => RoundingStrategy::AwayFromZero,
        "FLOOR" | "3" => RoundingStrategy::ToNegativeInfinity,
        "CEILING" | "2" => RoundingStrategy::ToPositiveInfinity,
        _ => RoundingStrategy::MidpointAwayFromZero,
    }
}

fn method(recv: Value, name: &str, args: Vec<Value>, env: &EvalEnv) -> Result<Value> {
    if recv.is_null() {
        return Err(Error::eval(format!("{NULL_POINTER}cannot call {name}() on null")));
    }
    // Methods available on every object.
    match name {
        "toString" if args.is_empty() => return Ok(Value::str(recv.to_java_string())),
        "equals" => return Ok(Value::Bool(recv.java_equals(&arg(&args, 0, "equals")?))),
        "compareTo" => {
            let o = arg(&args, 0, "compareTo")?;
            let ord = recv.compare(&o).ok_or_else(|| Error::eval("compareTo on incompatible types"))?;
            return Ok(Value::Int(ord as i32));
        }
        _ => {}
    }
    match &recv {
        Value::Str(s) => string_method(s, name, &args),
        Value::DateRange(r) => match name {
            "getStart" => Ok(Value::Date(r.start)),
            "getEnd" => Ok(Value::Date(r.end)),
            _ => Err(Error::eval(format!("unsupported method DateRange.{name}()"))),
        },
        Value::Json(j) => json_method(j, name, &args),
        Value::Bool(b) if name == "booleanValue" => Ok(Value::Bool(*b)),
        Value::Date(d) => match name {
            "getTime" => Ok(Value::Long(d.and_utc().timestamp_millis())),
            // The getters of java.util.Date, with its conventions: years from 1900, months from 0.
            "getYear" => Ok(Value::Int(chrono::Datelike::year(d) - 1900)),
            "getMonth" => Ok(Value::Int(chrono::Datelike::month0(d) as i32)),
            "getDate" => Ok(Value::Int(chrono::Datelike::day(d) as i32)),
            "getDay" => Ok(Value::Int(chrono::Datelike::weekday(d).num_days_from_sunday() as i32)),
            "getHours" => Ok(Value::Int(chrono::Timelike::hour(d) as i32)),
            "getMinutes" => Ok(Value::Int(chrono::Timelike::minute(d) as i32)),
            "getSeconds" => Ok(Value::Int(chrono::Timelike::second(d) as i32)),
            "before" | "after" => {
                let o = arg(&args, 0, name)?;
                let Value::Date(o) = o else { return Err(Error::eval("Date comparison with non-date")) };
                Ok(Value::Bool(if name == "before" { d < &o } else { d > &o }))
            }
            _ => Err(Error::eval(format!("unsupported method Date.{name}()"))),
        },
        v if v.is_numeric() => number_method(v, name, &args, env),
        other => Err(Error::eval(format!("unsupported method {}.{name}()", other.type_name()))),
    }
}

/// Methods on nested JSON data: list size, and `subDataSource("path")` as on Jasper's
/// JSON data source.
fn json_method(j: &serde_json::Value, name: &str, args: &[Value]) -> Result<Value> {
    use serde_json::Value as J;
    Ok(match name {
        "size" => Value::Int(match j {
            J::Array(a) => a.len() as i32,
            J::Object(o) => o.len() as i32,
            _ => 0,
        }),
        "isEmpty" => Value::Bool(matches!(j, J::Array(a) if a.is_empty()) || j.is_null()),
        "subDataSource" => {
            let selected = match args.first() {
                Some(path) => {
                    let path = path.to_java_string();
                    path.split('.').filter(|p| !p.is_empty()).try_fold(j, |cur, key| cur.get(key)).cloned().unwrap_or(J::Null)
                }
                None => j.clone(),
            };
            let rows = match selected {
                J::Array(_) => selected,
                J::Null => J::Array(Vec::new()),
                other => J::Array(vec![other]),
            };
            Value::Json(std::sync::Arc::new(rows))
        }
        "get" => {
            let i = int_arg(args, 0, name)? as usize;
            match j.get(i) {
                Some(v) => Value::Json(std::sync::Arc::new(v.clone())),
                None => return Err(Error::eval("IndexOutOfBoundsException")),
            }
        }
        _ => return Err(Error::eval(format!("unsupported method List.{name}()"))),
    })
}

fn string_method(s: &str, name: &str, args: &[Value]) -> Result<Value> {
    let chars = || s.chars().collect::<Vec<_>>();
    let sarg = |i: usize| -> Result<String> { Ok(arg(args, i, name)?.to_java_string()) };
    Ok(match name {
        "length" => Value::Int(s.encode_utf16().count() as i32),
        "isEmpty" => Value::Bool(s.is_empty()),
        "isBlank" => Value::Bool(s.trim().is_empty()),
        "trim" | "strip" => Value::str(s.trim()),
        "toUpperCase" => Value::str(s.to_uppercase()),
        "toLowerCase" => Value::str(s.to_lowercase()),
        "contains" => Value::Bool(s.contains(sarg(0)?.as_str())),
        "startsWith" => Value::Bool(s.starts_with(sarg(0)?.as_str())),
        "endsWith" => Value::Bool(s.ends_with(sarg(0)?.as_str())),
        "equalsIgnoreCase" => Value::Bool(s.to_lowercase() == sarg(0)?.to_lowercase()),
        "indexOf" => {
            let needle = sarg(0)?;
            Value::Int(s.find(&needle).map(|b| s[..b].chars().count() as i32).unwrap_or(-1))
        }
        "lastIndexOf" => {
            let needle = sarg(0)?;
            Value::Int(s.rfind(&needle).map(|b| s[..b].chars().count() as i32).unwrap_or(-1))
        }
        "concat" => Value::str(format!("{s}{}", sarg(0)?)),
        "replace" | "replaceAll" => Value::str(s.replace(sarg(0)?.as_str(), sarg(1)?.as_str())),
        "charAt" => {
            let i = int_arg(args, 0, name)? as usize;
            let c = chars();
            Value::str(c.get(i).ok_or_else(|| Error::eval("StringIndexOutOfBoundsException"))?.to_string())
        }
        "substring" => {
            let c = chars();
            let start = int_arg(args, 0, name)? as usize;
            let end = if args.len() > 1 { int_arg(args, 1, name)? as usize } else { c.len() };
            if start > end || end > c.len() {
                return Err(Error::eval("StringIndexOutOfBoundsException in substring"));
            }
            Value::str(c[start..end].iter().collect::<String>())
        }
        _ => return Err(Error::eval(format!("unsupported method String.{name}()"))),
    })
}

fn number_method(v: &Value, name: &str, args: &[Value], env: &EvalEnv) -> Result<Value> {
    let overflow = || Error::eval("ArithmeticException: the result has more than 28 digits");
    let d = || v.as_decimal().ok_or_else(|| Error::eval("not a decimal"));
    let darg = |i: usize| -> Result<Decimal> {
        arg(args, i, name)?.as_decimal().ok_or_else(|| Error::eval(format!("{name}: argument must be numeric")))
    };
    Ok(match name {
        "intValue" | "shortValue" | "byteValue" => Value::Int(v.as_i64().unwrap_or(0) as i32),
        "longValue" => Value::Long(v.as_i64().unwrap_or(0)),
        "doubleValue" => Value::Double(v.as_f64().unwrap_or(0.0)),
        "floatValue" => Value::Float(v.as_f64().unwrap_or(0.0) as f32),
        "add" => Value::Decimal(d()?.checked_add(darg(0)?).ok_or_else(overflow)?),
        "subtract" => Value::Decimal(d()?.checked_sub(darg(0)?).ok_or_else(overflow)?),
        "multiply" => Value::Decimal(d()?.checked_mul(darg(0)?).ok_or_else(overflow)?),
        "negate" => Value::Decimal(-d()?),
        "abs" => Value::Decimal(d()?.abs()),
        "signum" => {
            let x = d()?;
            Value::Int(if x.is_zero() {
                0
            } else if x.is_sign_negative() {
                -1
            } else {
                1
            })
        }
        "max" => Value::Decimal(d()?.max(darg(0)?)),
        "min" => Value::Decimal(d()?.min(darg(0)?)),
        "divide" => {
            let a = d()?;
            let b = darg(0)?;
            if b.is_zero() {
                return Err(Error::eval("ArithmeticException: division by zero"));
            }
            let q = a.checked_div(b).ok_or_else(overflow)?;
            match args.len() {
                1 => Value::Decimal(q.normalize()),
                2 => Value::Decimal(q.round_dp_with_strategy(a.scale(), rounding(&args[1]))),
                _ => {
                    let scale = (int_arg(args, 1, name)?).clamp(0, 28) as u32;
                    Value::Decimal(q.round_dp_with_strategy(scale, rounding(&args[2])))
                }
            }
        }
        "setScale" => {
            let scale = int_arg(args, 0, name)?.clamp(0, 28) as u32;
            let strategy = args.get(1).map(rounding).unwrap_or(RoundingStrategy::ToZero);
            let mut r = d()?.round_dp_with_strategy(scale, strategy);
            r.rescale(scale);
            Value::Decimal(r)
        }
        "isNaN" => Value::Bool(v.as_f64().is_some_and(f64::is_nan)),
        "toPlainString" => Value::str(d()?.to_string()),
        "stripTrailingZeros" => Value::Decimal(d()?.normalize()),
        "scale" => Value::Int(d()?.scale() as i32),
        _ => {
            let _ = env;
            return Err(Error::eval(format!("unsupported method {}.{name}()", v.type_name())));
        }
    })
}

fn parse_number(s: &str, ty: JavaType) -> Result<Value> {
    let t = s.trim();
    let bad = || Error::eval(format!("NumberFormatException: For input string: \"{s}\""));
    Ok(match ty {
        JavaType::Integer => Value::Int(t.parse().map_err(|_| bad())?),
        JavaType::Long => Value::Long(t.parse().map_err(|_| bad())?),
        JavaType::Float => Value::Float(t.parse().map_err(|_| bad())?),
        JavaType::BigDecimal => Value::Decimal(t.parse().map_err(|_| bad())?),
        _ => Value::Double(t.parse().map_err(|_| bad())?),
    })
}

fn to_type(v: Value, ty: JavaType) -> Result<Value> {
    match v {
        Value::Str(s) => parse_number(&s, ty),
        Value::Null => Ok(Value::Null),
        other => Ok(ty.coerce(other)),
    }
}

/// Parameter that holds the texts of the report's resource bundle, as a JSON object.
pub const RESOURCE_BUNDLE: &str = "REPORT_RESOURCE_BUNDLE";
/// Key of that object that says what a missing text gives (`Null`, `Empty`, `Key`, `Error`).
pub const RESOURCE_MISSING: &str = "\u{0}missing";

/// The text of `key` in the report's resource bundle (`$R{key}`, `str("key")`).
fn resource(key: &str, scope: &dyn Scope) -> Result<Value> {
    let bundle = scope.param(RESOURCE_BUNDLE).unwrap_or_default();
    let texts = match &bundle {
        Value::Json(j) => j.as_object(),
        _ => None,
    };
    if let Some(text) = texts.and_then(|t| t.get(key)).and_then(|v| v.as_str()) {
        return Ok(Value::str(text));
    }
    match texts.and_then(|t| t.get(RESOURCE_MISSING)).and_then(|v| v.as_str()) {
        Some("Empty") => Ok(Value::str("")),
        Some("Key") => Ok(Value::str(key)),
        Some("Error") => Err(Error::eval(format!("no text for `{key}` in the report's resource bundle"))),
        _ => Ok(Value::Null),
    }
}

fn static_call(class: &str, name: &str, args: Vec<Value>, env: &EvalEnv) -> Result<Value> {
    let what = format!("{class}.{name}");
    let f = |i: usize| -> Result<f64> {
        arg(&args, i, &what)?.as_f64().ok_or_else(|| Error::eval(format!("{what}: numeric argument expected")))
    };
    Ok(match (class, name) {
        ("Math" | "StrictMath", "max" | "min") => {
            let (a, b) = (arg(&args, 0, &what)?, arg(&args, 1, &what)?);
            let ord = a.compare(&b).ok_or_else(|| Error::eval(format!("{what}: numeric arguments expected")))?;
            let pick_a = (name == "max") == (ord != Ordering::Less);
            let (pick, other) = if pick_a { (a, b) } else { (b, a) };
            // Result uses the wider of the two types, like Java overload resolution.
            match (rank(&pick), rank(&other)) {
                (Some(p), Some(o)) if o > p => match o {
                    Rank::Long => Value::Long(pick.as_i64().unwrap_or(0)),
                    Rank::Float => Value::Float(pick.as_f64().unwrap_or(0.0) as f32),
                    _ => Value::Double(pick.as_f64().unwrap_or(0.0)),
                },
                _ => pick,
            }
        }
        ("Math" | "StrictMath", "abs") => match arg(&args, 0, &what)? {
            Value::Int(n) => Value::Int(n.wrapping_abs()),
            Value::Long(n) => Value::Long(n.wrapping_abs()),
            Value::Float(x) => Value::Float(x.abs()),
            v => Value::Double(v.as_f64().unwrap_or(0.0).abs()),
        },
        ("Math" | "StrictMath", "round") => match arg(&args, 0, &what)? {
            Value::Float(x) => Value::Int((x + 0.5).floor() as i32),
            v => Value::Long((v.as_f64().unwrap_or(0.0) + 0.5).floor() as i64),
        },
        ("Math" | "StrictMath", "floor") => Value::Double(f(0)?.floor()),
        ("Math" | "StrictMath", "ceil") => Value::Double(f(0)?.ceil()),
        ("Math" | "StrictMath", "sqrt") => Value::Double(f(0)?.sqrt()),
        ("Math" | "StrictMath", "pow") => Value::Double(f(0)?.powf(f(1)?)),
        ("Math" | "StrictMath", "log") => Value::Double(f(0)?.ln()),
        ("Math" | "StrictMath", "log10") => Value::Double(f(0)?.log10()),
        ("Math" | "StrictMath", "exp") => Value::Double(f(0)?.exp()),
        ("Math" | "StrictMath", "cbrt") => Value::Double(f(0)?.cbrt()),
        ("Math" | "StrictMath", "sin") => Value::Double(f(0)?.sin()),
        ("Math" | "StrictMath", "cos") => Value::Double(f(0)?.cos()),
        ("Math" | "StrictMath", "tan") => Value::Double(f(0)?.tan()),
        ("Math" | "StrictMath", "asin") => Value::Double(f(0)?.asin()),
        ("Math" | "StrictMath", "acos") => Value::Double(f(0)?.acos()),
        ("Math" | "StrictMath", "atan") => Value::Double(f(0)?.atan()),
        ("Math" | "StrictMath", "atan2") => Value::Double(f(0)?.atan2(f(1)?)),
        ("Math" | "StrictMath", "hypot") => Value::Double(f(0)?.hypot(f(1)?)),
        ("Math" | "StrictMath", "toRadians") => Value::Double(f(0)?.to_radians()),
        ("Math" | "StrictMath", "toDegrees") => Value::Double(f(0)?.to_degrees()),
        ("Math" | "StrictMath", "signum") => Value::Double(f(0)?.signum() * if f(0)? == 0.0 { 0.0 } else { 1.0 }),
        ("String", "valueOf") => Value::str(arg(&args, 0, &what)?.to_java_string()),
        ("String", "format") => Value::str(java_string_format(&args, env)?),
        ("Integer", "valueOf" | "parseInt") => to_type(arg(&args, 0, &what)?, JavaType::Integer)?,
        ("Long", "valueOf" | "parseLong") => to_type(arg(&args, 0, &what)?, JavaType::Long)?,
        ("Double", "valueOf" | "parseDouble") => to_type(arg(&args, 0, &what)?, JavaType::Double)?,
        ("Float", "valueOf" | "parseFloat") => to_type(arg(&args, 0, &what)?, JavaType::Float)?,
        ("BigDecimal", "valueOf") => match arg(&args, 0, &what)? {
            Value::Double(x) => Value::Decimal(Value::Double(x).as_decimal().unwrap_or_default()),
            v => Value::Decimal(v.as_decimal().ok_or_else(|| Error::eval("BigDecimal.valueOf: number expected"))?),
        },
        ("Integer" | "Long" | "Double" | "Float", "toString") => Value::str(arg(&args, 0, &what)?.to_java_string()),
        ("Boolean", "valueOf" | "parseBoolean") => match arg(&args, 0, &what)? {
            Value::Bool(b) => Value::Bool(b),
            v => Value::Bool(v.to_java_string().eq_ignore_ascii_case("true")),
        },
        ("DatatypeConverter", "parseBase64Binary") => decode_bytes(arg(&args, 0, &what)?)?,
        ("Objects", "isNull") => Value::Bool(arg(&args, 0, &what)?.is_null()),
        ("Objects", "nonNull") => Value::Bool(!arg(&args, 0, &what)?.is_null()),
        ("Objects", "equals") => Value::Bool(arg(&args, 0, &what)?.java_equals(&arg(&args, 1, &what)?)),
        ("Objects", "toString") => {
            let v = arg(&args, 0, &what)?;
            if v.is_null() && args.len() > 1 {
                args[1].clone()
            } else {
                Value::str(v.to_java_string())
            }
        }
        _ => return Err(Error::eval(format!("unsupported static method {what}()"))),
    })
}

fn static_field(class: &str, name: &str) -> Result<Value> {
    Ok(match (class, name) {
        ("Boolean", "TRUE") => Value::Bool(true),
        ("Boolean", "FALSE") => Value::Bool(false),
        ("BigDecimal", "ZERO") => Value::Decimal(Decimal::ZERO),
        ("BigDecimal", "ONE") => Value::Decimal(Decimal::ONE),
        ("BigDecimal", "TEN") => Value::Decimal(Decimal::TEN),
        ("Integer", "MAX_VALUE") => Value::Int(i32::MAX),
        ("Integer", "MIN_VALUE") => Value::Int(i32::MIN),
        ("Long", "MAX_VALUE") => Value::Long(i64::MAX),
        ("Math", "PI") => Value::Double(std::f64::consts::PI),
        ("Math", "E") => Value::Double(std::f64::consts::E),
        ("RoundingMode", mode) => Value::str(format!("RoundingMode.{mode}")),
        ("BigDecimal", mode) if mode.starts_with("ROUND_") => Value::str(format!("RoundingMode.{}", &mode[6..])),
        _ => return Err(Error::eval(format!("unsupported static field {class}.{name}"))),
    })
}

fn construct(class: &str, args: Vec<Value>, env: &EvalEnv) -> Result<Value> {
    let what = format!("new {class}");
    Ok(match class {
        "Date" | "Timestamp" if args.is_empty() => Value::Date(env.now),
        "Date" | "Timestamp" => {
            let ms = int_arg(&args, 0, &what)?;
            Value::Date(chrono::DateTime::from_timestamp_millis(ms).ok_or_else(|| Error::eval("bad timestamp"))?.naive_utc())
        }
        "String" => Value::str(args.first().map(|v| v.to_java_string()).unwrap_or_default()),
        // Data sources for subreports wrap the rows they are given.
        "JRBeanCollectionDataSource"
        | "JRMapCollectionDataSource"
        | "JRBeanArrayDataSource"
        | "ListOfArrayDataSource"
        | "JRMapArrayDataSource" => match arg(&args, 0, &what)? {
            Value::Json(j) => Value::Json(j),
            Value::Null => Value::Json(std::sync::Arc::new(serde_json::Value::Array(Vec::new()))),
            other => return Err(Error::eval(format!("{what}: expected a list, got {}", other.type_name()))),
        },
        "JREmptyDataSource" => {
            let n = if args.is_empty() { 1 } else { int_arg(&args, 0, &what)?.max(0) as usize };
            let rows = vec![serde_json::Value::Object(Default::default()); n];
            Value::Json(std::sync::Arc::new(serde_json::Value::Array(rows)))
        }
        "ByteArrayInputStream" => match arg(&args, 0, &what)? {
            Value::Bytes(b) => Value::Bytes(b),
            other => return Err(Error::eval(format!("new ByteArrayInputStream({})", other.type_name()))),
        },
        "Integer" => to_type(arg(&args, 0, &what)?, JavaType::Integer)?,
        "Long" => to_type(arg(&args, 0, &what)?, JavaType::Long)?,
        "Double" => to_type(arg(&args, 0, &what)?, JavaType::Double)?,
        "Float" => to_type(arg(&args, 0, &what)?, JavaType::Float)?,
        "Boolean" => static_call("Boolean", "valueOf", args, env)?,
        "BigDecimal" => match arg(&args, 0, &what)? {
            Value::Str(s) => parse_number(&s, JavaType::BigDecimal)?,
            // new BigDecimal(double) uses the exact binary value.
            Value::Double(x) => Value::Decimal(Decimal::try_from(x).map_err(|e| Error::eval(e.to_string()))?),
            v => Value::Decimal(v.as_decimal().ok_or_else(|| Error::eval("new BigDecimal: number expected"))?),
        },
        // An empty list (a variable's initial value, which expressions may read), or a copy.
        "ArrayList" | "LinkedList" | "Vector" => match args.as_slice() {
            [] => Value::List(std::sync::Arc::new(Vec::new())),
            // `new ArrayList(int initialCapacity)`.
            [Value::Int(_) | Value::Long(_)] if class != "LinkedList" => Value::List(std::sync::Arc::new(Vec::new())),
            [list @ Value::List(_)] => list.clone(),
            [Value::Json(j)] if j.is_array() => Value::Json(j.clone()),
            _ => return Err(Error::eval(format!("{what}: expected no argument, a capacity or a collection"))),
        },
        _ => return Err(Error::eval(format!("unsupported constructor {what}()"))),
    })
}

/// Minimal `String.format` supporting %s, %d and %.Nf.
fn java_string_format(args: &[Value], env: &EvalEnv) -> Result<String> {
    let fmt = arg(args, 0, "String.format")?.to_java_string();
    let mut out = String::new();
    let mut next = 1;
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let mut spec = String::new();
        while let Some(&n) = chars.peek() {
            spec.push(n);
            chars.next();
            if n.is_ascii_alphabetic() || n == '%' {
                break;
            }
        }
        let conv = spec.pop().unwrap_or('s');
        if conv == '%' {
            out.push('%');
            continue;
        }
        if conv == 'n' {
            out.push('\n');
            continue;
        }
        let v = arg(args, next, "String.format")?;
        next += 1;
        match conv {
            'd' => out.push_str(&v.as_i64().unwrap_or(0).to_string()),
            'f' => {
                let prec = spec.split_once('.').and_then(|(_, p)| p.parse().ok()).unwrap_or(6usize);
                let pattern = if prec == 0 { "0".to_string() } else { format!("0.{}", "0".repeat(prec)) };
                let grouped = if spec.contains(',') { format!("#,##{pattern}") } else { pattern };
                out.push_str(&NumberPattern::parse(&grouped, env.locale).format(&v, env.locale).unwrap_or_default());
            }
            _ => out.push_str(&v.to_java_string()),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::super::parser::parse_expr;
    use super::*;
    use crate::format::locale::EN_US;
    use std::collections::HashMap;

    struct Map(HashMap<&'static str, Value>);
    impl Scope for Map {
        fn field(&self, n: &str) -> Result<Value> {
            self.0.get(n).cloned().ok_or_else(|| Error::eval(n))
        }
        fn param(&self, n: &str) -> Result<Value> {
            self.field(n)
        }
        fn variable(&self, n: &str) -> Result<Value> {
            self.field(n)
        }
    }

    fn run(src: &str) -> Value {
        let scope = Map(HashMap::from([
            ("PAGE_NUMBER", Value::Int(3)),
            ("name", Value::str("  Ana  ")),
            ("price", Value::Decimal("10.50".parse().unwrap())),
            ("qty", Value::Int(3)),
            ("d", Value::Date(chrono::NaiveDate::from_ymd_opt(2026, 1, 15).unwrap().and_hms_opt(0, 0, 0).unwrap())),
            ("nil", Value::Null),
        ]));
        let env = EvalEnv { locale: &EN_US, groovy: false, now: chrono::NaiveDateTime::default() };
        eval(&parse_expr(src, false).unwrap(), &scope, &env).unwrap()
    }

    #[test]
    fn dangerous_expressions_are_rejected() {
        // In JasperReports these run arbitrary code inside the JVM; here they simply fail.
        let scope = Map(HashMap::new());
        let env = EvalEnv { locale: &EN_US, groovy: true, now: chrono::NaiveDateTime::default() };
        for src in [
            "Runtime.getRuntime().exec(\"rm -rf /\")",
            "java.lang.Runtime.getRuntime().exec(\"id\")",
            "new java.io.File(\"/etc/passwd\").delete()",
            "Class.forName(\"java.lang.Runtime\")",
            "System.getProperty(\"user.home\")",
            "new java.net.URL(\"http://example.com\").openStream()",
            "new ProcessBuilder(\"sh\").start()",
        ] {
            let parsed = parse_expr(src, true);
            let result = parsed.map_err(Error::eval).and_then(|e| eval(&e, &scope, &env));
            assert!(result.is_err(), "{src} should be rejected");
        }
    }

    #[test]
    fn new_lists_are_empty() {
        // NoPageBreakReport (JasperReports' nopagebreak sample) starts a variable with one.
        assert_eq!(run("new java.util.ArrayList()").to_java_string(), "[]");
        assert_eq!(run("new ArrayList(10).size()").to_java_string(), "0");
        assert_eq!(run("new java.util.LinkedList().isEmpty()").to_java_string(), "true");
        assert_eq!(run("new java.util.Vector()").type_name(), "List");
    }

    #[test]
    fn java_semantics() {
        assert_eq!(run("\"Page \" + $V{PAGE_NUMBER}").to_java_string(), "Page 3");
        assert_eq!(run("7 / 2").to_java_string(), "3");
        assert_eq!(run("7 / 2.0").to_java_string(), "3.5");
        assert_eq!(run("1 + 2 + \"x\" + 1 + 2").to_java_string(), "3x12");
        assert_eq!(run("$F{name}.trim().toUpperCase()").to_java_string(), "ANA");
        assert_eq!(run("$F{nil} == null ? \"-\" : \"x\"").to_java_string(), "-");
        assert_eq!(run("$F{price}.multiply(new java.math.BigDecimal($F{qty}))").to_java_string(), "31.50");
        assert_eq!(
            run("$F{price}.multiply(new BigDecimal(\"1.18\")).setScale(2, java.math.RoundingMode.HALF_UP)").to_java_string(),
            "12.39"
        );
        assert_eq!(run("new java.text.SimpleDateFormat(\"dd/MM/yyyy\").format($F{d})").to_java_string(), "15/01/2026");
        assert_eq!(run("String.format(\"%s has %d items\", \"cart\", 3)").to_java_string(), "cart has 3 items");
        assert_eq!(run("Math.max($F{qty}, 10)").to_java_string(), "10");
        assert_eq!(run("((Integer) $V{PAGE_NUMBER}).intValue() % 2 == 1").to_java_string(), "true");
    }
}
