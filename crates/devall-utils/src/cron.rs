//! Cron expressions: human description and next run times in an IANA time zone (croner + chrono-tz).

use std::str::FromStr;

use chrono::{DateTime, Local, TimeZone, Utc};
use croner::Cron;
use serde_json::{Value, json};

use crate::{Result, invalid};

fn nickname(expr: &str) -> Option<&'static str> {
    Some(match expr.to_ascii_lowercase().as_str() {
        "@yearly" | "@annually" => "0 0 1 1 *",
        "@monthly" => "0 0 1 * *",
        "@weekly" => "0 0 * * 0",
        "@daily" | "@midnight" => "0 0 * * *",
        "@hourly" => "0 * * * *",
        _ => return None,
    })
}

fn runs<Tz: TimeZone>(cron: &Cron, start: DateTime<Tz>, count: usize, now: DateTime<Utc>) -> Vec<Value>
where
    Tz::Offset: std::fmt::Display,
{
    cron.iter_after(start)
        .take(count)
        .map(|t| {
            let delta = t.with_timezone(&Utc).signed_duration_since(now).num_seconds();
            json!({
                "time": t.to_rfc3339(),
                "display": t.format("%Y-%m-%d %H:%M:%S %Z").to_string(),
                "utc": t.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                "weekday": t.format("%A").to_string(),
                "relative": relative(delta),
            })
        })
        .collect()
}

fn relative(secs: i64) -> String {
    let s = secs.max(0);
    let (d, h, m) = (s / 86_400, (s % 86_400) / 3600, (s % 3600) / 60);
    match (d, h, m) {
        (0, 0, 0) => format!("in {s}s"),
        (0, 0, m) => format!("in {m}m"),
        (0, h, m) => format!("in {h}h {m}m"),
        (d, h, _) => format!("in {d}d {h}h"),
    }
}

pub fn explain(input: &str, tz: &str, count: usize, now: DateTime<Utc>) -> Result<Value> {
    let expr = input.trim();
    if expr.is_empty() {
        return Err(invalid("enter a cron expression, e.g. */5 * * * *"));
    }
    let normalized = nickname(expr).unwrap_or(expr);
    let cron = Cron::from_str(normalized).map_err(|e| invalid(format!("invalid cron expression: {e}")))?;
    let parts: Vec<&str> = normalized.split_whitespace().collect();
    let names: &[&str] = match parts.len() {
        5 => &["minute", "hour", "dayOfMonth", "month", "dayOfWeek"],
        6 => &["second", "minute", "hour", "dayOfMonth", "month", "dayOfWeek"],
        _ => &["second", "minute", "hour", "dayOfMonth", "month", "dayOfWeek", "year"],
    };
    let fields: serde_json::Map<String, Value> = names.iter().zip(&parts).map(|(n, p)| ((*n).to_string(), json!(p))).collect();
    let mut warnings = Vec::new();
    let dom_dow = match parts.len() {
        5 => Some((parts[2], parts[4])),
        6 | 7 => Some((parts[3], parts[5])),
        _ => None,
    };
    if let Some((dom, dow)) = dom_dow
        && !matches!(dom, "*" | "?")
        && !matches!(dow, "*" | "?")
        && !dow.starts_with('+')
    {
        warnings.push("Both day-of-month and day-of-week are set: cron runs when EITHER matches (prefix the weekday with + to require both)".to_string());
    }
    let tz = tz.trim();
    let (zone, next) = if tz.is_empty() || tz.eq_ignore_ascii_case("utc") {
        ("UTC".to_string(), runs(&cron, now, count, now))
    } else if tz.eq_ignore_ascii_case("local") {
        ("local".to_string(), runs(&cron, now.with_timezone(&Local), count, now))
    } else {
        let zone: chrono_tz::Tz = tz.parse().map_err(|_| invalid(format!("unknown time zone {tz:?} (use an IANA name like Europe/Berlin, UTC or local)")))?;
        (zone.name().to_string(), runs(&cron, now.with_timezone(&zone), count, now))
    };
    if next.is_empty() {
        warnings.push("This expression never fires (e.g. 30 February)".to_string());
    }
    Ok(json!({
        "expression": expr,
        "normalized": normalized,
        "description": cron.describe(),
        "fields": fields,
        "timezone": zone,
        "nextRuns": next,
        "warnings": warnings,
    }))
}
