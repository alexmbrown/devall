//! Date handling shared by the date matchers and the templating helpers: WireMock-style offsets
//! (`3 days`, `now -1 months`), Java `DateTimeFormatter` patterns, time zones and truncation.

use std::fmt::Display;
use std::str::FromStr;

use chrono::{DateTime, Datelike, Duration, FixedOffset, Months, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Millis,
    Seconds,
    Minutes,
    Hours,
    Days,
    Weeks,
    Months,
    Years,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offset {
    pub amount: i64,
    pub unit: Unit,
}

pub fn parse_unit(text: &str) -> Option<Unit> {
    let t = text.trim().to_ascii_lowercase();
    let t = t.strip_suffix('s').filter(|s| !s.is_empty() && *s != "m").unwrap_or(&t).to_string();
    Some(match t.as_str() {
        "millisecond" | "milli" | "ms" => Unit::Millis,
        "second" | "sec" => Unit::Seconds,
        "minute" | "min" => Unit::Minutes,
        "hour" | "h" => Unit::Hours,
        "day" | "d" => Unit::Days,
        "week" | "w" => Unit::Weeks,
        "month" => Unit::Months,
        "year" | "y" => Unit::Years,
        _ => return None,
    })
}

/// `3 days`, `-24 seconds`, `+1 month`.
pub fn parse_offset(text: &str) -> Option<Offset> {
    let t = text.trim();
    let split = t.find(|c: char| !(c.is_ascii_digit() || c == '-' || c == '+')).unwrap_or(t.len());
    let (num, unit) = t.split_at(split);
    let num = num.trim().replace('+', "");
    let amount: i64 = num.parse().ok()?;
    Some(Offset { amount, unit: parse_unit(unit)? })
}

pub fn apply_offset(dt: DateTime<Utc>, off: Offset) -> DateTime<Utc> {
    let n = off.amount;
    match off.unit {
        Unit::Millis => dt + Duration::milliseconds(n),
        Unit::Seconds => dt + Duration::seconds(n),
        Unit::Minutes => dt + Duration::minutes(n),
        Unit::Hours => dt + Duration::hours(n),
        Unit::Days => dt + Duration::days(n),
        Unit::Weeks => dt + Duration::weeks(n),
        Unit::Months | Unit::Years => {
            let months = if off.unit == Unit::Years { n.saturating_mul(12) } else { n };
            let m = Months::new(months.unsigned_abs().min(u32::MAX as u64) as u32);
            let shifted = if months >= 0 { dt.checked_add_months(m) } else { dt.checked_sub_months(m) };
            shifted.unwrap_or(dt)
        }
    }
}

/// Parses the common ISO-8601 / RFC-1123 shapes, date-only values and epoch numbers.
pub fn parse_flexible(text: &str) -> Option<DateTime<Utc>> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(t) {
        return Some(dt.with_timezone(&Utc));
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f%z", "%Y-%m-%dT%H:%M%z", "%Y-%m-%d %H:%M:%S%.f%z"] {
        if let Ok(dt) = DateTime::parse_from_str(t, fmt) {
            return Some(dt.with_timezone(&Utc));
        }
    }
    if let Ok(dt) = DateTime::parse_from_rfc2822(t) {
        return Some(dt.with_timezone(&Utc));
    }
    let gmt = t.replace(" GMT", " +0000").replace(" UTC", " +0000");
    if let Ok(dt) = DateTime::parse_from_str(&gmt, "%a, %d %b %Y %H:%M:%S %z") {
        return Some(dt.with_timezone(&Utc));
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M"] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(t, fmt) {
            return Some(dt.and_utc());
        }
    }
    if let Ok(d) = NaiveDate::parse_from_str(t, "%Y-%m-%d") {
        return d.and_hms_opt(0, 0, 0).map(|dt| dt.and_utc());
    }
    if t.chars().all(|c| c.is_ascii_digit() || c == '-') {
        let n: i64 = t.parse().ok()?;
        return from_epoch_guess(n);
    }
    None
}

/// Epoch seconds or milliseconds, guessed from magnitude.
pub fn from_epoch_guess(n: i64) -> Option<DateTime<Utc>> {
    if n.abs() > 100_000_000_000 { DateTime::from_timestamp_millis(n) } else { DateTime::from_timestamp(n, 0) }
}

