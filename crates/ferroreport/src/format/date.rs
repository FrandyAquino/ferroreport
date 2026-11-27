//! `java.text.SimpleDateFormat` pattern formatting.

use super::locale::Locale;
use chrono::{Datelike, NaiveDateTime, Timelike};

/// Formats `dt` with a SimpleDateFormat pattern. Unknown letters are rejected with `None`.
pub fn format_date(pattern: &str, dt: &NaiveDateTime, locale: &Locale) -> Option<String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            if chars.get(i + 1) == Some(&'\'') {
                out.push('\'');
                i += 2;
                continue;
            }
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        out.push('\'');
                        i += 2;
                        continue;
                    }
                    break;
                }
                out.push(chars[i]);
                i += 1;
            }
            i += 1;
            continue;
        }
        if !c.is_ascii_alphabetic() {
            out.push(c);
            i += 1;
            continue;
        }
        let mut n = 1;
        while i + n < chars.len() && chars[i + n] == c {
            n += 1;
        }
        i += n;
        let pad = |v: i64, w: usize| format!("{:0w$}", v, w = w);
        match c {
            'y' | 'Y' => {
                let y = dt.year() as i64;
                out.push_str(&if n == 2 { pad(y.rem_euclid(100), 2) } else { pad(y, n) });
            }
            'M' | 'L' => {
                let m = dt.month0() as usize;
                out.push_str(&match n {
                    1 | 2 => pad(m as i64 + 1, n),
                    3 => locale.short_months[m].to_string(),
                    _ => locale.months[m].to_string(),
                });
            }
            'd' => out.push_str(&pad(dt.day() as i64, n)),
            'D' => out.push_str(&pad(dt.ordinal() as i64, n)),
            'E' => {
                let w = dt.weekday().num_days_from_sunday() as usize;
                out.push_str(if n >= 4 { locale.weekdays[w] } else { locale.short_weekdays[w] });
            }
            'u' => out.push_str(&pad(dt.weekday().number_from_monday() as i64, n)),
            'a' => out.push_str(locale.am_pm[(dt.hour() >= 12) as usize]),
            'H' => out.push_str(&pad(dt.hour() as i64, n)),
            'k' => out.push_str(&pad(if dt.hour() == 0 { 24 } else { dt.hour() as i64 }, n)),
            'K' => out.push_str(&pad((dt.hour() % 12) as i64, n)),
            'h' => out.push_str(&pad(if dt.hour() % 12 == 0 { 12 } else { (dt.hour() % 12) as i64 }, n)),
            'm' => out.push_str(&pad(dt.minute() as i64, n)),
            's' => out.push_str(&pad(dt.second() as i64, n)),
            'S' => out.push_str(&pad((dt.nanosecond() / 1_000_000) as i64, n)),
            'z' => out.push_str(if n >= 4 { "Coordinated Universal Time" } else { "UTC" }),
            'Z' => out.push_str("+0000"),
            'X' => out.push('Z'),
            'G' => out.push_str("AD"),
            _ => return None,
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::locale::{EN_US, ES_DO};
    use chrono::NaiveDate;

    #[test]
    fn formats_common_patterns() {
        let dt = NaiveDate::from_ymd_opt(2026, 3, 5).unwrap().and_hms_opt(14, 7, 9).unwrap();
        assert_eq!(format_date("dd/MM/yyyy", &dt, &EN_US).unwrap(), "05/03/2026");
        assert_eq!(format_date("MMM d, yyyy h:mm a", &dt, &EN_US).unwrap(), "Mar 5, 2026 2:07 PM");
        assert_eq!(format_date("EEEE d 'de' MMMM 'de' yyyy", &dt, &ES_DO).unwrap(), "jueves 5 de marzo de 2026");
        assert_eq!(format_date("yy-MM-dd HH:mm:ss", &dt, &EN_US).unwrap(), "26-03-05 14:07:09");
        assert_eq!(format_date("'Q''s' yyyy", &dt, &EN_US).unwrap(), "Q's 2026");
        // Values printed by Java 21.0.13 for es-DO.
        assert_eq!(format_date("EEE MMM hh:mm a", &dt, &ES_DO).unwrap(), "jue mar 02:07 p.\u{A0}m.");
    }
}
