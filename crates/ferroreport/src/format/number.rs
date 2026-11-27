//! `java.text.DecimalFormat` pattern formatting.
//!
//! Supports the subset used by report templates: digit placeholders (`#`, `0`), grouping,
//! literal prefixes/suffixes (quoted or plain), `%`, `‰`, `¤` and an explicit negative
//! subpattern. Rounding is HALF_EVEN on the exact value, like Java 8+.

use super::locale::Locale;
use crate::value::Value;

#[derive(Debug, Clone, PartialEq)]
struct Affix {
    text: String,
    percent: bool,
    permille: bool,
    /// It has a currency sign (`¤`): the number takes the monetary decimal separator.
    currency: bool,
}

/// A compiled `DecimalFormat` pattern.
#[derive(Debug, Clone)]
pub struct NumberPattern {
    pos_prefix: Affix,
    pos_suffix: Affix,
    neg_prefix: Option<Affix>,
    neg_suffix: Option<Affix>,
    min_int: usize,
    min_frac: usize,
    max_frac: usize,
    grouping: usize,
    decimal_always_shown: bool,
}

fn parse_affix(s: &str, locale: &Locale) -> Affix {
    let mut out = Affix { text: String::new(), percent: false, permille: false, currency: false };
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                if chars.peek() == Some(&'\'') {
                    chars.next();
                    out.text.push('\'');
                    continue;
                }
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    out.text.push(q);
                }
            }
            '%' => {
                out.percent = true;
                out.text.push('%');
            }
            '\u{2030}' => {
                out.permille = true;
                out.text.push('\u{2030}');
            }
            '\u{00A4}' => {
                out.currency = true;
                out.text.push_str(locale.currency_symbol);
            }
            _ => out.text.push(c),
        }
    }
    out
}

/// Splits one subpattern into (prefix, number part, suffix), honouring quotes.
fn split_subpattern(p: &str) -> (String, String, String) {
    let mut prefix = String::new();
    let mut number = String::new();
    let mut suffix = String::new();
    let mut state = 0; // 0 prefix, 1 number, 2 suffix
    let mut in_quote = false;
    for c in p.chars() {
        if c == '\'' {
            in_quote = !in_quote;
        }
        let is_num = !in_quote && matches!(c, '#' | '0'..='9' | ',' | '.');
        match state {
            0 if is_num => {
                state = 1;
                number.push(c);
            }
            0 => prefix.push(c),
            1 if is_num => number.push(c),
            1 => {
                state = 2;
                suffix.push(c);
            }
            _ => suffix.push(c),
        }
    }
    (prefix, number, suffix)
}

impl NumberPattern {
    pub fn parse(pattern: &str, locale: &Locale) -> NumberPattern {
        let mut parts = Vec::new();
        let mut in_quote = false;
        let mut start = 0;
        for (i, c) in pattern.char_indices() {
            if c == '\'' {
                in_quote = !in_quote;
            } else if c == ';' && !in_quote {
                parts.push(&pattern[start..i]);
                start = i + 1;
            }
        }
        parts.push(&pattern[start..]);

        let (prefix, number, suffix) = split_subpattern(parts[0]);
        let (int_part, frac_part) = match number.split_once('.') {
            Some((a, b)) => (a.to_string(), Some(b.to_string())),
            None => (number.clone(), None),
        };
        let min_int = int_part.chars().filter(|c| c.is_ascii_digit()).count();
        let grouping = match int_part.rfind(',') {
            Some(i) => int_part[i + 1..].chars().filter(|c| *c == '#' || c.is_ascii_digit()).count(),
            None => 0,
        };
        let (min_frac, max_frac) = match &frac_part {
            Some(f) => {
                (f.chars().filter(|c| c.is_ascii_digit()).count(), f.chars().filter(|c| *c == '#' || c.is_ascii_digit()).count())
            }
            None => (0, 0),
        };
        let (neg_prefix, neg_suffix) = match parts.get(1) {
            Some(neg) => {
                let (np, _, ns) = split_subpattern(neg);
                (Some(parse_affix(&np, locale)), Some(parse_affix(&ns, locale)))
            }
            None => (None, None),
        };
        NumberPattern {
            pos_prefix: parse_affix(&prefix, locale),
            pos_suffix: parse_affix(&suffix, locale),
            neg_prefix,
            neg_suffix,
            // DecimalFormat("#.##") prints "0" for zero, i.e. at least one integer digit
            // unless there are fraction digits that can carry the value.
            min_int: if min_int == 0 && max_frac == 0 { 1 } else { min_int },
            min_frac,
            max_frac,
            grouping,
            decimal_always_shown: frac_part.as_deref() == Some(""),
        }
    }