/// Parses with an explicit Java pattern (or `epoch` / `unix`).
pub fn parse_with_format(text: &str, format: &str) -> Option<DateTime<Utc>> {
    let t = text.trim();
    match format {
        "epoch" => return t.parse::<i64>().ok().and_then(DateTime::from_timestamp_millis),
        "unix" => return t.parse::<i64>().ok().and_then(|s| DateTime::from_timestamp(s, 0)),
        _ => {}
    }
    let fmt = java_to_strftime(format, false);
    if let Ok(dt) = DateTime::parse_from_str(t, &fmt) {
        return Some(dt.with_timezone(&Utc));
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(t, &fmt) {
        return Some(dt.and_utc());
    }
    if let Ok(d) = NaiveDate::parse_from_str(t, &fmt) {
        return d.and_hms_opt(0, 0, 0).map(|dt| dt.and_utc());
    }
    None
}

/// Converts a Java `DateTimeFormatter` / `SimpleDateFormat` pattern into a chrono strftime string.
pub fn java_to_strftime(pattern: &str, zero_offset: bool) -> String {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let push_lit = |out: &mut String, c: char| {
        if c == '%' {
            out.push_str("%%");
        } else {
            out.push(c);
        }
    };
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
                    i += 1;
                    break;
                }
                push_lit(&mut out, chars[i]);
                i += 1;
            }
            continue;
        }
        if c.is_ascii_alphabetic() {
            let mut n = 1;
            while chars.get(i + n) == Some(&c) {
                n += 1;
            }
            let token = match c {
                'y' | 'u' | 'Y' => {
                    if n == 2 {
                        "%y"
                    } else {
                        "%Y"
                    }
                }
                'M' | 'L' => match n {
                    1 => "%-m",
                    2 => "%m",
                    3 => "%b",
                    _ => "%B",
                },
                'd' => {
                    if n == 1 {
                        "%-d"
                    } else {
                        "%d"
                    }
                }
                'D' => "%j",
                'H' | 'k' => {
                    if n == 1 {
                        "%-H"
                    } else {
                        "%H"
                    }
                }
                'h' | 'K' => {
                    if n == 1 {
                        "%-I"
                    } else {
                        "%I"
                    }
                }
                'm' => {
                    if n == 1 {
                        "%-M"
                    } else {
                        "%M"
                    }
                }
                's' => {
                    if n == 1 {
                        "%-S"
                    } else {
                        "%S"
                    }
                }
                'S' => match n {
                    1..=3 => "%3f",
                    4..=6 => "%6f",
                    _ => "%9f",
                },
                'n' => "%9f",
                'a' => "%p",
                'E' => {
                    if n >= 4 {
                        "%A"
                    } else {
                        "%a"
                    }
                }
                'Z' => {
                    if n >= 4 {
                        "%:z"
                    } else {
                        "%z"
                    }
                }
                'X' => {
                    if zero_offset {
                        "Z"
                    } else if n <= 2 {
                        "%z"
                    } else {
                        "%:z"
                    }
                }
                'x' => {
                    if n <= 2 {
                        "%z"
                    } else {
                        "%:z"
                    }
                }
                'z' | 'V' | 'O' => "%Z",
                'G' => "AD",
                _ => {
                    for _ in 0..n {
                        out.push(c);
                    }
                    i += n;
                    continue;
                }
            };
            out.push_str(token);
            i += n;
            continue;
        }
        push_lit(&mut out, c);
        i += 1;
    }
    out
}

/// A named IANA zone, a fixed offset or UTC.
#[derive(Debug, Clone, Copy)]
pub enum Zone {
    Utc,
    Fixed(FixedOffset),
    Named(chrono_tz::Tz),
}

pub fn parse_zone(text: &str) -> Option<Zone> {
    let t = text.trim();
    if t.eq_ignore_ascii_case("utc") || t.eq_ignore_ascii_case("gmt") || t == "Z" {
        return Some(Zone::Utc);
    }
    if let Ok(tz) = chrono_tz::Tz::from_str(t) {
        return Some(Zone::Named(tz));
    }
    let body = t.strip_prefix("GMT").or_else(|| t.strip_prefix("UTC")).unwrap_or(t);
    let (sign, rest) = match body.chars().next()? {
        '+' => (1, &body[1..]),
        '-' => (-1, &body[1..]),
        _ => return None,
    };
    let digits: String = rest.chars().filter(char::is_ascii_digit).collect();
    let (h, m) = match digits.len() {
        1 | 2 => (digits.parse::<i32>().ok()?, 0),
        3 | 4 => (digits[..digits.len() - 2].parse::<i32>().ok()?, digits[digits.len() - 2..].parse::<i32>().ok()?),
        _ => return None,
    };
    FixedOffset::east_opt(sign * (h * 3600 + m * 60)).map(Zone::Fixed)
}

fn format_tz<Tz: TimeZone>(dt: &DateTime<Tz>, format: Option<&str>, default_fmt: &str) -> String
where
    Tz::Offset: Display,
{
    let zero = chrono::Offset::fix(dt.offset()).local_minus_utc() == 0;
    match format {
        Some(f) => {
            let pattern = java_to_strftime(f, zero);
            let mut s = String::new();
            use std::fmt::Write as _;
            if write!(s, "{}", dt.format(&pattern)).is_err() {
                return dt.format(default_fmt).to_string();
            }
            s
        }
        None => dt.format(default_fmt).to_string(),
    }
}

