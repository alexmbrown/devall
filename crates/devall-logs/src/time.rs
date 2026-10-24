//! Timestamp parsing tuned for log lines: hand-rolled fast paths for ISO-8601/RFC 3339, nginx/Go
//! slash dates, Apache CLF, syslog and ctime stamps; epoch numbers in seconds/ms/µs/ns. All results
//! are Unix milliseconds (UTC). Stamps without a zone use [`TimeCtx::naive`] (local time by
//! default, like most log tools), and stamps without a year (syslog, klog) use [`TimeCtx::year`].

use std::cell::Cell;

use chrono::{Datelike, Offset, TimeZone};

/// How to interpret timestamps that carry no zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NaiveTz {
    #[default]
    Local,
    Utc,
    /// Fixed offset, seconds east of UTC.
    Offset(i32),
}

#[derive(Debug, Clone)]
pub struct TimeCtx {
    pub naive: NaiveTz,
    /// Year for stamps that omit it.
    pub year: i32,
    /// (hour key, offset seconds) of the last local-offset lookup.
    cache: Cell<(i64, i32)>,
}

impl Default for TimeCtx {
    fn default() -> Self {
        Self::new(NaiveTz::Local, current_year())
    }
}

impl TimeCtx {
    pub fn new(naive: NaiveTz, year: i32) -> Self {
        Self { naive, year, cache: Cell::new((i64::MIN, 0)) }
    }

    pub fn utc() -> Self {
        Self::new(NaiveTz::Utc, current_year())
    }

    /// Offset (seconds east) to apply to a zone-less civil time.
    fn naive_offset(&self, days: i64, secs: i64) -> i32 {
        match self.naive {
            NaiveTz::Utc => 0,
            NaiveTz::Offset(o) => o,
            NaiveTz::Local => {
                let key = days * 24 + secs / 3600;
                let (k, off) = self.cache.get();
                if k == key {
                    return off;
                }
                let off = local_offset(days, secs);
                self.cache.set((key, off));
                off
            }
        }
    }

    /// Unix ms for a civil time; `offset` is seconds east of UTC when the stamp carried a zone.
    pub fn civil_to_ms(&self, c: &Civil, offset: Option<i32>) -> Option<i64> {
        if !(1..=12).contains(&c.mo) || !(1..=31).contains(&c.d) || c.h > 24 || c.mi > 59 || c.s > 60 {
            return None;
        }
        let days = days_from_civil(c.y as i64, c.mo, c.d);
        let secs = c.h as i64 * 3600 + c.mi as i64 * 60 + c.s as i64;
        let off = offset.unwrap_or_else(|| self.naive_offset(days, secs)) as i64;
        Some((days * 86_400 + secs - off) * 1000 + c.ms as i64)
    }
}

fn local_offset(days: i64, secs: i64) -> i32 {
    let Some(date) = chrono::NaiveDate::from_num_days_from_ce_opt((days + 719_163) as i32) else { return 0 };
    let Some(time) = chrono::NaiveTime::from_num_seconds_from_midnight_opt(secs.clamp(0, 86_399) as u32, 0) else { return 0 };
    match chrono::Local.offset_from_local_datetime(&date.and_time(time)).earliest() {
        Some(o) => o.fix().local_minus_utc(),
        None => 0,
    }
}

pub fn current_year() -> i32 {
    chrono::Utc::now().year()
}

/// A civil (calendar) time with millisecond precision.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Civil {
    pub y: i32,
    pub mo: u32,
    pub d: u32,
    pub h: u32,
    pub mi: u32,
    pub s: u32,
    pub ms: u32,
}

/// Days since 1970-01-01 (proleptic Gregorian), Howard Hinnant's algorithm.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
const WEEKDAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

fn month_from(b: &[u8]) -> Option<u32> {
    if b.len() < 3 {
        return None;
    }
    let m = [b[0].to_ascii_lowercase(), b[1].to_ascii_lowercase(), b[2].to_ascii_lowercase()];
    MONTHS.iter().position(|x| x.as_bytes() == m).map(|i| i as u32 + 1)
}

fn is_weekday(b: &[u8]) -> bool {
    if b.len() < 3 {
        return false;
    }
    let w = [b[0].to_ascii_lowercase(), b[1].to_ascii_lowercase(), b[2].to_ascii_lowercase()];
    WEEKDAYS.iter().any(|x| x.as_bytes() == w)
}

/// Reads exactly `n` ASCII digits at `*i`.
fn digits(b: &[u8], i: &mut usize, n: usize) -> Option<u32> {
    if *i + n > b.len() {
        return None;
    }
    let mut v = 0u32;
    for k in 0..n {
        let c = b[*i + k];
        if !c.is_ascii_digit() {
            return None;
        }
        v = v * 10 + (c - b'0') as u32;
    }
    *i += n;
    Some(v)
}