    /// Formats a numeric value. Returns `None` for non-numeric values.
    pub fn format(&self, value: &Value, locale: &Locale) -> Option<String> {
        let (negative, mut int_digits, mut frac_digits) = exact_decimal(value)?;
        let scale = if self.pos_prefix.percent || self.pos_suffix.percent {
            2
        } else if self.pos_prefix.permille || self.pos_suffix.permille {
            3
        } else {
            0
        };
        for _ in 0..scale {
            let c = if frac_digits.is_empty() { '0' } else { frac_digits.remove(0) };
            int_digits.push(c);
        }
        let (int_digits, frac_digits) = round_half_even(&int_digits, &frac_digits, self.max_frac);
        let int_trimmed = int_digits.trim_start_matches('0');
        let mut frac = frac_digits;
        while frac.len() > self.min_frac && frac.ends_with('0') {
            frac.pop();
        }
        let is_zero = int_trimmed.is_empty() && frac.chars().all(|c| c == '0');
        let mut int_str = int_trimmed.to_string();
        while int_str.len() < self.min_int {
            int_str.insert(0, '0');
        }
        if int_str.is_empty() && frac.is_empty() {
            int_str.push('0');
        }
        // Java's DecimalFormat takes the monetary separators when the pattern has a `¤` anywhere.
        let currency = [Some(&self.pos_prefix), Some(&self.pos_suffix), self.neg_prefix.as_ref(), self.neg_suffix.as_ref()]
            .into_iter()
            .any(|a| a.is_some_and(|a| a.currency));
        let grouping = if currency { locale.monetary_grouping_separator() } else { locale.grouping_separator };
        let mut body = String::new();
        if self.grouping > 0 && int_str.len() > self.grouping {
            let first = int_str.len() % self.grouping;
            for (i, c) in int_str.chars().enumerate() {
                if i > 0 && (i + self.grouping - first) % self.grouping == 0 {
                    body.push(grouping);
                }
                body.push(c);
            }
        } else {
            body.push_str(&int_str);
        }
        if !frac.is_empty() || self.decimal_always_shown {
            body.push(if currency { locale.monetary_decimal_separator() } else { locale.decimal_separator });
            body.push_str(&frac);
        }
        let negative = negative && !is_zero;
        Some(if negative {
            match (&self.neg_prefix, &self.neg_suffix) {
                (Some(p), Some(s)) => format!("{}{}{}", p.text, body, s.text),
                _ => format!("-{}{}{}", self.pos_prefix.text, body, self.pos_suffix.text),
            }
        } else {
            format!("{}{}{}", self.pos_prefix.text, body, self.pos_suffix.text)
        })
    }
}

/// Exact decimal expansion of a numeric value as (negative, integer digits, fraction digits).
fn exact_decimal(value: &Value) -> Option<(bool, String, String)> {
    let s = match value {
        Value::Int(n) => n.to_string(),
        Value::Long(n) => n.to_string(),
        Value::Decimal(d) => d.to_string(),
        Value::Float(f) => format!("{:.200}", *f as f64),
        Value::Double(f) if f.is_finite() => format!("{:.400}", f),
        _ => return None,
    };
    let negative = s.starts_with('-');
    let s = s.trim_start_matches('-');
    let (i, f) = match s.split_once('.') {
        Some((i, f)) => (i.to_string(), f.trim_end_matches('0').to_string()),
        None => (s.to_string(), String::new()),
    };
    Some((negative, i, f))
}

/// Rounds `int.frac` to `max_frac` fraction digits with HALF_EVEN.
fn round_half_even(int: &str, frac: &str, max_frac: usize) -> (String, String) {
    if frac.len() <= max_frac {
        let mut f = frac.to_string();
        while f.len() < max_frac {
            f.push('0');
        }
        return (int.to_string(), f);
    }
    let kept = &frac[..max_frac];
    let rest = &frac[max_frac..];
    let first = rest.as_bytes()[0];
    let tail_nonzero = rest[1..].bytes().any(|b| b != b'0');
    let last_kept_digit = if max_frac > 0 { kept.as_bytes()[max_frac - 1] } else { *int.as_bytes().last().unwrap_or(&b'0') };
    let round_up = first > b'5' || (first == b'5' && (tail_nonzero || (last_kept_digit - b'0') % 2 == 1));
    let mut digits: Vec<u8> = int.bytes().chain(kept.bytes()).collect();
    if round_up {
        let mut i = digits.len();
        loop {
            if i == 0 {
                digits.insert(0, b'1');
                break;
            }
            i -= 1;
            if digits[i] == b'9' {
                digits[i] = b'0';
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    let split = digits.len() - max_frac;
    let s = String::from_utf8(digits).unwrap_or_default();
    (s[..split].to_string(), s[split..].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::locale::{EN_US, ES_ES};

    fn fmt(p: &str, v: Value) -> String {
        NumberPattern::parse(p, &EN_US).format(&v, &EN_US).unwrap()
    }

    #[test]
    fn common_patterns() {
        assert_eq!(fmt("#,##0.00", Value::Double(1234567.891)), "1,234,567.89");
        assert_eq!(fmt("#,##0.00", Value::Double(0.5)), "0.50");
        assert_eq!(fmt("#,##0", Value::Int(1000)), "1,000");
        assert_eq!(fmt("0.00", Value::Double(-1.23456)), "-1.23");
        assert_eq!(fmt("$ #,##0.00", Value::Double(12.5)), "$ 12.50");
        assert_eq!(fmt("#,##0.00;(#,##0.00)", Value::Double(-12.5)), "(12.50)");
        assert_eq!(fmt("0.0%", Value::Double(0.256)), "25.6%");
        assert_eq!(fmt("#.##", Value::Double(0.0)), "0");
        assert_eq!(fmt("#,##0.###", Value::Double(2.5)), "2.5");
        assert_eq!(fmt("000", Value::Int(7)), "007");
    }

    #[test]
    fn half_even_rounding() {
        assert_eq!(fmt("0.00", Value::Double(0.125)), "0.12");
        assert_eq!(fmt("0.00", Value::Double(0.135)), "0.14");
        assert_eq!(fmt("0", Value::Double(2.5)), "2");
        assert_eq!(fmt("0", Value::Double(3.5)), "4");
        assert_eq!(fmt("0.00", Value::Double(1.005)), "1.00");
        // 999.995 is 999.99500000000000454... in binary, so it rounds up (verified against Java 21).
        assert_eq!(fmt("#,##0.00", Value::Double(999.995)), "1,000.00");
    }

    #[test]
    fn locale_symbols() {
        let p = NumberPattern::parse("#,##0.00", &ES_ES);
        assert_eq!(p.format(&Value::Double(1234.5), &ES_ES).unwrap(), "1.234,50");
    }
}