/// Renders a date like WireMock: ISO-8601 by default (`2024-01-02T03:04:05Z`), `epoch` (ms),
/// `unix` (s) or a Java pattern, optionally in a time zone.
pub fn format_date(dt: DateTime<Utc>, format: Option<&str>, zone: Option<Zone>) -> String {
    match format {
        Some("epoch") => return dt.timestamp_millis().to_string(),
        Some("unix") => return dt.timestamp().to_string(),
        _ => {}
    }
    match zone.unwrap_or(Zone::Utc) {
        Zone::Utc => format_tz(&dt, format, "%Y-%m-%dT%H:%M:%SZ"),
        Zone::Fixed(off) => format_tz(&dt.with_timezone(&off), format, "%Y-%m-%dT%H:%M:%S%:z"),
        Zone::Named(tz) => format_tz(&dt.with_timezone(&tz), format, "%Y-%m-%dT%H:%M:%S%:z"),
    }
}

/// `first day of month`, `last day of year`, `first hour of day`, …
pub fn truncate(dt: DateTime<Utc>, spec: &str) -> Option<DateTime<Utc>> {
    let midnight = |d: NaiveDate| d.and_hms_opt(0, 0, 0).map(|x| x.and_utc());
    let date = dt.date_naive();
    match spec.trim().to_ascii_lowercase().as_str() {
        "first minute of hour" => dt.with_minute(0)?.with_second(0)?.with_nanosecond(0),
        "first second of minute" => dt.with_second(0)?.with_nanosecond(0),
        "first hour of day" => midnight(date),
        "first day of month" => midnight(date.with_day(1)?),
        "first day of next month" => midnight(date.with_day(1)?.checked_add_months(Months::new(1))?),
        "last day of month" => midnight(date.with_day(1)?.checked_add_months(Months::new(1))?.pred_opt()?),
        "first day of year" => midnight(NaiveDate::from_ymd_opt(date.year(), 1, 1)?),
        "first day of next year" => midnight(NaiveDate::from_ymd_opt(date.year() + 1, 1, 1)?),
        "last day of year" => midnight(NaiveDate::from_ymd_opt(date.year(), 12, 31)?),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn offsets() {
        let base = at("2024-01-31T10:00:00Z");
        assert_eq!(apply_offset(base, parse_offset("3 days").unwrap()), at("2024-02-03T10:00:00Z"));
        assert_eq!(apply_offset(base, parse_offset("-24 seconds").unwrap()), at("2024-01-31T09:59:36Z"));
        assert_eq!(apply_offset(base, parse_offset("1 months").unwrap()), at("2024-02-29T10:00:00Z"));
        assert_eq!(apply_offset(base, parse_offset("+1 year").unwrap()), at("2025-01-31T10:00:00Z"));
        assert!(parse_offset("soon").is_none());
    }

    #[test]
    fn java_patterns() {
        let dt = at("2024-03-05T07:08:09.123Z");
        assert_eq!(format_date(dt, Some("yyyy-MM-dd"), None), "2024-03-05");
        assert_eq!(format_date(dt, Some("dd/MM/yyyy HH:mm:ss.SSS"), None), "05/03/2024 07:08:09.123");
        assert_eq!(format_date(dt, Some("yyyy-MM-dd'T'HH:mm:ssXXX"), None), "2024-03-05T07:08:09Z");
        assert_eq!(format_date(dt, Some("EEE, d MMM yyyy"), None), "Tue, 5 Mar 2024");
        assert_eq!(format_date(dt, Some("epoch"), None), "1709622489123");
        assert_eq!(format_date(dt, Some("unix"), None), "1709622489");
        assert_eq!(format_date(dt, None, None), "2024-03-05T07:08:09Z");
        let syd = parse_zone("Australia/Sydney").unwrap();
        assert_eq!(format_date(dt, None, Some(syd)), "2024-03-05T18:08:09+11:00");
        assert_eq!(format_date(dt, Some("HH:mm"), Some(parse_zone("+02:00").unwrap())), "09:08");
    }

    #[test]
    fn parsing() {
        assert_eq!(parse_flexible("2024-03-05T07:08:09Z"), Some(at("2024-03-05T07:08:09Z")));
        assert_eq!(parse_flexible("2024-03-05"), Some(at("2024-03-05T00:00:00Z")));
        assert_eq!(parse_flexible("Tue, 05 Mar 2024 07:08:09 GMT"), Some(at("2024-03-05T07:08:09Z")));
        assert_eq!(parse_flexible("1709622489"), Some(at("2024-03-05T07:08:09Z")));
        assert_eq!(parse_with_format("05/03/2024", "dd/MM/yyyy"), Some(at("2024-03-05T00:00:00Z")));
        assert_eq!(parse_with_format("1709622489123", "epoch"), Some(at("2024-03-05T07:08:09.123Z")));
    }

    #[test]
    fn truncation() {
        let dt = at("2024-02-10T07:08:09Z");
        assert_eq!(truncate(dt, "first day of month"), Some(at("2024-02-01T00:00:00Z")));
        assert_eq!(truncate(dt, "last day of month"), Some(at("2024-02-29T00:00:00Z")));
        assert_eq!(truncate(dt, "first day of next year"), Some(at("2025-01-01T00:00:00Z")));
        assert_eq!(truncate(dt, "first minute of hour"), Some(at("2024-02-10T07:00:00Z")));
        assert_eq!(truncate(dt, "nonsense"), None);
    }
}