/// Reads 1 or 2 digits (also accepts a leading space as padding, e.g. syslog `Oct  1`).
fn digits_1_2(b: &[u8], i: &mut usize) -> Option<u32> {
    let mut v = 0u32;
    let mut n = 0;
    while *i < b.len() && n < 2 && b[*i].is_ascii_digit() {
        v = v * 10 + (b[*i] - b'0') as u32;
        *i += 1;
        n += 1;
    }
    (n > 0).then_some(v)
}

fn expect(b: &[u8], i: &mut usize, c: u8) -> Option<()> {
    if *i < b.len() && b[*i] == c {
        *i += 1;
        Some(())
    } else {
        None
    }
}

/// `hh:mm[:ss][.fff]` starting at `*i`.
fn clock(b: &[u8], i: &mut usize, c: &mut Civil) -> Option<()> {
    c.h = digits(b, i, 2)?;
    expect(b, i, b':')?;
    c.mi = digits(b, i, 2)?;
    if *i < b.len() && b[*i] == b':' {
        *i += 1;
        c.s = digits(b, i, 2)?;
        fraction(b, i, c);
    }
    Some(())
}

fn fraction(b: &[u8], i: &mut usize, c: &mut Civil) {
    if *i + 1 < b.len() && (b[*i] == b'.' || b[*i] == b',') && b[*i + 1].is_ascii_digit() {
        *i += 1;
        let mut ms = 0u32;
        let mut n = 0;
        while *i < b.len() && b[*i].is_ascii_digit() {
            if n < 3 {
                ms = ms * 10 + (b[*i] - b'0') as u32;
            }
            n += 1;
            *i += 1;
        }
        while n < 3 {
            ms *= 10;
            n += 1;
        }
        c.ms = ms;
    }
}

/// Zone designator: `Z`, `+hh:mm`, `+hhmm`, `+hh`, optionally after one space; also ` UTC`/` GMT`.
fn zone(b: &[u8], i: &mut usize) -> Option<i32> {
    let start = *i;
    let mut j = *i;
    if j < b.len() && (b[j] == b'Z' || b[j] == b'z') {
        // Not part of a longer word (e.g. "Zebra").
        if j + 1 >= b.len() || !b[j + 1].is_ascii_alphanumeric() {
            *i = j + 1;
            return Some(0);
        }
        return None;
    }
    if j < b.len() && b[j] == b' ' {
        j += 1;
        if b.len() >= j + 3 && (b[j..j + 3].eq_ignore_ascii_case(b"UTC") || b[j..j + 3].eq_ignore_ascii_case(b"GMT")) {
            if b.len() == j + 3 || !b[j + 3].is_ascii_alphanumeric() {
                *i = j + 3;
                return Some(0);
            }
            return None;
        }
    }
    if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
        let sign = if b[j] == b'-' { -1 } else { 1 };
        j += 1;
        let hh = digits(b, &mut j, 2)?;
        let mut mm = 0;
        if j < b.len() && b[j] == b':' {
            j += 1;
            mm = digits(b, &mut j, 2)?;
        } else if j + 1 < b.len() && b[j].is_ascii_digit() && b[j + 1].is_ascii_digit() {
            mm = digits(b, &mut j, 2)?;
        } else if j > start + 3 && b[start] == b' ' {
            // " -07" alone is too ambiguous after a space.
            return None;
        }
        if hh > 14 || mm > 59 {
            return None;
        }
        *i = j;
        return Some(sign * (hh as i32 * 3600 + mm as i32 * 60));
    }
    None
}

/// `YYYY-MM-DD[T ]hh:mm[:ss[.fff]][zone]` or with `/` date separators.
fn iso(b: &[u8], ctx: &TimeCtx) -> Option<(i64, usize)> {
    let mut i = 0;
    let mut c = Civil { y: digits(b, &mut i, 4)? as i32, ..Default::default() };
    let sep = *b.get(i)?;
    if sep != b'-' && sep != b'/' && sep != b'.' {
        return None;
    }
    i += 1;
    c.mo = digits(b, &mut i, 2)?;
    expect(b, &mut i, sep)?;
    c.d = digits(b, &mut i, 2)?;
    let t = *b.get(i)?;
    if t != b'T' && t != b' ' && t != b't' && t != b'_' {
        return None;
    }
    i += 1;
    clock(b, &mut i, &mut c)?;
    let off = zone(b, &mut i);
    Some((ctx.civil_to_ms(&c, off)?, i))
}

