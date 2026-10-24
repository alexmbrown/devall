use chrono::{DateTime, Datelike, Local, TimeZone, Utc};
use serde_json::{Value, json};

use crate::{Result, invalid};

/// Parses epoch seconds/ms/µs/ns (by digit count), RFC 3339 / ISO 8601, RFC 2822, or `now`.
pub fn parse(input: &str, now: DateTime<Utc>) -> Result<(DateTime<Utc>, &'static str)> {
    let s = input.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("now") {
        return Ok((now, "now"));
    }
    if let Ok(n) = s.parse::<i64>() {
        let digits = s.trim_start_matches('-').len();
        let (dt, kind) = match digits {
            0..=11 => (Utc.timestamp_opt(n, 0).single(), "unix seconds"),
            12..=14 => (Utc.timestamp_millis_opt(n).single(), "unix milliseconds"),
            15..=17 => (Some(Utc.timestamp_nanos(n.saturating_mul(1000))), "unix microseconds"),
            _ => (Some(Utc.timestamp_nanos(n)), "unix nanoseconds"),
        };
        return dt.map(|d| (d, kind)).ok_or_else(|| invalid("timestamp out of range"));
    }
    if let Ok(f) = s.parse::<f64>() {
        let ms = (f * 1000.0).round() as i64;
        return Utc.timestamp_millis_opt(ms).single().map(|d| (d, "unix seconds (fractional)")).ok_or_else(|| invalid("timestamp out of range"));
    }
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return Ok((d.with_timezone(&Utc), "RFC 3339 / ISO 8601"));
    }
    if let Ok(d) = DateTime::parse_from_rfc2822(s) {
        return Ok((d.with_timezone(&Utc), "RFC 2822"));
    }
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f"] {
        if let Ok(n) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Ok((Utc.from_utc_datetime(&n), "ISO 8601 without zone (assumed UTC)"));
        }
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok((Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap()), "date (assumed UTC midnight)"));
    }
    Err(invalid("unrecognized time: use epoch seconds/ms, ISO 8601, RFC 2822 or `now`"))
}

pub fn convert(input: &str, now: DateTime<Utc>) -> Result<Value> {
    let (dt, detected) = parse(input, now)?;
    let delta = dt.signed_duration_since(now).num_seconds();
    Ok(json!({
        "detected": detected,
        "unixSeconds": dt.timestamp(),
        "unixMillis": dt.timestamp_millis(),
        "iso8601Utc": dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "local": dt.with_timezone(&Local).to_rfc3339_opts(chrono::SecondsFormat::Millis, false),
        "rfc2822": dt.to_rfc2822(),
        "httpDate": dt.format("%a, %d %b %Y %H:%M:%S GMT").to_string(),
        "dayOfWeek": dt.format("%A").to_string(),
        "relative": if delta == 0 { "now".to_string() } else if delta > 0 { format!("in {}", human(delta)) } else { format!("{} ago", human(-delta)) },
    }))
}

/// Duration between two times (any format [`parse`] accepts), as totals, a breakdown, ISO 8601
/// and a calendar difference (years / months / days).
pub fn diff(a: &str, b: &str, now: DateTime<Utc>) -> Result<Value> {
    let (from, from_kind) = parse(a, now)?;
    let (to, to_kind) = parse(if b.trim().is_empty() { "now" } else { b }, now)?;
    let d = to.signed_duration_since(from);
    let ms = d.num_milliseconds();
    let abs = ms.unsigned_abs();
    let (days, rem) = (abs / 86_400_000, abs % 86_400_000);
    let (hours, rem) = (rem / 3_600_000, rem % 3_600_000);
    let (minutes, rem) = (rem / 60_000, rem % 60_000);
    let (seconds, millis) = (rem / 1000, rem % 1000);
    let mut iso = format!("P{}{days}D", if ms < 0 { "-" } else { "" });
    if hours + minutes + seconds + millis > 0 {
        iso.push('T');
        if hours > 0 {
            iso.push_str(&format!("{hours}H"));
        }
        if minutes > 0 {
            iso.push_str(&format!("{minutes}M"));
        }
        if seconds > 0 || millis > 0 {
            iso.push_str(&if millis > 0 { format!("{seconds}.{millis:03}S") } else { format!("{seconds}S") });
        }
    }
    let human = [(days, "d"), (hours, "h"), (minutes, "m"), (seconds, "s")]
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, u)| format!("{n}{u}"))
        .collect::<Vec<_>>()
        .join(" ");
    let (lo, hi) = if from <= to { (from, to) } else { (to, from) };
    let mut months = (hi.year() - lo.year()) * 12 + hi.month() as i32 - lo.month() as i32;
    let add = |m: i32| lo.checked_add_months(chrono::Months::new(m.max(0) as u32));
    while months > 0 && add(months).is_none_or(|t| t > hi) {
        months -= 1;
    }
    let anchor = add(months).unwrap_or(lo);
    let rest = hi.signed_duration_since(anchor).num_seconds();
    let calendar = format!(
        "{} years, {} months, {} days, {:02}:{:02}:{:02}",
        months / 12,
        months % 12,
        rest / 86_400,
        (rest % 86_400) / 3600,
        (rest % 3600) / 60,
        rest % 60
    );
    Ok(json!({
        "from": from.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "to": to.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "detected": [from_kind, to_kind],
        "direction": if ms >= 0 { "forward" } else { "backward" },
        "human": if human.is_empty() { format!("{millis}ms") } else { human },
        "iso8601": iso,
        "calendar": calendar,
        "totalMilliseconds": ms,
        "totalSeconds": d.num_seconds(),
        "totalMinutes": ms as f64 / 60_000.0,
        "totalHours": ms as f64 / 3_600_000.0,
        "totalDays": ms as f64 / 86_400_000.0,
        "totalWeeks": ms as f64 / 604_800_000.0,
        "breakdown": { "days": days, "hours": hours, "minutes": minutes, "seconds": seconds, "milliseconds": millis },
    }))
}

fn human(secs: i64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m {}s", s / 60, s % 60),
        s if s < 86_400 => format!("{}h {}m", s / 3600, (s % 3600) / 60),
        s => format!("{}d {}h", s / 86_400, (s % 86_400) / 3600),
    }
}