/// Apache CLF: `10/Oct/2000:13:55:36 -0700`.
fn clf(b: &[u8], ctx: &TimeCtx) -> Option<(i64, usize)> {
    let mut i = 0;
    let mut c = Civil { d: digits_1_2(b, &mut i)?, ..Default::default() };
    expect(b, &mut i, b'/')?;
    c.mo = month_from(b.get(i..)?)?;
    i += 3;
    expect(b, &mut i, b'/')?;
    c.y = digits(b, &mut i, 4)? as i32;
    expect(b, &mut i, b':')?;
    clock(b, &mut i, &mut c)?;
    let off = zone(b, &mut i);
    Some((ctx.civil_to_ms(&c, off)?, i))
}

/// Syslog `Oct 11 22:14:15[.ffffff]` (no year) or ctime `Wed Oct 11 22:14:15[.ffffff] 2000`.
fn syslog_like(b: &[u8], ctx: &TimeCtx) -> Option<(i64, usize)> {
    let mut i = 0;
    let weekday = is_weekday(b) && b.get(3) == Some(&b' ');
    if weekday {
        i = 4;
    }
    let mut c = Civil { mo: month_from(b.get(i..)?)?, ..Default::default() };
    i += 3;
    expect(b, &mut i, b' ')?;
    if b.get(i) == Some(&b' ') {
        i += 1;
    }
    c.d = digits_1_2(b, &mut i)?;
    expect(b, &mut i, b' ')?;
    clock(b, &mut i, &mut c)?;
    c.y = ctx.year;
    // ctime (and some syslog daemons) put the year after the time.
    let mut j = i;
    if expect(b, &mut j, b' ').is_some()
        && let Some(y) = digits(b, &mut j, 4)
        && (j >= b.len() || !b[j].is_ascii_digit())
    {
        c.y = y as i32;
        i = j;
    }
    let off = zone(b, &mut i);
    Some((ctx.civil_to_ms(&c, off)?, i))
}

/// A timestamp at the very start of `s`: `(unix ms, bytes consumed)`.
pub fn parse_prefix(s: &str, ctx: &TimeCtx) -> Option<(i64, usize)> {
    let b = s.as_bytes();
    let first = *b.first()?;
    if first.is_ascii_digit() {
        iso(b, ctx).or_else(|| clf(b, ctx))
    } else if first.is_ascii_alphabetic() {
        syslog_like(b, ctx)
    } else {
        None
    }
}

/// Epoch number → ms, guessing the unit from the magnitude (s, ms, µs, ns).
pub fn from_epoch(n: f64) -> Option<i64> {
    if !n.is_finite() || n <= 0.0 {
        return None;
    }
    let ms = if n < 1e11 {
        n * 1000.0
    } else if n < 1e14 {
        n
    } else if n < 1e17 {
        n / 1000.0
    } else {
        n / 1e6
    };
    // 1990..2200 sanity window keeps counters and ids from turning into dates.
    (631_152_000_000.0..7_258_118_400_000.0).contains(&ms).then_some(ms as i64)
}

/// A whole timestamp string (JSON value, query literal…).
pub fn parse_str(s: &str, ctx: &TimeCtx) -> Option<i64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    if let Some((ms, n)) = parse_prefix(t, ctx)
        && t[n..].trim().is_empty()
    {
        return Some(ms);
    }
    if t.bytes().all(|c| c.is_ascii_digit() || c == b'.') {
        return t.parse::<f64>().ok().and_then(from_epoch);
    }
    if let Ok(d) = chrono::DateTime::parse_from_rfc2822(t) {
        return Some(d.timestamp_millis());
    }
    if let Ok(d) = chrono::DateTime::parse_from_rfc3339(t) {
        return Some(d.timestamp_millis());
    }
    None
}

/// `2024-01-02T03:04:05.123Z`.
pub fn format_iso(ms: i64) -> String {
    match chrono::DateTime::from_timestamp_millis(ms) {
        Some(d) => d.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
        None => ms.to_string(),
    }
}

/// `500ms`, `30s`, `15m`, `1h30m`, `2d`, `1w` → ms.
pub fn parse_duration(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let b = s.as_bytes();
    let mut i = 0;
    let mut total = 0f64;
    while i < b.len() {
        let start = i;
        while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
            i += 1;
        }
        if start == i {
            return None;
        }
        let n: f64 = s[start..i].parse().ok()?;
        let ustart = i;
        while i < b.len() && b[i].is_ascii_alphabetic() {
            i += 1;
        }
        let unit = match &s[ustart..i] {
            "ms" => 1.0,
            "s" | "sec" => 1000.0,
            "m" | "min" => 60_000.0,
            "h" | "hr" => 3_600_000.0,
            "d" => 86_400_000.0,
            "w" => 604_800_000.0,
            _ => return None,
        };
        total += n * unit;
    }
    Some(total as i64)
}

/// Query-time literal: `now`, `now-15m`, `-15m` (ago), ISO/other stamps, `YYYY-MM-DD` (midnight),
/// or epoch numbers.
pub fn parse_query_time(s: &str, now: i64, ctx: &TimeCtx) -> Option<i64> {
    let t = s.trim();
    let lower = t.to_ascii_lowercase();
    if lower == "now" {
        return Some(now);
    }
    if let Some(rest) = lower.strip_prefix("now-").or_else(|| lower.strip_prefix('-')) {
        return parse_duration(rest).map(|d| now - d);
    }
    if let Some(rest) = lower.strip_prefix("now+") {
        return parse_duration(rest).map(|d| now + d);
    }
    if let Some(ms) = parse_str(t, ctx) {
        return Some(ms);
    }
    let b = t.as_bytes();
    if b.len() == 10 {
        let mut i = 0;
        let y = digits(b, &mut i, 4)?;
        let sep = *b.get(i)?;
        i += 1;
        let mo = digits(b, &mut i, 2)?;
        expect(b, &mut i, sep)?;
        let d = digits(b, &mut i, 2)?;
        return ctx.civil_to_ms(&Civil { y: y as i32, mo, d, ..Default::default() }, None);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc() -> TimeCtx {
        TimeCtx::new(NaiveTz::Utc, 2024)
    }

    #[test]
    fn iso_variants() {
        let c = utc();
        let base = 1_704_164_645_000; // 2024-01-02T03:04:05Z
        assert_eq!(parse_prefix("2024-01-02T03:04:05Z rest", &c), Some((base, 20)));
        assert_eq!(parse_prefix("2024-01-02T03:04:05.123Z", &c).unwrap().0, base + 123);
        assert_eq!(parse_prefix("2024-01-02 03:04:05,5 - x", &c).unwrap().0, base + 500);
        assert_eq!(parse_prefix("2024-01-02T05:04:05+02:00", &c).unwrap().0, base);
        assert_eq!(parse_prefix("2024-01-02T05:04:05+0200", &c).unwrap().0, base);
        assert_eq!(parse_prefix("2024/01/02 03:04:05 [error]", &c).unwrap(), (base, 19));
        assert_eq!(parse_prefix("2024-01-02T03:04:05.123456789Z", &c).unwrap().0, base + 123);
        assert_eq!(parse_prefix("2024-01-02 03:04:05 UTC", &c).unwrap(), (base, 23));
        assert!(parse_prefix("2024-13-02T03:04:05Z", &c).is_none());
        assert!(parse_prefix("hello", &c).is_none());
        let plus2 = TimeCtx::new(NaiveTz::Offset(7200), 2024);
        assert_eq!(parse_prefix("2024-01-02T05:04:05", &plus2).unwrap().0, base);
    }

    #[test]
    fn clf_syslog_ctime() {
        let c = utc();
        assert_eq!(parse_prefix("10/Oct/2000:13:55:36 -0700] x", &c).unwrap().0, 971_211_336_000);
        let (ms, n) = parse_prefix("Oct 11 22:14:15 host app:", &c).unwrap();
        assert_eq!(n, 15);
        assert_eq!(format_iso(ms), "2024-10-11T22:14:15.000Z");
        assert_eq!(format_iso(parse_prefix("Oct  1 02:03:04 h", &c).unwrap().0), "2024-10-01T02:03:04.000Z");
        let (ms, n) = parse_prefix("Wed Oct 11 14:32:52.123456 2000] [core:error]", &c).unwrap();
        assert_eq!(format_iso(ms), "2000-10-11T14:32:52.123Z");
        assert_eq!(n, 31);
    }

    #[test]
    fn epochs_and_strings() {
        let c = utc();
        assert_eq!(from_epoch(1_704_164_645.0), Some(1_704_164_645_000));
        assert_eq!(from_epoch(1_704_164_645_123.0), Some(1_704_164_645_123));
        assert_eq!(from_epoch(1_704_164_645_123_456.0), Some(1_704_164_645_123));
        assert_eq!(from_epoch(42.0), None);
        assert_eq!(parse_str("1704164645.5", &c), Some(1_704_164_645_500));
        assert_eq!(parse_str("Tue, 2 Jan 2024 03:04:05 GMT", &c), Some(1_704_164_645_000));
        assert_eq!(parse_duration("1h30m"), Some(5_400_000));
        assert_eq!(parse_duration("500ms"), Some(500));
        assert_eq!(parse_query_time("now-15m", 1_000_000_000, &c), Some(1_000_000_000 - 900_000));
        assert_eq!(parse_query_time("2024-01-02", 0, &c), Some(1_704_153_600_000));
    }

    #[test]
    fn days() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
    }
}
